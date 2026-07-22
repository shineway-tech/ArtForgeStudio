use super::*;

const MAX_CANVAS_NODES: usize = 200;
const MAX_CANVAS_LINKS: usize = 400;
const MAX_CANVAS_HISTORY: usize = 100;

#[derive(Clone, Debug, Default, PartialEq)]
struct CanvasSnapshot {
    notes: Vec<CanvasNoteData>,
    links: Vec<CanvasLinkData>,
}

#[derive(Default)]
struct CanvasHistory {
    undo: Vec<CanvasSnapshot>,
    redo: Vec<CanvasSnapshot>,
}

impl CanvasHistory {
    fn record(&mut self, snapshot: CanvasSnapshot) {
        self.undo.push(snapshot);
        if self.undo.len() > MAX_CANVAS_HISTORY {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    fn undo(&mut self, current: CanvasSnapshot) -> Option<CanvasSnapshot> {
        let previous = self.undo.pop()?;
        self.redo.push(current);
        Some(previous)
    }

    fn redo(&mut self, current: CanvasSnapshot) -> Option<CanvasSnapshot> {
        let next = self.redo.pop()?;
        self.undo.push(current);
        Some(next)
    }
}

fn canvas_snapshot(store: &Store) -> CanvasSnapshot {
    CanvasSnapshot {
        notes: store.canvas_notes.clone(),
        links: store.canvas_links.clone(),
    }
}

fn restore_canvas_snapshot(store: &mut Store, snapshot: CanvasSnapshot) {
    store.canvas_notes = snapshot.notes;
    store.canvas_links = snapshot.links;
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

fn link_reaches(links: &[CanvasLinkData], start: &str, target: &str) -> bool {
    let mut pending = vec![start];
    let mut visited = BTreeSet::new();
    while let Some(node_id) = pending.pop() {
        if node_id == target {
            return true;
        }
        if !visited.insert(node_id.to_string()) {
            continue;
        }
        pending.extend(
            links
                .iter()
                .filter(|link| link.source_id == node_id)
                .map(|link| link.target_id.as_str()),
        );
    }
    false
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
        _ => (
            if english {
                "Use the edit button to change text"
            } else {
                "点击编辑按钮修改文字"
            }
            .to_string(),
            320.0,
            210.0,
        ),
    }
}

fn sync_history_state(app: &AppWindow, history: &CanvasHistory) {
    let state = app.global::<AppState>();
    state.set_canvas_can_undo(!history.undo.is_empty());
    state.set_canvas_can_redo(!history.redo.is_empty());
}

fn persist_canvas(app: &AppWindow, store: &Store) {
    push_canvas_notes(app, store);
    save_local_store(app, store);
}

pub(super) fn wire_infinite_canvas_callbacks(app: &AppWindow, store: Rc<RefCell<Store>>) {
    let state = app.global::<AppState>();
    let history = Rc::new(RefCell::new(CanvasHistory::default()));

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
                return;
            }

            let node_kind = match kind.as_str() {
                "image" | "video" | "audio" | "group" => kind.to_string(),
                _ => "text".to_string(),
            };
            let (content, width, height) =
                canvas_node_defaults(&node_kind, state.get_language().as_str() == "en");
            let id = Uuid::new_v4().to_string();
            history.borrow_mut().record(canvas_snapshot(&store_mut));
            store_mut.canvas_notes.push(CanvasNoteData {
                id: id.clone(),
                kind: node_kind,
                content,
                x: center_x - width / 2.0,
                y: center_y - height / 2.0,
                width,
                height,
            });
            persist_canvas(&app, &store_mut);
            drop(store_mut);
            state.set_canvas_selected_id(id.into());
            sync_history_state(&app, &history.borrow());
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
            store_mut.canvas_notes.retain(|note| note.id != id.as_str());
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
        let store = store.clone();
        let history = history.clone();
        state.on_finish_canvas_link(move |source_id, x, y, tolerance| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut store_mut = store.borrow_mut();
            if store_mut.canvas_links.len() >= MAX_CANVAS_LINKS
                || !store_mut
                    .canvas_notes
                    .iter()
                    .any(|note| note.id == source_id.as_str() && note.kind != "group")
            {
                return;
            }
            let Some(target_id) =
                target_at_input(&store_mut, source_id.as_str(), x, y, tolerance.max(8.0))
            else {
                return;
            };
            if store_mut
                .canvas_links
                .iter()
                .any(|link| link.source_id == source_id.as_str() && link.target_id == target_id)
                || link_reaches(&store_mut.canvas_links, &target_id, source_id.as_str())
            {
                return;
            }

            history.borrow_mut().record(canvas_snapshot(&store_mut));
            let id = Uuid::new_v4().to_string();
            store_mut.canvas_links.push(CanvasLinkData {
                id: id.clone(),
                source_id: source_id.to_string(),
                target_id: target_id.clone(),
            });
            persist_canvas(&app, &store_mut);
            drop(store_mut);
            let state = app.global::<AppState>();
            state.set_canvas_selected_id(target_id.into());
            state.set_canvas_selected_link_id(id.into());
            state.set_generation_status(
                if state.get_language().as_str() == "en" {
                    "Connected. Upstream content will be used during generation."
                } else {
                    "连接成功，生成时将自动使用上游节点内容。"
                }
                .into(),
            );
            sync_history_state(&app, &history.borrow());
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
        }
    }

    #[test]
    fn canvas_history_round_trips_undo_and_redo() {
        let mut history = CanvasHistory::default();
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
