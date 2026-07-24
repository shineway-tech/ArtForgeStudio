use super::*;

pub(super) fn wire_custom_prompt_callbacks(app: &AppWindow, store: Rc<RefCell<Store>>) {
    let state = app.global::<AppState>();

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
    state.set_custom_prompt_editing_original("".into());
}

fn normalized_custom_prompt_category(value: &str) -> String {
    match value {
        "character" | "scene" | "ui" | "effect" => value.to_string(),
        _ => "default".to_string(),
    }
}

fn normalized_custom_prompt_format(value: &str) -> String {
    if value == "txt" {
        "txt".to_string()
    } else {
        "json".to_string()
    }
}
