use super::*;

pub(super) fn wire_custom_prompt_callbacks(app: &AppWindow, store: Rc<RefCell<Store>>) {
    let state = app.global::<AppState>();

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_add_custom_prompt(move |prompt| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            let result = add_custom_prompt_to_store(&mut store.borrow_mut(), &prompt);
            match result {
                AddCustomPromptResult::Added => {
                    state.set_custom_prompt_input("".into());
                    state.set_custom_prompt_message("".into());
                    state.set_custom_prompt_editor_open(false);
                    push_custom_prompts(&app, &store.borrow());
                    save_local_store(&app, &store.borrow());
                }
                AddCustomPromptResult::Empty => {
                    state.set_custom_prompt_message(
                        if state.get_language().as_str() == "en" {
                            "Enter a prompt first"
                        } else {
                            "请输入提示词"
                        }
                        .into(),
                    );
                }
                AddCustomPromptResult::Duplicate => {
                    state.set_custom_prompt_message(
                        if state.get_language().as_str() == "en" {
                            "This prompt already exists"
                        } else {
                            "该提示词已存在"
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
