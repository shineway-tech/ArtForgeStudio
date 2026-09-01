use super::*;
use std::hash::{Hash, Hasher};

pub(super) const MAX_CANVAS_NODES: usize = 200;
pub(super) const MAX_CANVAS_LINKS: usize = 400;
const MAX_CANVAS_SPLIT_AXIS: u32 = 64;

struct PreparedCanvasImage {
    path: String,
    width: f32,
    height: f32,
}

#[derive(Clone, Debug)]
struct CanvasSplitSource {
    id: String,
    image_path: String,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

#[derive(Clone, Debug)]
struct CanvasSplitTile {
    path: String,
    row: u32,
    column: u32,
}

type CanvasSplitOutcome = std::result::Result<Vec<CanvasSplitTile>, String>;

#[derive(Clone, Debug)]
struct CanvasExtractedElement {
    path: String,
    width: u32,
    height: u32,
}

type CanvasExtractionOutcome = std::result::Result<Vec<CanvasExtractedElement>, String>;

enum CanvasSystemClipboard {
    Image {
        fingerprint: u64,
        width: u32,
        height: u32,
        bytes: Vec<u8>,
    },
    Text {
        fingerprint: u64,
        text: String,
    },
}

impl CanvasSystemClipboard {
    fn fingerprint(&self) -> u64 {
        match self {
            Self::Image { fingerprint, .. } | Self::Text { fingerprint, .. } => *fingerprint,
        }
    }
}

fn read_canvas_system_clipboard() -> Option<CanvasSystemClipboard> {
    let mut clipboard = arboard::Clipboard::new().ok()?;
    if let Ok(image) = clipboard.get_image() {
        let width = image.width as u32;
        let height = image.height as u32;
        let bytes = image.bytes.into_owned();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        "image".hash(&mut hasher);
        width.hash(&mut hasher);
        height.hash(&mut hasher);
        bytes.hash(&mut hasher);
        return Some(CanvasSystemClipboard::Image {
            fingerprint: hasher.finish(),
            width,
            height,
            bytes,
        });
    }

    let text = clipboard.get_text().ok()?;
    if text.trim().is_empty() {
        return None;
    }
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    "text".hash(&mut hasher);
    text.hash(&mut hasher);
    Some(CanvasSystemClipboard::Text {
        fingerprint: hasher.finish(),
        text,
    })
}

fn persist_canvas_clipboard_image(
    width: u32,
    height: u32,
    bytes: Vec<u8>,
) -> Option<PreparedCanvasImage> {
    let rgba = image::RgbaImage::from_raw(width, height, bytes)?;
    let encoded = encode_png_rgba(&rgba, width, height).ok()?;
    let upload_dir = app_data_dir().join("canvas").join("uploads");
    if !ensure_managed_subdirectory(&upload_dir) {
        return None;
    }
    let destination = upload_dir.join(format!("pasted-{}.png", Uuid::new_v4()));
    atomic_write_file(&destination, &encoded).ok()?;
    Some(PreparedCanvasImage {
        path: destination.display().to_string(),
        width: width as f32,
        height: height as f32,
    })
}

fn terminal_remainder_span(total: u32, parts: u32, index: u32) -> (u32, u32) {
    let base = total / parts;
    let remainder = total % parts;
    let extras_start = parts - remainder;
    let extra_before = index.saturating_sub(extras_start);
    let start = base * index + extra_before;
    let size = base + u32::from(remainder > 0 && index >= extras_start);
    (start, size)
}

fn split_parts_from_lines(lines: u32) -> Option<u32> {
    lines.checked_add(1)
}

fn split_canvas_image_to_directory(
    source_path: &Path,
    output_dir: &Path,
    rows: u32,
    columns: u32,
) -> Result<Vec<CanvasSplitTile>> {
    let (decoded, _) = decode_image_file(source_path)?;
    let rgba = decoded.to_rgba8();
    let (image_width, image_height) = rgba.dimensions();
    if rows == 0 || columns == 0 || rows > image_height || columns > image_width {
        return Err(anyhow!("split grid exceeds image dimensions"));
    }
    ensure_managed_subdirectory(output_dir)
        .then_some(())
        .ok_or_else(|| anyhow!("unable to prepare the canvas split output directory"))?;

    let mut tiles = Vec::with_capacity((rows * columns) as usize);
    let result = (|| -> Result<()> {
        for row in 0..rows {
            let (top, tile_height) = terminal_remainder_span(image_height, rows, row);
            for column in 0..columns {
                let (left, tile_width) = terminal_remainder_span(image_width, columns, column);
                let tile =
                    image::imageops::crop_imm(&rgba, left, top, tile_width, tile_height).to_image();
                let bytes = encode_png_rgba(&tile, tile_width, tile_height)?;
                let path = output_dir.join(format!("tile-r{:02}-c{:02}.png", row + 1, column + 1));
                atomic_write_file(&path, &bytes)?;
                tiles.push(CanvasSplitTile {
                    path: path.display().to_string(),
                    row,
                    column,
                });
            }
        }
        Ok(())
    })();
    if let Err(error) = result {
        let _ = fs::remove_dir_all(output_dir);
        return Err(error);
    }
    Ok(tiles)
}

fn remove_canvas_split_tiles(tiles: &[CanvasSplitTile]) {
    let parent = tiles
        .first()
        .and_then(|tile| Path::new(&tile.path).parent())
        .map(Path::to_path_buf);
    for tile in tiles {
        let _ = fs::remove_file(&tile.path);
    }
    if let Some(parent) = parent {
        let _ = fs::remove_dir(&parent);
    }
}

fn extract_canvas_elements_to_directory(
    source_path: &Path,
    output_dir: &Path,
) -> Result<Vec<CanvasExtractedElement>> {
    let (decoded, _) = decode_image_file(source_path)?;
    let source = decoded.to_rgba8();
    let components = extract_ui_components(&source)?;
    ensure_managed_subdirectory(output_dir)
        .then_some(())
        .ok_or_else(|| anyhow!("unable to prepare the canvas extraction output directory"))?;

    let mut elements = Vec::with_capacity(components.len());
    let result = (|| -> Result<()> {
        for (index, component) in components.into_iter().enumerate() {
            let width = component.image.width();
            let height = component.image.height();
            let bytes = encode_png_rgba(&component.image, width, height)?;
            let path = output_dir.join(format!("element-{:02}.png", index + 1));
            atomic_write_file(&path, &bytes)?;
            elements.push(CanvasExtractedElement {
                path: path.display().to_string(),
                width,
                height,
            });
        }
        Ok(())
    })();
    if let Err(error) = result {
        let _ = fs::remove_dir_all(output_dir);
        return Err(error);
    }
    Ok(elements)
}

fn remove_canvas_extracted_elements(elements: &[CanvasExtractedElement]) {
    let parent = elements
        .first()
        .and_then(|element| Path::new(&element.path).parent())
        .map(Path::to_path_buf);
    for element in elements {
        let _ = fs::remove_file(&element.path);
    }
    if let Some(parent) = parent {
        let _ = fs::remove_dir(&parent);
    }
}

fn clear_canvas_extraction_loading(state: &AppState, source_id: &str) {
    if state.get_canvas_extraction_loading_node_id().as_str() == source_id {
        state.set_canvas_extraction_loading_node_id("".into());
    }
}

fn poll_canvas_element_extraction(
    app_weak: Weak<AppWindow>,
    store: Rc<RefCell<Store>>,
    history: Rc<RefCell<CanvasController>>,
    source: CanvasSplitSource,
    receiver: Rc<RefCell<Option<mpsc::Receiver<CanvasExtractionOutcome>>>>,
) {
    slint::Timer::single_shot(Duration::from_millis(80), move || {
        let outcome = {
            let mut slot = receiver.borrow_mut();
            let Some(rx) = slot.as_ref() else {
                return;
            };
            match rx.try_recv() {
                Ok(outcome) => {
                    slot.take();
                    Some(outcome)
                }
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    slot.take();
                    Some(Err("element extraction worker stopped unexpectedly".to_string()))
                }
            }
        };
        let Some(outcome) = outcome else {
            poll_canvas_element_extraction(app_weak, store, history, source, receiver);
            return;
        };
        let Some(app) = app_weak.upgrade() else {
            if let Ok(elements) = outcome {
                remove_canvas_extracted_elements(&elements);
            }
            return;
        };
        let state = app.global::<AppState>();
        let elements = match outcome {
            Ok(elements) => elements,
            Err(error) => {
                clear_canvas_extraction_loading(&state, &source.id);
                state.set_generation_status(
                    if state.get_language().as_str() == "en" {
                        format!("Unable to extract elements: {error}")
                    } else {
                        format!("提取元素失败：{error}")
                    }
                    .into(),
                );
                return;
            }
        };

        let mut store_mut = store.borrow_mut();
        let source_is_current = store_mut.canvas_notes.iter().any(|note| {
            note.id == source.id
                && note.image_path == source.image_path
                && matches!(note.kind.as_str(), "image" | "board-image")
        });
        if !source_is_current
            || store_mut.canvas_notes.len() + elements.len() > MAX_CANVAS_NODES
            || store_mut.canvas_links.len() + elements.len() > MAX_CANVAS_LINKS
        {
            remove_canvas_extracted_elements(&elements);
            clear_canvas_extraction_loading(&state, &source.id);
            if !source_is_current {
                state.set_generation_status(
                    if state.get_language().as_str() == "en" {
                        "The source image changed before extraction finished"
                    } else {
                        "提取完成前原图已被更换，请重新操作"
                    }
                    .into(),
                );
            } else {
                show_canvas_capacity_status(&app);
            }
            return;
        }

        history.borrow_mut().record(canvas_snapshot(&store_mut));
        clear_selection(&mut store_mut.canvas_notes);
        let next_z = store_mut
            .canvas_notes
            .iter()
            .map(|note| note.z_index)
            .max()
            .unwrap_or(0)
            + 1;
        let origin_x = source.x + source.width + 64.0;
        let origin_y = source.y;
        let cell_width = 220.0;
        let cell_height = 200.0;
        let gap = 16.0;
        let first_id = elements.first().map(|_| Uuid::new_v4().to_string());
        let mut created_ids = Vec::with_capacity(elements.len());

        for (index, element) in elements.into_iter().enumerate() {
            let id = if index == 0 {
                first_id
                    .clone()
                    .unwrap_or_else(|| Uuid::new_v4().to_string())
            } else {
                Uuid::new_v4().to_string()
            };
            let column = index % 4;
            let row = index / 4;
            let mut note = CanvasNoteData {
                id: id.clone(),
                kind: "board-image".to_string(),
                content: String::new(),
                width: 180.0,
                height: 180.0,
                parent_group_id: String::new(),
                z_index: next_z + index as i32,
                image_path: element.path,
                selected: index == 0,
                ..CanvasNoteData::default()
            };
            fit_image_node_to_intrinsic_aspect(
                &mut note,
                element.width as f32,
                element.height as f32,
            );
            note.x = origin_x + column as f32 * (cell_width + gap) + (cell_width - note.width) / 2.0;
            note.y = origin_y + row as f32 * (cell_height + gap) + (cell_height - note.height) / 2.0;
            store_mut.canvas_notes.push(note);
            created_ids.push(id);
        }
        for id in &created_ids {
            let _ = connect_nodes(&mut store_mut.canvas_links, &source.id, id);
        }
        persist_canvas(&app, &store_mut);
        sync_canvas_selection(&app, &store_mut);
        if let Some(first_id) = first_id {
            state.set_canvas_selected_id(first_id.into());
        }
        state.set_canvas_selected_link_id("".into());
        clear_canvas_extraction_loading(&state, &source.id);
        state.set_generation_status(
            if state.get_language().as_str() == "en" {
                format!("Extracted {} transparent PNG elements from the current image", created_ids.len())
            } else {
                format!("已从当前图片提取 {} 个透明 PNG 元素", created_ids.len())
            }
            .into(),
        );
        sync_history_state(&app, &history.borrow());
    });
}

fn clear_canvas_split_loading(state: &AppState, source_id: &str) {
    if state.get_canvas_split_loading_node_id().as_str() == source_id {
        state.set_canvas_split_loading_node_id("".into());
    }
}

fn poll_canvas_image_split(
    app_weak: Weak<AppWindow>,
    store: Rc<RefCell<Store>>,
    history: Rc<RefCell<CanvasController>>,
    source: CanvasSplitSource,
    rows: u32,
    columns: u32,
    receiver: Rc<RefCell<Option<mpsc::Receiver<CanvasSplitOutcome>>>>,
) {
    slint::Timer::single_shot(Duration::from_millis(80), move || {
        let outcome = {
            let mut slot = receiver.borrow_mut();
            let Some(rx) = slot.as_ref() else {
                return;
            };
            match rx.try_recv() {
                Ok(outcome) => {
                    slot.take();
                    Some(outcome)
                }
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    slot.take();
                    Some(Err("image split worker stopped unexpectedly".to_string()))
                }
            }
        };
        let Some(outcome) = outcome else {
            poll_canvas_image_split(app_weak, store, history, source, rows, columns, receiver);
            return;
        };
        let Some(app) = app_weak.upgrade() else {
            if let Ok(tiles) = outcome {
                remove_canvas_split_tiles(&tiles);
            }
            return;
        };
        let state = app.global::<AppState>();
        let tiles = match outcome {
            Ok(tiles) => tiles,
            Err(error) => {
                clear_canvas_split_loading(&state, &source.id);
                state.set_generation_status(
                    if state.get_language().as_str() == "en" {
                        format!("Unable to split image: {error}")
                    } else {
                        format!("图片分割失败：{error}")
                    }
                    .into(),
                );
                return;
            }
        };

        let mut store_mut = store.borrow_mut();
        let source_is_current = store_mut.canvas_notes.iter().any(|note| {
            note.id == source.id
                && note.image_path == source.image_path
                && matches!(note.kind.as_str(), "image" | "board-image")
        });
        if !source_is_current
            || store_mut.canvas_notes.len() + tiles.len() > MAX_CANVAS_NODES
            || store_mut.canvas_links.len() + tiles.len() > MAX_CANVAS_LINKS
        {
            remove_canvas_split_tiles(&tiles);
            clear_canvas_split_loading(&state, &source.id);
            if !source_is_current {
                state.set_generation_status(
                    if state.get_language().as_str() == "en" {
                        "The source image changed before splitting finished"
                    } else {
                        "分割完成前原图已被更换，请重新操作"
                    }
                    .into(),
                );
            } else {
                show_canvas_capacity_status(&app);
            }
            return;
        }

        history.borrow_mut().record(canvas_snapshot(&store_mut));
        clear_selection(&mut store_mut.canvas_notes);
        let next_z = store_mut
            .canvas_notes
            .iter()
            .map(|note| note.z_index)
            .max()
            .unwrap_or(0)
            + 1;
        let raw_tile_width = source.width / columns as f32;
        let raw_tile_height = source.height / rows as f32;
        let display_scale = (80.0 / raw_tile_width).max(80.0 / raw_tile_height).max(1.0);
        let tile_width = raw_tile_width * display_scale;
        let tile_height = raw_tile_height * display_scale;
        let gap = 16.0;
        let origin_x = source.x + source.width + 64.0;
        let origin_y = source.y;
        let first_id = tiles.first().map(|_| Uuid::new_v4().to_string());
        let mut created_ids = Vec::with_capacity(tiles.len());

        for (index, tile) in tiles.into_iter().enumerate() {
            let id = if index == 0 {
                first_id
                    .clone()
                    .unwrap_or_else(|| Uuid::new_v4().to_string())
            } else {
                Uuid::new_v4().to_string()
            };
            store_mut.canvas_notes.push(CanvasNoteData {
                id: id.clone(),
                kind: "board-image".to_string(),
                content: String::new(),
                x: origin_x + tile.column as f32 * (tile_width + gap),
                y: origin_y + tile.row as f32 * (tile_height + gap),
                width: tile_width,
                height: tile_height,
                parent_group_id: String::new(),
                z_index: next_z + index as i32,
                image_path: tile.path,
                selected: index == 0,
                ..CanvasNoteData::default()
            });
            created_ids.push(id);
        }
        for id in &created_ids {
            let _ = connect_nodes(&mut store_mut.canvas_links, &source.id, id);
        }
        persist_canvas(&app, &store_mut);
        sync_canvas_selection(&app, &store_mut);
        if let Some(first_id) = first_id {
            state.set_canvas_selected_id(first_id.into());
        }
        state.set_canvas_selected_link_id("".into());
        clear_canvas_split_loading(&state, &source.id);
        state.set_generation_status(
            if state.get_language().as_str() == "en" {
                format!("Split evenly into {rows} rows × {columns} columns")
            } else {
                format!(
                    "已平均分割为 {rows} 行 × {columns} 列，共 {} 张",
                    rows * columns
                )
            }
            .into(),
        );
        sync_history_state(&app, &history.borrow());
    });
}

fn pick_canvas_image(app: &AppWindow, node_id: &str) -> Option<PreparedCanvasImage> {
    let source_path = rfd::FileDialog::new()
        .add_filter("Images", crate::image_formats::picker_image_extensions())
        .pick_file()?;
    let (source_width, source_height) = match inspect_image_dimensions(&source_path) {
        Ok(size) => size,
        Err(_) => {
            let state = app.global::<AppState>();
            state.set_generation_status(
                if state.get_language().as_str() == "en" {
                    "The selected file is not a supported image"
                } else {
                    "所选文件不是受支持的图片"
                }
                .into(),
            );
            return None;
        }
    };
    let bytes = match fs::read(&source_path) {
        Ok(bytes) => bytes,
        Err(_) => {
            let state = app.global::<AppState>();
            state.set_generation_status(
                if state.get_language().as_str() == "en" {
                    "Unable to read the selected image"
                } else {
                    "无法读取所选图片"
                }
                .into(),
            );
            return None;
        }
    };
    let upload_dir = app_data_dir().join("canvas").join("uploads");
    if !ensure_managed_subdirectory(&upload_dir) {
        return None;
    }
    let destination = upload_dir.join(format!(
        "{}-{}.{}",
        node_id,
        Uuid::new_v4(),
        image_extension(&bytes)
    ));
    if atomic_write_file(&destination, &bytes).is_err() {
        return None;
    }
    Some(PreparedCanvasImage {
        path: destination.display().to_string(),
        width: source_width as f32,
        height: source_height as f32,
    })
}

pub(super) fn import_viewer_image_to_canvas(
    app: &AppWindow,
    store: &mut Store,
    source_path: &Path,
) -> Result<String> {
    if store.canvas_notes.len() >= MAX_CANVAS_NODES {
        return Err(anyhow!("canvas node limit reached"));
    }

    let (source_width, source_height) = inspect_image_dimensions(source_path)?;
    let bytes = fs::read(source_path)
        .with_context(|| format!("unable to read {}", source_path.display()))?;
    let upload_dir = app_data_dir().join("canvas").join("uploads");
    if !ensure_managed_subdirectory(&upload_dir) {
        return Err(anyhow!("unable to create the canvas upload directory"));
    }

    let id = Uuid::new_v4().to_string();
    let destination = upload_dir.join(format!(
        "imported-{}-{}.{}",
        id,
        Uuid::new_v4(),
        image_extension(&bytes)
    ));
    atomic_write_file(&destination, &bytes)?;

    let (_, width, height) = canvas_node_defaults("image", false);
    let mut note = CanvasNoteData {
        id: id.clone(),
        kind: "board-image".into(),
        width,
        height,
        image_path: destination.display().to_string(),
        selected: true,
        ..CanvasNoteData::default()
    };
    fit_image_node_to_intrinsic_aspect(
        &mut note,
        source_width as f32,
        source_height as f32,
    );

    let mut anchor_ids = selected_ids(&store.canvas_notes);
    if anchor_ids.is_empty() {
        anchor_ids.extend(store.canvas_notes.iter().map(|item| item.id.clone()));
    }
    if let Some(bounds) = selection_bounds(&store.canvas_notes, &anchor_ids) {
        note.x = bounds.x + bounds.width + 64.0;
        note.y = bounds.y;
    }

    clear_selection(&mut store.canvas_notes);
    store.canvas_notes.push(note);
    persist_canvas(app, store);
    sync_canvas_selection(app, store);

    let state = app.global::<AppState>();
    state.set_canvas_selected_id(id.clone().into());
    state.set_canvas_focus_request(state.get_canvas_focus_request().saturating_add(1));
    Ok(id)
}

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
    sync_canvas_selection_rows(app, store);
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

pub(super) fn switch_canvas_workspace(
    store: &mut Store,
    current_prompt: &str,
    target_workspace_id: &str,
) -> String {
    let current_workspace_id = normalize_canvas_workspace_id(&store.active_canvas_workspace_id);
    store.canvas_workspaces.insert(
        current_workspace_id,
        CanvasWorkspaceData {
            notes: store.canvas_notes.clone(),
            links: store.canvas_links.clone(),
            prompt: current_prompt.to_string(),
            references: store.canvas_references.clone(),
        },
    );

    let target_workspace_id = normalize_canvas_workspace_id(target_workspace_id);
    let target = store
        .canvas_workspaces
        .get(&target_workspace_id)
        .cloned()
        .unwrap_or_default();
    store.active_canvas_workspace_id = target_workspace_id.clone();
    store.canvas_notes = target.notes;
    store.canvas_links = target.links;
    store.canvas_references = target.references;
    clear_selection(&mut store.canvas_notes);
    store.canvas_workspaces.insert(
        target_workspace_id,
        CanvasWorkspaceData {
            notes: store.canvas_notes.clone(),
            links: store.canvas_links.clone(),
            prompt: target.prompt.clone(),
            references: store.canvas_references.clone(),
        },
    );
    target.prompt
}

pub(super) fn wire_infinite_canvas_callbacks(app: &AppWindow, context: AppContext) {
    let state = app.global::<AppState>();
    let store = context.store.clone();
    let history = context.canvas_history.clone();

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        let history = history.clone();
        state.on_open_canvas_workspace(move |workspace_id| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            let prompt = {
                let mut store = store.borrow_mut();
                let prompt = switch_canvas_workspace(
                    &mut store,
                    state.get_canvas_workflow_prompt().as_str(),
                    workspace_id.as_str(),
                );
                push_canvas_notes(&app, &store);
                push_canvas_references(&app, &store);
                prompt
            };
            *history.borrow_mut() = CanvasController::default();
            state.set_canvas_workflow_prompt(prompt.into());
            save_local_store(&app, &store.borrow());
            state.set_canvas_selected_id("".into());
            state.set_canvas_selected_link_id("".into());
            state.set_canvas_selected_count(0);
            state.set_canvas_node_info_open(false);
            state.set_canvas_group_name_dialog_open(false);
            state.set_canvas_can_undo(false);
            state.set_canvas_can_redo(false);
            state.set_canvas_workspace_switch_request(
                state.get_canvas_workspace_switch_request().saturating_add(1),
            );
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_generate_canvas_node(move |source_node_id, prompt| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            start_canvas_generation(
                &app,
                context.clone(),
                source_node_id.to_string(),
                prompt.to_string(),
            );
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        let history = history.clone();
        state.on_extract_canvas_ui_elements(move |source_node_id| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            if !state.get_canvas_extraction_loading_node_id().is_empty()
                || !state.get_canvas_split_loading_node_id().is_empty()
            {
                state.set_generation_status(
                    if state.get_language().as_str() == "en" {
                        "Wait for the current canvas image operation to finish"
                    } else {
                        "请等待当前画布图片操作完成"
                    }
                    .into(),
                );
                return;
            }
            let source = {
                let store_ref = store.borrow();
                if store_ref.canvas_notes.len() + MAX_EXTRACTED_COMPONENTS > MAX_CANVAS_NODES
                    || store_ref.canvas_links.len() + MAX_EXTRACTED_COMPONENTS > MAX_CANVAS_LINKS
                {
                    show_canvas_capacity_status(&app);
                    return;
                }
                let Some(note) = store_ref.canvas_notes.iter().find(|note| {
                    note.id == source_node_id.as_str()
                        && matches!(note.kind.as_str(), "image" | "board-image")
                        && !note.image_path.trim().is_empty()
                        && Path::new(&note.image_path).is_file()
                }) else {
                    state.set_generation_status(
                        if state.get_language().as_str() == "en" {
                            "Select an uploaded canvas image before extracting elements"
                        } else {
                            "请先选择已上传的画布图片"
                        }
                        .into(),
                    );
                    return;
                };
                CanvasSplitSource {
                    id: note.id.clone(),
                    image_path: note.image_path.clone(),
                    x: note.x,
                    y: note.y,
                    width: note.width,
                    height: note.height,
                }
            };

            state.set_generation_status(
                if state.get_language().as_str() == "en" {
                    "Extracting transparent PNG elements from the current image..."
                } else {
                    "正在从当前图片提取透明 PNG 元素..."
                }
                .into(),
            );
            state.set_canvas_extraction_loading_node_id(source.id.clone().into());
            let output_dir = app_data_dir()
                .join("canvas")
                .join("ui-extractions")
                .join(Uuid::new_v4().to_string());
            let source_path = PathBuf::from(&source.image_path);
            let (sender, receiver) = mpsc::channel::<CanvasExtractionOutcome>();
            std::thread::spawn(move || {
                let outcome = extract_canvas_elements_to_directory(&source_path, &output_dir)
                    .map_err(|error| error.to_string());
                let _ = sender.send(outcome);
            });
            poll_canvas_element_extraction(
                app.as_weak(),
                store.clone(),
                history.clone(),
                source,
                Rc::new(RefCell::new(Some(receiver))),
            );
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        let history = history.clone();
        state.on_split_canvas_image(move |source_node_id, rows, columns| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            if !state.get_canvas_split_loading_node_id().is_empty()
                || !state.get_canvas_extraction_loading_node_id().is_empty()
            {
                state.set_generation_status(
                    if state.get_language().as_str() == "en" {
                        "Wait for the current image split to finish"
                    } else {
                        "请等待当前图片分割完成"
                    }
                    .into(),
                );
                return;
            }
            let horizontal_lines = rows.trim().parse::<u32>();
            let vertical_lines = columns.trim().parse::<u32>();
            let (Ok(horizontal_lines), Ok(vertical_lines)) =
                (horizontal_lines, vertical_lines)
            else {
                state.set_generation_status(
                    if state.get_language().as_str() == "en" {
                        "Enter positive whole numbers for horizontal and vertical split lines"
                    } else {
                        "横向和纵向分割线数量请输入正整数"
                    }
                    .into(),
                );
                return;
            };
            if horizontal_lines == 0
                || vertical_lines == 0
                || horizontal_lines > MAX_CANVAS_SPLIT_AXIS
                || vertical_lines > MAX_CANVAS_SPLIT_AXIS
            {
                state.set_generation_status(
                    if state.get_language().as_str() == "en" {
                        "Horizontal and vertical split lines must each be between 1 and 64"
                    } else {
                        "横向和纵向分割线数量均需在 1 到 64 之间"
                    }
                    .into(),
                );
                return;
            }
            let Some(rows) = split_parts_from_lines(horizontal_lines) else {
                show_canvas_capacity_status(&app);
                return;
            };
            let Some(columns) = split_parts_from_lines(vertical_lines) else {
                show_canvas_capacity_status(&app);
                return;
            };
            let tile_count = match rows.checked_mul(columns) {
                Some(count) => count as usize,
                None => {
                    show_canvas_capacity_status(&app);
                    return;
                }
            };
            let source = {
                let store_ref = store.borrow();
                if store_ref.canvas_notes.len() + tile_count > MAX_CANVAS_NODES
                    || store_ref.canvas_links.len() + tile_count > MAX_CANVAS_LINKS
                {
                    show_canvas_capacity_status(&app);
                    return;
                }
                let Some(note) = store_ref.canvas_notes.iter().find(|note| {
                    note.id == source_node_id.as_str()
                        && matches!(note.kind.as_str(), "image" | "board-image")
                        && !note.image_path.trim().is_empty()
                }) else {
                    state.set_generation_status(
                        if state.get_language().as_str() == "en" {
                            "Select an uploaded image before splitting"
                        } else {
                            "请先选择已上传图片的节点"
                        }
                        .into(),
                    );
                    return;
                };
                CanvasSplitSource {
                    id: note.id.clone(),
                    image_path: note.image_path.clone(),
                    x: note.x,
                    y: note.y,
                    width: note.width,
                    height: note.height,
                }
            };
            let Ok((image_width, image_height)) =
                inspect_image_dimensions(Path::new(&source.image_path))
            else {
                state.set_generation_status(
                    if state.get_language().as_str() == "en" {
                        "Unable to read the source image"
                    } else {
                        "无法读取原图"
                    }
                    .into(),
                );
                return;
            };
            if rows > image_height || columns > image_width {
                state.set_generation_status(
                    if state.get_language().as_str() == "en" {
                        "The requested split lines exceed the source image pixel size"
                    } else {
                        "分割线数量不能超过原图像素尺寸"
                    }
                    .into(),
                );
                return;
            }

            state.set_generation_status(
                if state.get_language().as_str() == "en" {
                    "Splitting image locally..."
                } else {
                    "正在本地平均分割图片..."
                }
                .into(),
            );
            state.set_canvas_split_loading_node_id(source.id.clone().into());
            let output_dir = app_data_dir()
                .join("canvas")
                .join("splits")
                .join(Uuid::new_v4().to_string());
            let source_path = PathBuf::from(&source.image_path);
            let (sender, receiver) = mpsc::channel::<CanvasSplitOutcome>();
            std::thread::spawn(move || {
                let outcome =
                    split_canvas_image_to_directory(&source_path, &output_dir, rows, columns)
                        .map_err(|error| error.to_string());
                let _ = sender.send(outcome);
            });
            poll_canvas_image_split(
                app.as_weak(),
                store.clone(),
                history.clone(),
                source,
                rows,
                columns,
                Rc::new(RefCell::new(Some(receiver))),
            );
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_save_canvas_image(move |node_id| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            let source = {
                let store_ref = store.borrow();
                store_ref
                    .canvas_notes
                    .iter()
                    .find(|note| {
                        note.id == node_id.as_str()
                            && matches!(note.kind.as_str(), "image" | "board-image")
                            && !note.image_path.trim().is_empty()
                    })
                    .map(|note| PathBuf::from(&note.image_path))
            };
            let Some(source) = source.filter(|path| path.is_file()) else {
                state.set_generation_status(
                    if state.get_language().as_str() == "en" {
                        "The canvas image is no longer available"
                    } else {
                        "画布图片文件已不存在"
                    }
                    .into(),
                );
                return;
            };
            let default_name = source
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("canvas-image.png");
            let Some(destination) = rfd::FileDialog::new()
                .add_filter(
                    "Images",
                    crate::image_formats::picker_image_extensions(),
                )
                .set_file_name(default_name)
                .save_file()
            else {
                return;
            };
            let result = if destination == source {
                Ok(())
            } else {
                fs::read(&source).and_then(|bytes| {
                    atomic_write_file(&destination, &bytes)
                        .map_err(|error| std::io::Error::other(error.to_string()))
                })
            };
            state.set_generation_status(
                match result {
                    Ok(()) if state.get_language().as_str() == "en" => {
                        "Canvas image saved".to_string()
                    }
                    Ok(()) => "画布图片已保存到本地".to_string(),
                    Err(error) if state.get_language().as_str() == "en" => {
                        format!("Unable to save the canvas image: {error}")
                    }
                    Err(error) => format!("保存画布图片失败：{error}"),
                }
                .into(),
            );
        });
    }

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
            if !store
                .borrow()
                .canvas_notes
                .iter()
                .any(|node| node.id == id.as_str() && node.kind == "image")
            {
                return;
            }
            let Some(image) = pick_canvas_image(&app, id.as_str()) else {
                return;
            };

            let mut store_mut = store.borrow_mut();
            let Some(index) = store_mut
                .canvas_notes
                .iter()
                .position(|node| node.id == id.as_str() && node.kind == "image")
            else {
                let _ = fs::remove_file(&image.path);
                return;
            };
            history.borrow_mut().record(canvas_snapshot(&store_mut));
            store_mut.canvas_notes[index].image_path = image.path;
            fit_image_node_to_intrinsic_aspect(
                &mut store_mut.canvas_notes[index],
                image.width,
                image.height,
            );
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
        state.on_add_canvas_uploaded_image(move |center_x, center_y| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            if store.borrow().canvas_notes.len() >= MAX_CANVAS_NODES {
                show_canvas_capacity_status(&app);
                return;
            }
            let id = Uuid::new_v4().to_string();
            let Some(image) = pick_canvas_image(&app, &id) else {
                return;
            };

            let mut store_mut = store.borrow_mut();
            if store_mut.canvas_notes.len() >= MAX_CANVAS_NODES {
                let _ = fs::remove_file(&image.path);
                show_canvas_capacity_status(&app);
                return;
            }
            let (_, width, height) = canvas_node_defaults("image", false);
            let mut note = CanvasNoteData {
                id: id.clone(),
                kind: "board-image".into(),
                x: center_x - width / 2.0,
                y: center_y - height / 2.0,
                width,
                height,
                image_path: image.path,
                selected: true,
                ..CanvasNoteData::default()
            };
            fit_image_node_to_intrinsic_aspect(&mut note, image.width, image.height);

            history.borrow_mut().record(canvas_snapshot(&store_mut));
            clear_selection(&mut store_mut.canvas_notes);
            store_mut.canvas_notes.push(note);
            persist_canvas(&app, &store_mut);
            sync_canvas_selection(&app, &store_mut);
            app.global::<AppState>().set_canvas_selected_id(id.into());
            sync_history_state(&app, &history.borrow());
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        let history = history.clone();
        state.on_create_canvas_generation_source(move |prompt, center_x, center_y| {
            let Some(app) = app_weak.upgrade() else {
                return "".into();
            };
            let prompt = prompt.trim().to_string();
            if prompt.is_empty() {
                return "".into();
            }

            let state = app.global::<AppState>();
            let mut store_mut = store.borrow_mut();
            if store_mut.canvas_notes.len() >= MAX_CANVAS_NODES {
                show_canvas_capacity_status(&app);
                return "".into();
            }

            let (_, width, height) =
                canvas_node_defaults("image", state.get_language().as_str() == "en");
            let id = Uuid::new_v4().to_string();
            history.borrow_mut().record(canvas_snapshot(&store_mut));
            clear_selection(&mut store_mut.canvas_notes);
            store_mut.canvas_notes.push(CanvasNoteData {
                id: id.clone(),
                kind: "image".to_string(),
                content: prompt,
                x: center_x - width / 2.0,
                y: center_y - height / 2.0,
                width,
                height,
                selected: true,
                ..CanvasNoteData::default()
            });
            persist_canvas(&app, &store_mut);
            sync_canvas_selection(&app, &store_mut);
            state.set_canvas_selected_id(id.clone().into());
            sync_history_state(&app, &history.borrow());
            id.into()
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
            let (mut content, width, height) =
                canvas_node_defaults(&node_kind, state.get_language().as_str() == "en");
            if node_kind == "group" {
                content = next_group_name(
                    &store_mut.canvas_notes,
                    state.get_language().as_str() == "en",
                );
            }
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
            sync_history_state(&app, &history.borrow());
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        let history = history.clone();
        state.on_rename_canvas_group(move |id, name| {
            let Some(app) = app_weak.upgrade() else {
                return false;
            };
            let name = name.trim();
            if name.is_empty() {
                app.global::<AppState>()
                    .set_generation_status("分组名称不能为空".into());
                return false;
            }
            let mut store_mut = store.borrow_mut();
            let Some(index) = store_mut
                .canvas_notes
                .iter()
                .position(|note| note.id == id.as_str() && note.kind == "group")
            else {
                return false;
            };
            if store_mut.canvas_notes[index].content == name {
                return true;
            }
            history.borrow_mut().record(canvas_snapshot(&store_mut));
            store_mut.canvas_notes[index].content = name.to_string();
            persist_canvas(&app, &store_mut);
            sync_history_state(&app, &history.borrow());
            true
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
        let history = history.clone();
        state.on_resize_canvas_image_node(move |id, width, height| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut store_mut = store.borrow_mut();
            let before = canvas_snapshot(&store_mut);
            if !resize_image_node_proportionally(
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
            let system_fingerprint =
                read_canvas_system_clipboard().map(|content| content.fingerprint());
            let store_ref = store.borrow();
            let mut controller = history.borrow_mut();
            controller.copy_selection(&store_ref.canvas_notes, &store_ref.canvas_links);
            controller.remember_system_clipboard(system_fingerprint);
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
        state.on_paste_canvas_content(move |center_x, center_y| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let system_clipboard = read_canvas_system_clipboard();
            let system_fingerprint = system_clipboard
                .as_ref()
                .map(CanvasSystemClipboard::fingerprint);
            let paste_system_clipboard = history
                .borrow()
                .should_paste_system_clipboard(system_fingerprint);
            if !paste_system_clipboard {
                app.global::<AppState>()
                    .invoke_paste_canvas_selection(24.0, 24.0);
                return;
            }
            let Some(system_clipboard) = system_clipboard else {
                return;
            };
            if store.borrow().canvas_notes.len() >= MAX_CANVAS_NODES {
                show_canvas_capacity_status(&app);
                return;
            }

            let id = Uuid::new_v4().to_string();
            let state = app.global::<AppState>();
            let note = match system_clipboard {
                CanvasSystemClipboard::Image {
                    width,
                    height,
                    bytes,
                    ..
                } => {
                    let Some(image) = persist_canvas_clipboard_image(width, height, bytes) else {
                        return;
                    };
                    let (_, width, height) = canvas_node_defaults("image", false);
                    let mut note = CanvasNoteData {
                        id: id.clone(),
                        kind: "board-image".into(),
                        x: center_x - width / 2.0,
                        y: center_y - height / 2.0,
                        width,
                        height,
                        image_path: image.path,
                        selected: true,
                        ..CanvasNoteData::default()
                    };
                    fit_image_node_to_intrinsic_aspect(&mut note, image.width, image.height);
                    note
                }
                CanvasSystemClipboard::Text { text, .. } => {
                    let (_, width, height) =
                        canvas_node_defaults("text", state.get_language().as_str() == "en");
                    CanvasNoteData {
                        id: id.clone(),
                        kind: "text".into(),
                        content: text,
                        x: center_x - width / 2.0,
                        y: center_y - height / 2.0,
                        width,
                        height,
                        selected: true,
                        ..CanvasNoteData::default()
                    }
                }
            };

            let mut store_mut = store.borrow_mut();
            if store_mut.canvas_notes.len() >= MAX_CANVAS_NODES {
                if note.kind == "board-image" {
                    let _ = fs::remove_file(&note.image_path);
                }
                show_canvas_capacity_status(&app);
                return;
            }
            history.borrow_mut().record(canvas_snapshot(&store_mut));
            clear_selection(&mut store_mut.canvas_notes);
            store_mut.canvas_notes.push(note);
            persist_canvas(&app, &store_mut);
            sync_canvas_selection(&app, &store_mut);
            state.set_canvas_selected_id(id.into());
            state.set_canvas_selected_link_id("".into());
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
                let (_, width, height) = canvas_node_defaults("group", english);
                let content = next_group_name(&store_mut.canvas_notes, english);
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
        state.on_ungroup_canvas_node(move |id| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut store_mut = store.borrow_mut();
            let before = canvas_snapshot(&store_mut);
            if !ungroup_node(&mut store_mut.canvas_notes, id.as_str()) {
                return;
            }
            history.borrow_mut().record(before);
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
        state.on_remove_canvas_group_with_children(move |id| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut store_mut = store.borrow_mut();
            let before = canvas_snapshot(&store_mut);
            let mut links = std::mem::take(&mut store_mut.canvas_links);
            let removed =
                remove_group_with_descendants(&mut store_mut.canvas_notes, &mut links, id.as_str());
            store_mut.canvas_links = links;
            if removed.is_empty() {
                return;
            }
            history.borrow_mut().record(before);
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
            } = connect_nodes_with_flow(
                &mut store_mut.canvas_links,
                source_id.as_str(),
                target_id.as_str(),
                true,
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
    fn legacy_canvas_links_keep_forward_flow_by_default() {
        let legacy = r#"{"id":"link","source_id":"a","target_id":"b"}"#;
        let parsed: CanvasLinkData = serde_json::from_str(legacy).expect("legacy canvas link");

        assert!(!parsed.flow_reversed);
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
            flow_reversed: false,
        }];
        assert!(link_reaches(&links, "source", "target"));
        assert!(!link_reaches(&links, "target", "source"));
    }

    #[test]
    fn split_spans_assign_pixel_remainders_to_terminal_tiles() {
        assert_eq!(terminal_remainder_span(10, 3, 0), (0, 3));
        assert_eq!(terminal_remainder_span(10, 3, 1), (3, 3));
        assert_eq!(terminal_remainder_span(10, 3, 2), (6, 4));
    }

    #[test]
    fn split_line_counts_create_one_more_tile_part_per_axis() {
        assert_eq!(split_parts_from_lines(1), Some(2));
        assert_eq!(split_parts_from_lines(2), Some(3));
        assert_eq!(split_parts_from_lines(64), Some(65));
    }

    #[test]
    fn canvas_image_split_is_lossless_and_covers_every_pixel() {
        let temp = tempfile::tempdir().expect("temporary split directory");
        let source_path = temp.path().join("source.png");
        let output_dir = app_data_dir()
            .join("canvas")
            .join("splits")
            .join(format!("test-{}", Uuid::new_v4()));
        fs::create_dir_all(app_data_dir()).expect("prepare managed app data directory");
        let mut source = image::RgbaImage::new(5, 3);
        for y in 0..3 {
            for x in 0..5 {
                source.put_pixel(x, y, image::Rgba([x as u8, y as u8, 42, 255]));
            }
        }
        let bytes = encode_png_rgba(&source, 5, 3).expect("encode split source");
        atomic_write_file(&source_path, &bytes).expect("write split source");

        let tiles =
            split_canvas_image_to_directory(&source_path, &output_dir, 2, 2).expect("split source");
        assert_eq!(tiles.len(), 4);
        let mut rebuilt = image::RgbaImage::new(5, 3);
        for tile in &tiles {
            let (left, _) = terminal_remainder_span(5, 2, tile.column);
            let (top, _) = terminal_remainder_span(3, 2, tile.row);
            let decoded = image::open(&tile.path).expect("read tile").to_rgba8();
            image::imageops::replace(&mut rebuilt, &decoded, left as i64, top as i64);
        }
        assert_eq!(rebuilt, source);
        remove_canvas_split_tiles(&tiles);
    }

    #[test]
    fn canvas_element_extraction_saves_transparent_png_board_images() {
        let temp = tempfile::tempdir().expect("temporary extraction source directory");
        let source_path = temp.path().join("source.png");
        let output_dir = app_data_dir()
            .join("canvas")
            .join("ui-extractions")
            .join(format!("test-{}", Uuid::new_v4()));
        fs::create_dir_all(app_data_dir()).expect("prepare managed app data directory");
        let mut source = image::RgbaImage::from_pixel(480, 360, image::Rgba([250, 249, 246, 255]));
        for (left, top, right, bottom, color) in [
            (30, 35, 155, 130, [32, 74, 180, 255]),
            (280, 30, 430, 120, [204, 62, 72, 255]),
            (45, 225, 175, 325, [56, 164, 92, 255]),
            (290, 210, 440, 330, [128, 68, 184, 255]),
        ] {
            for y in top..bottom {
                for x in left..right {
                    source.put_pixel(x, y, image::Rgba(color));
                }
            }
        }
        let bytes = encode_png_rgba(&source, source.width(), source.height())
            .expect("encode extraction source");
        atomic_write_file(&source_path, &bytes).expect("write extraction source");

        let elements = extract_canvas_elements_to_directory(&source_path, &output_dir)
            .expect("extract current canvas image");

        assert_eq!(elements.len(), 4);
        for element in &elements {
            assert!(element.path.ends_with(".png"));
            let decoded = image::open(&element.path)
                .expect("read extracted PNG")
                .to_rgba8();
            assert!(decoded.pixels().any(|pixel| pixel[3] == 0));
        }
        remove_canvas_extracted_elements(&elements);
    }
}
