use super::*;

const MAX_CANVAS_NODES: usize = 200;
const MAX_CANVAS_LINKS: usize = 400;

fn target_at_input(
    store: &Store,
    source_id: &str,
    x: f32,
    y: f32,
    tolerance: f32,
) -> Option<String> {
    store
        .canvas_notes
        .iter()
        .filter(|note| note.id != source_id && note.kind != "group")
        .filter_map(|note| {
            let dx = note.x - x;
            let dy = note.y + note.height / 2.0 - y;
            let distance = (dx * dx + dy * dy).sqrt();
            (distance <= tolerance).then_some((distance, note.id.clone()))
        })
        .min_by(|left, right| left.0.total_cmp(&right.0))
        .map(|(_, id)| id)
}

fn source_at_output(
    store: &Store,
    target_id: &str,
    x: f32,
    y: f32,
    tolerance: f32,
) -> Option<String> {
    store
        .canvas_notes
        .iter()
        .filter(|note| note.id != target_id && note.kind != "group")
        .filter_map(|note| {
            let dx = note.x + note.width - x;
            let dy = note.y + note.height / 2.0 - y;
            let distance = (dx * dx + dy * dy).sqrt();
            (distance <= tolerance).then_some((distance, note.id.clone()))
        })
        .min_by(|left, right| left.0.total_cmp(&right.0))
        .map(|(_, id)| id)
}

fn canvas_node_defaults(kind: &str, english: bool) -> (String, f32, f32) {
    match kind {
        "image" => (String::new(), 340.0, 250.0),
        "video" => (String::new(), 400.0, 270.0),
        "audio" => (String::new(), 340.0, 190.0),
        "group" => (
            if english { "Group" } else { "节点组" }.to_string(),
            680.0,
            360.0,
        ),
        _ => (String::new(), 320.0, 210.0),
    }
}

fn sync_history_state(app: &AppWindow, history: &CanvasController) {
    let state = app.global::<AppState>();
    state.set_canvas_can_undo(history.can_undo());
    state.set_canvas_can_redo(history.can_redo());
}

fn persist_canvas(app: &AppWindow, store: &Store) {
    push_canvas_notes(app, store);
    save_local_store(app, store);
}

fn show_canvas_capacity_status(app: &AppWindow) {
    let state = app.global::<AppState>();
    state.set_generation_status(
        if state.get_language().as_str() == "en" {
            "Canvas limit reached (200 nodes / 400 connections)."
        } else {
            "画布已达到上限（200 个节点 / 400 条连线）。"
        }
        .into(),
    );
}

fn sync_canvas_selection_metrics(app: &AppWindow, store: &Store) {
    let state = app.global::<AppState>();
    let ids = selected_ids(&store.canvas_notes);
    state.set_canvas_selected_count(ids.len() as i32);
    if let Some(bounds) = selection_bounds(&store.canvas_notes, &ids) {
        state.set_canvas_focus_x(bounds.x);
        state.set_canvas_focus_y(bounds.y);
        state.set_canvas_focus_width(bounds.width);
        state.set_canvas_focus_height(bounds.height);
    } else {
        state.set_canvas_focus_width(0.0);
        state.set_canvas_focus_height(0.0);
    }
}

fn sync_canvas_selection(app: &AppWindow, store: &Store) {
    sync_canvas_selection_metrics(app, store);
    push_canvas_notes(app, store);
}

fn sync_canvas_selection_rows(app: &AppWindow, store: &Store) {
    sync_canvas_selection_metrics(app, store);

    let state = app.global::<AppState>();
    let canvas_notes = state.get_canvas_notes();
    for row in 0..canvas_notes.row_count() {
        let Some(mut note) = canvas_notes.row_data(row) else {
            continue;
        };
        let selected = store
            .canvas_notes
            .iter()
            .find(|stored| stored.id == note.id.as_str())
            .is_some_and(|stored| stored.selected);
        if note.selected != selected {
            note.selected = selected;
            canvas_notes.set_row_data(row, note);
        }
    }

    let canvas_links = state.get_canvas_links();
    for row in 0..canvas_links.row_count() {
        let Some(mut link) = canvas_links.row_data(row) else {
            continue;
        };
        let source_selected = store
            .canvas_notes
            .iter()
            .find(|note| note.id == link.source_id.as_str())
            .is_some_and(|note| note.selected);
        let target_selected = store
            .canvas_notes
            .iter()
            .find(|note| note.id == link.target_id.as_str())
            .is_some_and(|note| note.selected);
        if link.source_selected != source_selected || link.target_selected != target_selected {
            link.source_selected = source_selected;
            link.target_selected = target_selected;
            canvas_links.set_row_data(row, link);
        }
    }
}

pub(super) fn wire_infinite_canvas_callbacks(app: &AppWindow, store: Rc<RefCell<Store>>) {
    let state = app.global::<AppState>();
    let history = Rc::new(RefCell::new(CanvasController::default()));

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_show_canvas_node_info(move |id| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let store_ref = store.borrow();
            let Some(node) = store_ref
                .canvas_notes
                .iter()
                .find(|node| node.id == id.as_str())
            else {
                return;
            };
            let json = serde_json::to_string_pretty(&serde_json::json!({
                "id": node.id,
                "type": node.kind,
                "content": node.content,
                "width": node.width,
                "height": node.height,
                "x": node.x,
                "y": node.y,
                "parent_group_id": node.parent_group_id,
                "z_index": node.z_index,
                "image_path": node.image_path,
                "font_size": node.font_size,
                "status": "idle"
            }))
            .unwrap_or_else(|_| "{}".to_string());

            let state = app.global::<AppState>();
            state.set_canvas_node_info_id(node.id.clone().into());
            state.set_canvas_node_info_kind(node.kind.clone().into());
            state.set_canvas_node_info_x(node.x);
            state.set_canvas_node_info_y(node.y);
            state.set_canvas_node_info_width(node.width);
            state.set_canvas_node_info_height(node.height);
            state.set_canvas_node_info_status("idle".into());
            state.set_canvas_node_info_json(json.into());
            state.set_canvas_node_info_tab("info".into());
            state.set_canvas_node_info_open(true);
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        let history = history.clone();
        state.on_choose_canvas_node_image(move |id| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(source_path) = rfd::FileDialog::new()
                .add_filter("Images", &["png", "jpg", "jpeg", "webp"])
                .pick_file()
            else {
                return;
            };
            if load_image(&source_path).is_err() {
                let state = app.global::<AppState>();
                state.set_generation_status(
                    if state.get_language().as_str() == "en" {
                        "The selected file is not a supported image"
                    } else {
                        "所选文件不是受支持的图片"
                    }
                    .into(),
                );
                return;
            }
            let Ok(bytes) = fs::read(&source_path) else {
                return;
            };
            let upload_dir = app_data_dir().join("canvas").join("uploads");
            if fs::create_dir_all(&upload_dir).is_err() {
                return;
            }
            let destination = upload_dir.join(format!(
                "{}-{}.{}",
                id.as_str(),
                Uuid::new_v4(),
                image_extension(&bytes)
            ));
            if atomic_write_file(&destination, &bytes).is_err() {
                return;
            }

            let mut store_mut = store.borrow_mut();
            let Some(index) = store_mut
                .canvas_notes
                .iter()
                .position(|node| node.id == id.as_str() && node.kind == "image")
            else {
                let _ = fs::remove_file(&destination);
                return;
            };
            history.borrow_mut().record(canvas_snapshot(&store_mut));
            store_mut.canvas_notes[index].image_path = destination.display().to_string();
            persist_canvas(&app, &store_mut);
            sync_history_state(&app, &history.borrow());

            let state = app.global::<AppState>();
            state.set_generation_status(
                if state.get_language().as_str() == "en" {
                    "Image added to the node"
                } else {
                    "图片已添加到节点"
                }
                .into(),
            );
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        let history = history.clone();
        state.on_add_canvas_node(move |kind, center_x, center_y| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            let mut store_mut = store.borrow_mut();
            if store_mut.canvas_notes.len() >= MAX_CANVAS_NODES {
                show_canvas_capacity_status(&app);
                return;
            }

            let node_kind = match kind.as_str() {
                "image" | "group" => kind.to_string(),
                _ => "text".to_string(),
            };
            let (content, width, height) =
                canvas_node_defaults(&node_kind, state.get_language().as_str() == "en");
            let id = Uuid::new_v4().to_string();
            history.borrow_mut().record(canvas_snapshot(&store_mut));
            clear_selection(&mut store_mut.canvas_notes);
            store_mut.canvas_notes.push(CanvasNoteData {
                id: id.clone(),
                kind: node_kind,
                content,
                x: center_x - width / 2.0,
                y: center_y - height / 2.0,
                width,
                height,
                selected: true,
                ..CanvasNoteData::default()
            });
            persist_canvas(&app, &store_mut);
            sync_canvas_selection(&app, &store_mut);
            state.set_canvas_selected_id(id.into());
            sync_history_state(&app, &history.borrow());
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        let history = history.clone();
        state.on_adjust_canvas_text_font_size(move |id, delta| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut store_mut = store.borrow_mut();
            let Some(index) = store_mut
                .canvas_notes
                .iter()
                .position(|node| node.id == id.as_str() && node.kind == "text")
            else {
                return;
            };
            let next_font_size = (store_mut.canvas_notes[index].font_size + delta).clamp(8.0, 72.0);
            if next_font_size == store_mut.canvas_notes[index].font_size {
                return;
            }

            history.borrow_mut().record(canvas_snapshot(&store_mut));
            store_mut.canvas_notes[index].font_size = next_font_size;
            persist_canvas(&app, &store_mut);
            push_canvas_notes(&app, &store_mut);
            sync_history_state(&app, &history.borrow());
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        let history = history.clone();
        state.on_resize_canvas_group(move |id, width, height| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut store_mut = store.borrow_mut();
            let before = canvas_snapshot(&store_mut);
            if !resize_group(
                &mut store_mut.canvas_notes,
                id.as_str(),
                width.max(1.0),
                height.max(1.0),
            ) {
                return;
            }
            history.borrow_mut().record(before);
            persist_canvas(&app, &store_mut);
            sync_canvas_selection(&app, &store_mut);
            sync_history_state(&app, &history.borrow());
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_prepare_canvas_focus(move |viewport_width, viewport_height| {
            let Some(app) = app_weak.upgrade() else {
                return 100;
            };
            let store_ref = store.borrow();
            let mut ids = selected_ids(&store_ref.canvas_notes);
            if ids.is_empty() {
                ids.extend(store_ref.canvas_notes.iter().map(|note| note.id.clone()));
            }
            let Some(bounds) = selection_bounds(&store_ref.canvas_notes, &ids) else {
                return 100;
            };
            let state = app.global::<AppState>();
            state.set_canvas_focus_x(bounds.x);
            state.set_canvas_focus_y(bounds.y);
            state.set_canvas_focus_width(bounds.width);
            state.set_canvas_focus_height(bounds.height);
            let safe_width = bounds.width.max(1.0);
            let safe_height = bounds.height.max(1.0);
            ((viewport_width.max(1.0) / safe_width).min(viewport_height.max(1.0) / safe_height)
                * 84.0)
                .clamp(5.0, 500.0)
                .round() as i32
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        let history = history.clone();
        state.on_update_canvas_node(move |id, content, x, y| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut store_mut = store.borrow_mut();
            let Some(index) = store_mut
                .canvas_notes
                .iter()
                .position(|note| note.id == id.as_str())
            else {
                return;
            };
            let content = content.to_string();
            if store_mut.canvas_notes[index].content == content
                && store_mut.canvas_notes[index].x == x
                && store_mut.canvas_notes[index].y == y
            {
                return;
            }

            history.borrow_mut().record(canvas_snapshot(&store_mut));
            let node = &mut store_mut.canvas_notes[index];
            node.content = content;
            node.x = x;
            node.y = y;
            persist_canvas(&app, &store_mut);
            drop(store_mut);
            sync_history_state(&app, &history.borrow());
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_select_canvas_node(move |id, toggle| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut store_mut = store.borrow_mut();
            select_node(&mut store_mut.canvas_notes, id.as_str(), toggle);
            let selected = store_mut
                .canvas_notes
                .iter()
                .find(|note| note.id == id.as_str())
                .is_some_and(|note| note.selected);
            let state = app.global::<AppState>();
            state.set_canvas_selected_id(if selected { id } else { "".into() });
            state.set_canvas_selected_link_id("".into());
            sync_canvas_selection_rows(&app, &store_mut);
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_select_canvas_rect(move |x1, y1, x2, y2, additive| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut store_mut = store.borrow_mut();
            select_in_rect(
                &mut store_mut.canvas_notes,
                CanvasRect::normalized(x1, y1, x2, y2),
                additive,
            );
            let primary = store_mut
                .canvas_notes
                .iter()
                .find(|note| note.selected)
                .map(|note| note.id.clone())
                .unwrap_or_default();
            let state = app.global::<AppState>();
            state.set_canvas_selected_id(primary.into());
            state.set_canvas_selected_link_id("".into());
            sync_canvas_selection(&app, &store_mut);
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_clear_canvas_selection(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut store_mut = store.borrow_mut();
            clear_selection(&mut store_mut.canvas_notes);
            let state = app.global::<AppState>();
            state.set_canvas_selected_id("".into());
            state.set_canvas_selected_link_id("".into());
            sync_canvas_selection(&app, &store_mut);
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_select_all_canvas_nodes(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut store_mut = store.borrow_mut();
            for note in &mut store_mut.canvas_notes {
                note.selected = true;
            }
            let primary = store_mut
                .canvas_notes
                .first()
                .map(|note| note.id.clone())
                .unwrap_or_default();
            let state = app.global::<AppState>();
            state.set_canvas_selected_id(primary.into());
            state.set_canvas_selected_link_id("".into());
            sync_canvas_selection(&app, &store_mut);
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        let history = history.clone();
        state.on_move_canvas_selection(move |dx, dy| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            if dx == 0.0 && dy == 0.0 {
                return;
            }
            let mut store_mut = store.borrow_mut();
            let moved = expanded_selection_ids(&store_mut.canvas_notes);
            if moved.is_empty() {
                return;
            }
            history.borrow_mut().record(canvas_snapshot(&store_mut));
            move_selection(&mut store_mut.canvas_notes, dx, dy);
            assign_deepest_group(&mut store_mut.canvas_notes, &moved);
            fit_groups_to_children(&mut store_mut.canvas_notes);
            persist_canvas(&app, &store_mut);
            sync_canvas_selection(&app, &store_mut);
            sync_history_state(&app, &history.borrow());
        });
    }

    {
        let store = store.clone();
        let history = history.clone();
        state.on_copy_canvas_selection(move || {
            let store_ref = store.borrow();
            history
                .borrow_mut()
                .copy_selection(&store_ref.canvas_notes, &store_ref.canvas_links);
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        let history = history.clone();
        state.on_paste_canvas_selection(move |offset_x, offset_y| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let (notes, links) = history
                .borrow()
                .paste_clipboard(offset_x.max(0.0), offset_y.max(0.0));
            if notes.is_empty() {
                return;
            }
            let mut store_mut = store.borrow_mut();
            if store_mut.canvas_notes.len() + notes.len() > MAX_CANVAS_NODES
                || store_mut.canvas_links.len() + links.len() > MAX_CANVAS_LINKS
            {
                show_canvas_capacity_status(&app);
                return;
            }
            history.borrow_mut().record(canvas_snapshot(&store_mut));
            clear_selection(&mut store_mut.canvas_notes);
            let primary = notes
                .first()
                .map(|note| note.id.clone())
                .unwrap_or_default();
            store_mut.canvas_notes.extend(notes);
            store_mut.canvas_links.extend(links);
            fit_groups_to_children(&mut store_mut.canvas_notes);
            persist_canvas(&app, &store_mut);
            let state = app.global::<AppState>();
            state.set_canvas_selected_id(primary.into());
            state.set_canvas_selected_link_id("".into());
            sync_canvas_selection(&app, &store_mut);
            sync_history_state(&app, &history.borrow());
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        let history = history.clone();
        state.on_duplicate_canvas_selection(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let (notes, links) = {
                let store_ref = store.borrow();
                let mut controller = history.borrow_mut();
                controller.copy_selection(&store_ref.canvas_notes, &store_ref.canvas_links);
                controller.paste_clipboard(24.0, 24.0)
            };
            if notes.is_empty() {
                return;
            }
            let mut store_mut = store.borrow_mut();
            if store_mut.canvas_notes.len() + notes.len() > MAX_CANVAS_NODES
                || store_mut.canvas_links.len() + links.len() > MAX_CANVAS_LINKS
            {
                show_canvas_capacity_status(&app);
                return;
            }
            history.borrow_mut().record(canvas_snapshot(&store_mut));
            clear_selection(&mut store_mut.canvas_notes);
            let primary = notes
                .first()
                .map(|note| note.id.clone())
                .unwrap_or_default();
            store_mut.canvas_notes.extend(notes);
            store_mut.canvas_links.extend(links);
            fit_groups_to_children(&mut store_mut.canvas_notes);
            persist_canvas(&app, &store_mut);
            let state = app.global::<AppState>();
            state.set_canvas_selected_id(primary.into());
            state.set_canvas_selected_link_id("".into());
            sync_canvas_selection(&app, &store_mut);
            sync_history_state(&app, &history.borrow());
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        let history = history.clone();
        state.on_remove_canvas_selection(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut store_mut = store.borrow_mut();
            if selected_ids(&store_mut.canvas_notes).is_empty() {
                return;
            }
            history.borrow_mut().record(canvas_snapshot(&store_mut));
            let mut links = std::mem::take(&mut store_mut.canvas_links);
            remove_selection(&mut store_mut.canvas_notes, &mut links);
            store_mut.canvas_links = links;
            fit_groups_to_children(&mut store_mut.canvas_notes);
            persist_canvas(&app, &store_mut);
            let state = app.global::<AppState>();
            state.set_canvas_selected_id("".into());
            state.set_canvas_selected_link_id("".into());
            sync_canvas_selection(&app, &store_mut);
            sync_history_state(&app, &history.borrow());
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        let history = history.clone();
        state.on_group_canvas_selection(move |center_x, center_y| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut store_mut = store.borrow_mut();
            if store_mut.canvas_notes.len() >= MAX_CANVAS_NODES {
                show_canvas_capacity_status(&app);
                return;
            }
            history.borrow_mut().record(canvas_snapshot(&store_mut));
            let english = app.global::<AppState>().get_language().as_str() == "en";
            let id = if let Some(id) = group_selection(&mut store_mut.canvas_notes, english) {
                id
            } else {
                let (content, width, height) = canvas_node_defaults("group", english);
                clear_selection(&mut store_mut.canvas_notes);
                let id = Uuid::new_v4().to_string();
                store_mut.canvas_notes.push(CanvasNoteData {
                    id: id.clone(),
                    kind: "group".into(),
                    content,
                    x: center_x - width / 2.0,
                    y: center_y - height / 2.0,
                    width,
                    height,
                    selected: true,
                    ..CanvasNoteData::default()
                });
                id
            };
            fit_groups_to_children(&mut store_mut.canvas_notes);
            persist_canvas(&app, &store_mut);
            let state = app.global::<AppState>();
            state.set_canvas_selected_id(id.into());
            state.set_canvas_selected_link_id("".into());
            sync_canvas_selection(&app, &store_mut);
            sync_history_state(&app, &history.borrow());
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        let history = history.clone();
        state.on_ungroup_canvas_selection(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut store_mut = store.borrow_mut();
            if !store_mut
                .canvas_notes
                .iter()
                .any(|note| note.selected && note.kind == "group")
            {
                return;
            }
            history.borrow_mut().record(canvas_snapshot(&store_mut));
            ungroup_selection(&mut store_mut.canvas_notes);
            fit_groups_to_children(&mut store_mut.canvas_notes);
            persist_canvas(&app, &store_mut);
            let primary = store_mut
                .canvas_notes
                .iter()
                .find(|note| note.selected)
                .map(|note| note.id.clone())
                .unwrap_or_default();
            let state = app.global::<AppState>();
            state.set_canvas_selected_id(primary.into());
            state.set_canvas_selected_link_id("".into());
            sync_canvas_selection(&app, &store_mut);
            sync_history_state(&app, &history.borrow());
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        let history = history.clone();
        state.on_remove_canvas_node(move |id| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut store_mut = store.borrow_mut();
            if !store_mut
                .canvas_notes
                .iter()
                .any(|note| note.id == id.as_str())
            {
                return;
            }
            history.borrow_mut().record(canvas_snapshot(&store_mut));
            let removed_parent = store_mut
                .canvas_notes
                .iter()
                .find(|note| note.id == id.as_str() && note.kind == "group")
                .map(|note| note.parent_group_id.clone());
            if let Some(parent_id) = removed_parent {
                for child in store_mut
                    .canvas_notes
                    .iter_mut()
                    .filter(|note| note.parent_group_id == id.as_str())
                {
                    child.parent_group_id = parent_id.clone();
                }
            }
            store_mut.canvas_notes.retain(|note| note.id != id.as_str());
            fit_groups_to_children(&mut store_mut.canvas_notes);
            store_mut
                .canvas_links
                .retain(|link| link.source_id != id.as_str() && link.target_id != id.as_str());
            persist_canvas(&app, &store_mut);
            drop(store_mut);
            let state = app.global::<AppState>();
            if state.get_canvas_selected_id().as_str() == id.as_str() {
                state.set_canvas_selected_id("".into());
            }
            state.set_canvas_selected_link_id("".into());
            sync_history_state(&app, &history.borrow());
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_search_canvas_node_types(move |query| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let query = query.trim().to_lowercase();
            let options = [
                ("text", ["text", "文本", "prompt", "提示词"]),
                ("image", ["image", "图片", "picture", "图像"]),
            ];
            let results = options
                .into_iter()
                .filter(|(_, keywords)| {
                    query.is_empty()
                        || keywords
                            .iter()
                            .any(|keyword| keyword.to_lowercase().contains(&query))
                })
                .map(|(kind, _)| SharedString::from(kind))
                .collect::<Vec<_>>();
            app.global::<AppState>()
                .set_canvas_node_search_results(ModelRc::new(VecModel::from(results)));
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        let history = history.clone();
        state.on_add_connected_canvas_node(move |kind, source_id, x, y| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut store_mut = store.borrow_mut();
            if store_mut.canvas_notes.len() >= MAX_CANVAS_NODES
                || store_mut.canvas_links.len() >= MAX_CANVAS_LINKS
                || !store_mut
                    .canvas_notes
                    .iter()
                    .any(|note| note.id == source_id.as_str() && note.kind != "group")
            {
                show_canvas_capacity_status(&app);
                return;
            }
            let node_kind = if kind.as_str() == "image" {
                "image".to_string()
            } else {
                "text".to_string()
            };
            let state = app.global::<AppState>();
            let (content, width, height) =
                canvas_node_defaults(&node_kind, state.get_language().as_str() == "en");
            let id = Uuid::new_v4().to_string();
            let before = canvas_snapshot(&store_mut);
            clear_selection(&mut store_mut.canvas_notes);
            store_mut.canvas_notes.push(CanvasNoteData {
                id: id.clone(),
                kind: node_kind,
                content,
                x,
                y,
                width,
                height,
                selected: true,
                ..CanvasNoteData::default()
            });
            let CanvasConnectResult::Connected { link_id, .. } =
                connect_nodes(&mut store_mut.canvas_links, source_id.as_str(), &id)
            else {
                store_mut.canvas_notes.pop();
                return;
            };
            history.borrow_mut().record(before);
            persist_canvas(&app, &store_mut);
            sync_canvas_selection(&app, &store_mut);
            state.set_canvas_selected_id(id.into());
            state.set_canvas_selected_link_id(link_id.into());
            sync_history_state(&app, &history.borrow());
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_preview_canvas_link_target(move |source_id, x, y, tolerance| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let store_ref = store.borrow();
            let target_id =
                target_at_input(&store_ref, source_id.as_str(), x, y, tolerance.max(8.0))
                    .unwrap_or_default();
            let valid = !target_id.is_empty()
                && connection_allowed(
                    &store_ref.canvas_links,
                    source_id.as_str(),
                    target_id.as_str(),
                );
            let state = app.global::<AppState>();
            state.set_canvas_link_hover_target_id(target_id.into());
            state.set_canvas_link_hover_valid(valid);
        });
    }

    {
        let store = store.clone();
        state.on_canvas_input_link(move |target_id| {
            store
                .borrow()
                .canvas_links
                .iter()
                .find(|link| link.target_id == target_id.as_str())
                .map(|link| link.id.clone())
                .unwrap_or_default()
                .into()
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        let history = history.clone();
        state.on_finish_canvas_link(move |source_id, x, y, tolerance| {
            let Some(app) = app_weak.upgrade() else {
                return "rejected".into();
            };
            let mut store_mut = store.borrow_mut();
            if !store_mut
                .canvas_notes
                .iter()
                .any(|note| note.id == source_id.as_str() && note.kind != "group")
            {
                return "rejected".into();
            }
            let Some(target_id) =
                target_at_input(&store_mut, source_id.as_str(), x, y, tolerance.max(8.0))
            else {
                return "empty".into();
            };
            let replacing = store_mut
                .canvas_links
                .iter()
                .any(|link| link.target_id == target_id);
            if !replacing && store_mut.canvas_links.len() >= MAX_CANVAS_LINKS {
                show_canvas_capacity_status(&app);
                return "rejected".into();
            }

            let before = canvas_snapshot(&store_mut);
            let CanvasConnectResult::Connected {
                link_id, target_id, ..
            } = connect_nodes(
                &mut store_mut.canvas_links,
                source_id.as_str(),
                target_id.as_str(),
            )
            else {
                return "rejected".into();
            };
            history.borrow_mut().record(before);
            persist_canvas(&app, &store_mut);
            let state = app.global::<AppState>();
            state.set_canvas_selected_id(target_id.into());
            state.set_canvas_selected_link_id(link_id.into());
            state.set_generation_status(
                if state.get_language().as_str() == "en" {
                    "Connected. Upstream content will be used during generation."
                } else {
                    "连接成功，生成时将自动使用上游节点内容。"
                }
                .into(),
            );
            sync_history_state(&app, &history.borrow());
            "connected".into()
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        let history = history.clone();
        state.on_finish_canvas_reconnect(move |target_id, x, y, tolerance| {
            let Some(app) = app_weak.upgrade() else {
                return "rejected".into();
            };
            let mut store_mut = store.borrow_mut();
            let Some(source_id) =
                source_at_output(&store_mut, target_id.as_str(), x, y, tolerance.max(8.0))
            else {
                return "rejected".into();
            };
            let before = canvas_snapshot(&store_mut);
            let CanvasConnectResult::Connected {
                link_id, target_id, ..
            } = connect_nodes(
                &mut store_mut.canvas_links,
                source_id.as_str(),
                target_id.as_str(),
            )
            else {
                return "rejected".into();
            };
            history.borrow_mut().record(before);
            persist_canvas(&app, &store_mut);
            let state = app.global::<AppState>();
            state.set_canvas_selected_id(target_id.into());
            state.set_canvas_selected_link_id(link_id.into());
            sync_history_state(&app, &history.borrow());
            "connected".into()
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        let history = history.clone();
        state.on_remove_canvas_link(move |id| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut store_mut = store.borrow_mut();
            if !store_mut
                .canvas_links
                .iter()
                .any(|link| link.id == id.as_str())
            {
                return;
            }
            history.borrow_mut().record(canvas_snapshot(&store_mut));
            store_mut.canvas_links.retain(|link| link.id != id.as_str());
            persist_canvas(&app, &store_mut);
            drop(store_mut);
            let state = app.global::<AppState>();
            if state.get_canvas_selected_link_id().as_str() == id.as_str() {
                state.set_canvas_selected_link_id("".into());
            }
            sync_history_state(&app, &history.borrow());
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        let history = history.clone();
        state.on_undo_canvas(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut store_mut = store.borrow_mut();
            let Some(previous) = history.borrow_mut().undo(canvas_snapshot(&store_mut)) else {
                return;
            };
            restore_canvas_snapshot(&mut store_mut, previous);
            persist_canvas(&app, &store_mut);
            drop(store_mut);
            app.global::<AppState>().set_canvas_selected_id("".into());
            app.global::<AppState>()
                .set_canvas_selected_link_id("".into());
            sync_history_state(&app, &history.borrow());
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        let history = history.clone();
        state.on_redo_canvas(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut store_mut = store.borrow_mut();
            let Some(next) = history.borrow_mut().redo(canvas_snapshot(&store_mut)) else {
                return;
            };
            restore_canvas_snapshot(&mut store_mut, next);
            persist_canvas(&app, &store_mut);
            drop(store_mut);
            app.global::<AppState>().set_canvas_selected_id("".into());
            app.global::<AppState>()
                .set_canvas_selected_link_id("".into());
            sync_history_state(&app, &history.borrow());
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str) -> CanvasNoteData {
        CanvasNoteData {
            id: id.to_string(),
            kind: "text".to_string(),
            content: id.to_string(),
            x: 0.0,
            y: 0.0,
            width: 320.0,
            height: 210.0,
            ..CanvasNoteData::default()
        }
    }

    #[test]
    fn canvas_history_round_trips_undo_and_redo() {
        let mut history = CanvasController::default();
        history.record(CanvasSnapshot {
            notes: vec![node("before")],
            links: Vec::new(),
        });
        let previous = history
            .undo(CanvasSnapshot {
                notes: vec![node("after")],
                links: Vec::new(),
            })
            .expect("undo state");
        assert_eq!(previous.notes, vec![node("before")]);
        let next = history.redo(previous).expect("redo state");
        assert_eq!(next.notes, vec![node("after")]);
    }

    #[test]
    fn legacy_canvas_notes_receive_text_node_defaults() {
        let legacy = r#"{"id":"legacy","content":"old note","x":12.0,"y":24.0}"#;
        let parsed: CanvasNoteData = serde_json::from_str(legacy).expect("legacy canvas note");

        assert_eq!(parsed.kind, "text");
        assert_eq!(parsed.width, 280.0);
        assert_eq!(parsed.height, 176.0);
        assert_eq!(parsed.font_size, 12.0);
        assert_eq!(parsed.content, "old note");
    }

    #[test]
    fn canvas_links_reject_cycles_and_find_the_nearest_input() {
        let store = Store {
            canvas_notes: vec![
                node("source"),
                CanvasNoteData {
                    id: "target".to_string(),
                    x: 400.0,
                    ..node("target")
                },
            ],
            ..Store::default()
        };
        assert_eq!(
            target_at_input(&store, "source", 404.0, 105.0, 24.0).as_deref(),
            Some("target")
        );
        let links = vec![CanvasLinkData {
            id: "link".to_string(),
            source_id: "source".to_string(),
            target_id: "target".to_string(),
        }];
        assert!(link_reaches(&links, "source", "target"));
        assert!(!link_reaches(&links, "target", "source"));
    }
}
