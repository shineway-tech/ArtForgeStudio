use super::*;

const MAX_CANVAS_NOTES: usize = 200;

pub(super) fn wire_infinite_canvas_callbacks(app: &AppWindow, store: Rc<RefCell<Store>>) {
    let state = app.global::<AppState>();

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_add_canvas_note(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            let mut store_mut = store.borrow_mut();
            if store_mut.canvas_notes.len() >= MAX_CANVAS_NOTES {
                return;
            }
            let index = store_mut.canvas_notes.len();
            store_mut.canvas_notes.push(CanvasNoteData {
                id: Uuid::new_v4().to_string(),
                content: if state.get_language().as_str() == "en" {
                    "New note".to_string()
                } else {
                    "新建便签".to_string()
                },
                x: 40.0 + (index % 4) as f32 * 300.0,
                y: 40.0 + (index / 4) as f32 * 220.0,
            });
            drop(store_mut);
            push_canvas_notes(&app, &store.borrow());
            save_local_store(&app, &store.borrow());
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_update_canvas_note(move |id, content, x, y| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut store_mut = store.borrow_mut();
            let Some(note) = store_mut
                .canvas_notes
                .iter_mut()
                .find(|note| note.id == id.as_str())
            else {
                return;
            };
            note.content = content.to_string();
            note.x = x;
            note.y = y;
            drop(store_mut);
            push_canvas_notes(&app, &store.borrow());
            save_local_store(&app, &store.borrow());
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_remove_canvas_note(move |id| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut store_mut = store.borrow_mut();
            let original_len = store_mut.canvas_notes.len();
            store_mut.canvas_notes.retain(|note| note.id != id.as_str());
            if store_mut.canvas_notes.len() == original_len {
                return;
            }
            drop(store_mut);
            push_canvas_notes(&app, &store.borrow());
            save_local_store(&app, &store.borrow());
        });
    }
}
