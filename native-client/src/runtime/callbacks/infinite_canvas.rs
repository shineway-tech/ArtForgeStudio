use super::*;

const MAX_CANVAS_NODES: usize = 200;
const MAX_CANVAS_HISTORY: usize = 100;

#[derive(Default)]
struct CanvasHistory {
    undo: Vec<Vec<CanvasNoteData>>,
    redo: Vec<Vec<CanvasNoteData>>,
}

impl CanvasHistory {
    fn record(&mut self, snapshot: Vec<CanvasNoteData>) {
        self.undo.push(snapshot);
        if self.undo.len() > MAX_CANVAS_HISTORY {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    fn undo(&mut self, current: Vec<CanvasNoteData>) -> Option<Vec<CanvasNoteData>> {
        let previous = self.undo.pop()?;
        self.redo.push(current);
        Some(previous)
    }

    fn redo(&mut self, current: Vec<CanvasNoteData>) -> Option<Vec<CanvasNoteData>> {
        let next = self.redo.pop()?;
        self.undo.push(current);
        Some(next)
    }
}

fn canvas_node_defaults(kind: &str, english: bool) -> (String, f32, f32) {
    match kind {
        "image" => (
            if english {
                "Describe the image you want to generate"
            } else {
                "描述要生成的图片内容"
            }
            .to_string(),
            340.0,
            250.0,
        ),
        "video" => (
            if english {
                "Describe the video you want to generate"
            } else {
                "描述要生成的视频内容"
            }
            .to_string(),
            400.0,
            270.0,
        ),
        "audio" => (
            if english {
                "Describe the audio you want to generate"
            } else {
                "描述要生成的音频内容"
            }
            .to_string(),
            340.0,
            190.0,
        ),
        "group" => (
            if english { "Group" } else { "节点组" }.to_string(),
            680.0,
            360.0,
        ),
        _ => (
            if english {
                "Double-click to edit text"
            } else {
                "双击编辑文字"
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
            history.borrow_mut().record(store_mut.canvas_notes.clone());
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

            history.borrow_mut().record(store_mut.canvas_notes.clone());
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
            history.borrow_mut().record(store_mut.canvas_notes.clone());
            store_mut.canvas_notes.retain(|note| note.id != id.as_str());
            persist_canvas(&app, &store_mut);
            drop(store_mut);
            let state = app.global::<AppState>();
            if state.get_canvas_selected_id().as_str() == id.as_str() {
                state.set_canvas_selected_id("".into());
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
            let Some(previous) = history.borrow_mut().undo(store_mut.canvas_notes.clone()) else {
                return;
            };
            store_mut.canvas_notes = previous;
            persist_canvas(&app, &store_mut);
            drop(store_mut);
            app.global::<AppState>().set_canvas_selected_id("".into());
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
            let Some(next) = history.borrow_mut().redo(store_mut.canvas_notes.clone()) else {
                return;
            };
            store_mut.canvas_notes = next;
            persist_canvas(&app, &store_mut);
            drop(store_mut);
            app.global::<AppState>().set_canvas_selected_id("".into());
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
        history.record(vec![node("before")]);
        let previous = history.undo(vec![node("after")]).expect("undo state");
        assert_eq!(previous, vec![node("before")]);
        let next = history.redo(previous).expect("redo state");
        assert_eq!(next, vec![node("after")]);
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
}
