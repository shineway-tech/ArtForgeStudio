use super::*;

fn persist_custom_prompt_before_ack(
    persist: impl FnOnce() -> Result<()>,
    acknowledge: impl FnOnce(),
) -> Result<()> {
    persist()?;
    acknowledge();
    Ok(())
}

pub(super) const INLINE_CUSTOM_PROMPT_ICON_PLACEHOLDER: char = '\u{3000}';

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PromptEditorEdit {
    pub(super) text: String,
    pub(super) cursor_offset: i32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct InlineCustomPromptOccurrence {
    pub(super) name: String,
    pub(super) content: String,
    pub(super) prefix: String,
    pub(super) start_offset: i32,
    pub(super) end_offset: i32,
}

pub(super) fn inline_custom_prompt_display_text(name: &str) -> String {
    format!("{INLINE_CUSTOM_PROMPT_ICON_PLACEHOLDER}{}", name.trim())
}

fn clamped_char_boundary(text: &str, requested: usize) -> usize {
    let mut offset = requested.min(text.len());
    while offset > 0 && !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

fn needs_spacing(character: char) -> bool {
    !character.is_whitespace()
        && !matches!(
            character,
            '，' | '。' | '、' | '；' | '：' | '！' | '？' | ',' | '.' | ';' | ':' | '!' | '?'
        )
}

pub(super) fn remove_custom_prompt_trigger_before_cursor(
    editor_text: &str,
    requested_offset: usize,
) -> PromptEditorEdit {
    let offset = clamped_char_boundary(editor_text, requested_offset);
    if offset == 0 || !editor_text[..offset].ends_with('/') {
        return PromptEditorEdit {
            text: editor_text.to_string(),
            cursor_offset: -1,
        };
    }
    let trigger_start = offset - 1;
    let mut text = String::with_capacity(editor_text.len() - 1);
    text.push_str(&editor_text[..trigger_start]);
    text.push_str(&editor_text[offset..]);
    PromptEditorEdit {
        text,
        cursor_offset: trigger_start.min(i32::MAX as usize) as i32,
    }
}

pub(super) fn insert_custom_prompt_name_at_byte_offset(
    editor_text: &str,
    requested_offset: usize,
    name: &str,
) -> PromptEditorEdit {
    let offset = clamped_char_boundary(editor_text, requested_offset);
    let before = &editor_text[..offset];
    let after = &editor_text[offset..];
    let leading_space = before.chars().last().is_some_and(needs_spacing);
    let trailing_space = after.chars().next().is_some_and(needs_spacing);
    let display = inline_custom_prompt_display_text(name);
    let mut inserted = String::new();
    if leading_space {
        inserted.push(' ');
    }
    inserted.push_str(&display);
    if trailing_space {
        inserted.push(' ');
    }
    let cursor_offset = offset.saturating_add(inserted.len());
    let mut text = String::with_capacity(editor_text.len() + inserted.len());
    text.push_str(before);
    text.push_str(&inserted);
    text.push_str(after);
    PromptEditorEdit {
        text,
        cursor_offset: cursor_offset.min(i32::MAX as usize) as i32,
    }
}

pub(super) fn remove_custom_prompt_name(editor_text: &str, name: &str) -> PromptEditorEdit {
    let display = inline_custom_prompt_display_text(name);
    let Some(start) = editor_text.find(&display) else {
        return PromptEditorEdit {
            text: editor_text.to_string(),
            cursor_offset: editor_text.len().min(i32::MAX as usize) as i32,
        };
    };
    let mut remove_start = start;
    let mut remove_end = start + display.len();
    let before = &editor_text[..remove_start];
    let after = &editor_text[remove_end..];
    let before_space = before.chars().last().filter(|value| value.is_whitespace());
    let after_space = after.chars().next().filter(|value| value.is_whitespace());
    if let Some(character) = after_space.filter(|_| before_space.is_some() || before.is_empty()) {
        remove_end += character.len_utf8();
    } else if let Some(character) = before_space.filter(|_| after.is_empty()) {
        remove_start -= character.len_utf8();
    }
    let mut text = String::with_capacity(editor_text.len() - (remove_end - remove_start));
    text.push_str(&editor_text[..remove_start]);
    text.push_str(&editor_text[remove_end..]);
    PromptEditorEdit {
        text,
        cursor_offset: remove_start.min(i32::MAX as usize) as i32,
    }
}

pub(super) fn replace_custom_prompt_name(
    editor_text: &str,
    original_name: &str,
    replacement_name: &str,
) -> PromptEditorEdit {
    let original = inline_custom_prompt_display_text(original_name);
    let Some(start) = editor_text.find(&original) else {
        return PromptEditorEdit {
            text: editor_text.to_string(),
            cursor_offset: editor_text.len().min(i32::MAX as usize) as i32,
        };
    };
    let replacement = inline_custom_prompt_display_text(replacement_name);
    let end = start + original.len();
    let mut text = String::with_capacity(editor_text.len() + replacement.len() - original.len());
    text.push_str(&editor_text[..start]);
    text.push_str(&replacement);
    text.push_str(&editor_text[end..]);
    PromptEditorEdit {
        text,
        cursor_offset: (start + replacement.len()).min(i32::MAX as usize) as i32,
    }
}

pub(super) fn inline_custom_prompt_occurrences(
    editor_text: &str,
    replacements: &[(String, String)],
) -> Vec<InlineCustomPromptOccurrence> {
    let mut occurrences = replacements
        .iter()
        .filter_map(|(name, content)| {
            let display = inline_custom_prompt_display_text(name);
            let start = editor_text.find(&display)?;
            let end = start + display.len();
            Some(InlineCustomPromptOccurrence {
                name: name.clone(),
                content: content.clone(),
                prefix: editor_text[..start].to_string(),
                start_offset: start.min(i32::MAX as usize) as i32,
                end_offset: end.min(i32::MAX as usize) as i32,
            })
        })
        .collect::<Vec<_>>();
    occurrences.sort_by_key(|item| item.start_offset);
    occurrences
}

fn slint_prompt_text_edit(edit: PromptEditorEdit) -> PromptTextEdit {
    PromptTextEdit {
        text: edit.text.into(),
        cursor_offset: edit.cursor_offset,
    }
}

pub(super) fn wire_custom_prompt_callbacks(app: &AppWindow, context: AppContext) {
    let state = app.global::<AppState>();
    let store = context.store.clone();

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_normalize_prompt_editor_text(move |editor_text, _selected_prefix| {
            let Some(app) = app_weak.upgrade() else {
                return editor_text;
            };
            let state = app.global::<AppState>();
            let editor_text = editor_text.to_string();
            if state.get_prompt().as_str() != editor_text {
                state.set_prompt(editor_text.clone().into());
            }
            let category = current_workspace_category(&app);
            let selection_changed = {
                let mut store_mut = store.borrow_mut();
                let selected = store_mut
                    .selected_custom_prompts
                    .get(&category)
                    .cloned()
                    .unwrap_or_default();
                let retained = selected
                    .into_iter()
                    .filter(|prompt| {
                        let name = custom_prompt_display_name(&store_mut, prompt);
                        editor_text.contains(&inline_custom_prompt_display_text(&name))
                    })
                    .collect::<BTreeSet<_>>();
                let changed = store_mut
                    .selected_custom_prompts
                    .get(&category)
                    .is_some_and(|current| current != &retained);
                if retained.is_empty() {
                    store_mut.selected_custom_prompts.remove(&category);
                } else {
                    store_mut
                        .selected_custom_prompts
                        .insert(category.clone(), retained);
                }
                changed
            };
            push_custom_prompts(&app, &store.borrow());
            if selection_changed {
                save_local_store(&app, &store.borrow());
            }
            editor_text.into()
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_prepare_custom_prompt_insertion(move |editor_text, cursor_offset| {
            let edit = remove_custom_prompt_trigger_before_cursor(
                editor_text.as_str(),
                cursor_offset.max(0) as usize,
            );
            if edit.cursor_offset >= 0 {
                if let Some(app) = app_weak.upgrade() {
                    app.global::<AppState>()
                        .set_prompt(edit.text.clone().into());
                }
            }
            slint_prompt_text_edit(edit)
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_apply_inline_custom_prompt(move |prompt, editor_text, cursor_offset| {
            let Some(app) = app_weak.upgrade() else {
                return PromptTextEdit {
                    text: editor_text,
                    cursor_offset,
                };
            };
            let prompt = prompt.to_string();
            let editor_text = editor_text.to_string();
            let category = current_workspace_category(&app);
            let (name, selected) = {
                let store_ref = store.borrow();
                (
                    custom_prompt_display_name(&store_ref, &prompt),
                    custom_prompt_selected_for_category(&store_ref, &category, &prompt),
                )
            };
            let edit = if selected {
                remove_custom_prompt_name(&editor_text, &name)
            } else {
                insert_custom_prompt_name_at_byte_offset(
                    &editor_text,
                    cursor_offset.max(0) as usize,
                    &name,
                )
            };
            {
                let mut store_mut = store.borrow_mut();
                if store_mut.custom_prompts.contains(&prompt) {
                    toggle_custom_prompt_selection_for_category(&mut store_mut, &category, &prompt);
                }
            }
            let state = app.global::<AppState>();
            state.set_prompt(edit.text.clone().into());
            push_custom_prompts(&app, &store.borrow());
            save_local_store(&app, &store.borrow());
            slint_prompt_text_edit(edit)
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_clear_custom_prompt_selections(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let category = current_workspace_category(&app);
            store.borrow_mut().selected_custom_prompts.remove(&category);
            push_custom_prompts(&app, &store.borrow());
            save_local_store(&app, &store.borrow());
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_toggle_custom_prompt_selection(move |prompt| {
            let app_weak = app_weak.clone();
            let store = store.clone();
            let prompt = prompt.to_string();
            slint::Timer::single_shot(Duration::ZERO, move || {
                let Some(app) = app_weak.upgrade() else {
                    return;
                };
                {
                    let mut store_mut = store.borrow_mut();
                    if !store_mut.custom_prompts.contains(&prompt) {
                        return;
                    }
                    let category = current_workspace_category(&app);
                    toggle_custom_prompt_selection_for_category(&mut store_mut, &category, &prompt);
                }
                let state = app.global::<AppState>();
                if state.get_prompt().trim() == "//" {
                    state.set_prompt("".into());
                }
                push_custom_prompts(&app, &store.borrow());
                save_local_store(&app, &store.borrow());
            });
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_begin_new_custom_prompt(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            release_custom_prompt_recovered_result(&app, &context);
            reset_custom_prompt_editor(&app);
            open_custom_prompt_editor(&app);
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
            let reference_paths = custom_prompt_profile_reference_paths(&profile);
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
            state.set_custom_prompt_editor_session_id(Uuid::new_v4().to_string().into());
            state.set_custom_prompt_editing_original(prompt.into());
            state.set_custom_prompt_category(
                normalized_custom_prompt_category(&profile.category).into(),
            );
            state.set_custom_prompt_format(normalized_custom_prompt_format(&profile.format).into());
            state.set_custom_prompt_negative(profile.negative_prompt.into());
            set_custom_prompt_references(
                &app,
                reference_paths
                    .into_iter()
                    .filter_map(|path| {
                        load_preview_image(Path::new(&path), PreviewPurpose::Reference)
                            .ok()
                            .map(|image| ReferenceItem {
                                id: path.clone().into(),
                                image,
                                source_path: path.into(),
                            })
                    })
                    .collect(),
            );
            state.set_custom_prompt_message("".into());
            open_custom_prompt_editor(&app);
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_close_custom_prompt_editor(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            close_custom_prompt_editor(&app, &context);
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_choose_custom_prompt_reference(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(paths) = rfd::FileDialog::new()
                .add_filter("Images", crate::image_formats::picker_image_extensions())
                .pick_files()
            else {
                return;
            };
            let state = app.global::<AppState>();
            let model = state.get_custom_prompt_reference_items();
            let mut items = (0..model.row_count())
                .filter_map(|index| model.row_data(index))
                .collect::<Vec<_>>();
            let mut rejected = false;
            for path in paths {
                if items.len() >= MAX_CUSTOM_PROMPT_REFERENCES {
                    break;
                }
                let path_text = path.display().to_string();
                if items
                    .iter()
                    .any(|item| item.source_path.as_str().eq_ignore_ascii_case(&path_text))
                {
                    continue;
                }
                match load_preview_image(&path, PreviewPurpose::Reference) {
                    Ok(image) => items.push(ReferenceItem {
                        id: path_text.clone().into(),
                        image,
                        source_path: path_text.into(),
                    }),
                    Err(_) => rejected = true,
                }
            }
            set_custom_prompt_references(&app, items);
            state.set_custom_prompt_message(if rejected {
                if state.get_language().as_str() == "en" {
                    "Some selected files are not supported images"
                } else {
                    "部分所选文件不是受支持的图片"
                }
                .into()
            } else {
                "".into()
            });
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_clear_custom_prompt_reference(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            set_custom_prompt_references(&app, Vec::new());
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_remove_custom_prompt_reference(move |reference_id| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            let model = state.get_custom_prompt_reference_items();
            let items = (0..model.row_count())
                .filter_map(|index| model.row_data(index))
                .filter(|item| item.id != reference_id)
                .collect::<Vec<_>>();
            set_custom_prompt_references(&app, items);
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_open_custom_prompt_reference(move |reference_id| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            let model = state.get_custom_prompt_reference_items();
            let Some(item) = (0..model.row_count())
                .filter_map(|index| model.row_data(index))
                .find(|item| item.id == reference_id)
            else {
                return;
            };
            state.set_viewer_id(item.id);
            state.set_viewer_source("reference".into());
            state.set_viewer_source_path(item.source_path.clone());
            state.set_viewer_image(item.image);
            state.set_viewer_title(if state.get_language().as_str() == "en" {
                "Reference image".into()
            } else {
                "参考图".into()
            });
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

    {
        let app_weak = app.as_weak();
        let analysis_context = context.clone();
        state.on_analyze_custom_prompt_reference(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            if state.get_custom_prompt_analyzing() {
                return;
            }
            if !require_online_operation(&app, "分析参考图风格") {
                state.set_custom_prompt_message(
                    if state.get_language().as_str() == "en" {
                        "Image style analysis requires an internet connection"
                    } else {
                        "图片风格分析需要联网，请检查网络后重试"
                    }
                    .into(),
                );
                return;
            }
            let reference_paths = custom_prompt_reference_paths(&app)
                .into_iter()
                .map(PathBuf::from)
                .collect::<Vec<_>>();
            if reference_paths.is_empty() {
                state.set_custom_prompt_message(
                    if state.get_language().as_str() == "en" {
                        "Upload a style reference image first"
                    } else {
                        "请先上传风格参考图"
                    }
                    .into(),
                );
                return;
            }
            if analysis_context.backend.is_none() {
                state.set_custom_prompt_message(
                    if state.get_language().as_str() == "en" {
                        "The model service is unavailable"
                    } else {
                        "模型服务暂不可用"
                    }
                    .into(),
                );
                return;
            }
            if reference_paths.iter().any(|path| !path.is_file()) {
                state.set_custom_prompt_message(
                    if state.get_language().as_str() == "en" {
                        "The reference image file no longer exists"
                    } else {
                        "参考图文件已不存在"
                    }
                    .into(),
                );
                return;
            }
            let selection = sync_style_analysis_selection(&state);
            if !selection.available {
                state.set_custom_prompt_message(
                    if state.get_language().as_str() == "en" {
                        "No image style analysis model is available"
                    } else {
                        "服务端没有可用的图片风格分析模型"
                    }
                    .into(),
                );
                return;
            }
            let model_code = selection.model_code;
            let english = state.get_language().as_str() == "en";
            let target_id = state.get_custom_prompt_editor_session_id().to_string();
            let target_input = state.get_custom_prompt_input().to_string();
            state.set_custom_prompt_analyzing(true);
            state.set_custom_prompt_message(
                if english {
                    "Analyzing the reference image..."
                } else {
                    "正在分析参考图风格..."
                }
                .into(),
            );
            start_backend_prompt_task(
                &app,
                analysis_context.clone(),
                PromptTaskRequest {
                    model_code,
                    task_type: "image_style_analysis",
                    prompt: if english {
                        "Analyze the uploaded image's visual style. Return only one concise, \
                         reusable English image-generation style description covering \
                         composition, palette, lighting, rendering medium, texture, detail, and \
                         atmosphere. Do not describe file metadata and do not add headings."
                            .to_string()
                    } else {
                        "分析上传参考图的视觉风格。只输出一段可直接复用的中文生图风格描述，\
                         覆盖构图、配色、光影、绘制媒介、纹理、细节与氛围；不要描述文件元数据，\
                         不要添加标题。"
                            .to_string()
                    },
                    target_language: None,
                    optimize: true,
                    target: PromptResultTarget::CustomPrompt {
                        session_id: target_id,
                        input: target_input,
                        append_result: true,
                    },
                    reference_paths,
                },
            );
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        let context = context.clone();
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
            let format = normalized_custom_prompt_format(state.get_custom_prompt_format().as_str());
            let reference_paths = custom_prompt_reference_paths(&app);
            let original_prompt = original.trim().to_string();
            let category = current_workspace_category(&app);
            let (original_name, selected_in_current_category) = {
                let store_ref = store.borrow();
                (
                    custom_prompt_display_name(&store_ref, &original_prompt),
                    custom_prompt_selected_for_category(
                        &store_ref,
                        &category,
                        &original_prompt,
                    ),
                )
            };
            let replacement_name = name.clone();
            let previous_prompt = state.get_prompt().to_string();
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
                reference_path: reference_paths.first().cloned().unwrap_or_default(),
                reference_paths,
            };
            let previous_custom_state = {
                let store = store.borrow();
                (
                    store.custom_prompts.clone(),
                    store.selected_custom_prompts.clone(),
                    store.custom_prompt_times.clone(),
                    store.custom_prompt_profiles.clone(),
                )
            };
            let result = {
                let mut store = store.borrow_mut();
                let result =
                    save_custom_prompt_to_store(&mut store, &original, &prompt, &timestamp);
                if result == SaveCustomPromptResult::Saved {
                    save_custom_prompt_profile(&mut store, &original, &prompt, profile);
                    if !original_prompt.is_empty() {
                        replace_selected_custom_prompt(&mut store, &original_prompt, prompt.trim());
                    }
                }
                result
            };
            match result {
                SaveCustomPromptResult::Saved => {
                    if selected_in_current_category && original_name != replacement_name {
                        let edit = replace_custom_prompt_name(
                            state.get_prompt().as_str(),
                            &original_name,
                            &replacement_name,
                        );
                        state.set_prompt(edit.text.into());
                    }
                    push_custom_prompts(&app, &store.borrow());
                    let persisted = persist_custom_prompt_before_ack(
                        || save_local_store_checked(&app, &store.borrow()),
                        || acknowledge_custom_prompt_recovered_result(&app, &context),
                    );
                    if persisted.is_ok() {
                        reset_custom_prompt_editor(&app);
                        close_custom_prompt_editor(&app, &context);
                    } else {
                        let mut store_mut = store.borrow_mut();
                        store_mut.custom_prompts = previous_custom_state.0;
                        store_mut.selected_custom_prompts = previous_custom_state.1;
                        store_mut.custom_prompt_times = previous_custom_state.2;
                        store_mut.custom_prompt_profiles = previous_custom_state.3;
                        drop(store_mut);
                        state.set_prompt(previous_prompt.clone().into());
                        push_custom_prompts(&app, &store.borrow());
                        state.set_custom_prompt_message(
                            if state.get_language().as_str() == "en" {
                                "Unable to save locally. Your entered content is still available; please retry."
                            } else {
                                "本地保存失败，填写的内容仍已保留，请重试"
                            }
                            .into(),
                        );
                    }
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
            let category = current_workspace_category(&app);
            let (name, selected) = {
                let store_ref = store.borrow();
                (
                    custom_prompt_display_name(&store_ref, &prompt),
                    custom_prompt_selected_for_category(&store_ref, &category, &prompt),
                )
            };
            if remove_custom_prompt_from_store(&mut store.borrow_mut(), &prompt) {
                let state = app.global::<AppState>();
                if selected {
                    let edit = remove_custom_prompt_name(state.get_prompt().as_str(), &name);
                    state.set_prompt(edit.text.into());
                }
                state.set_custom_prompt_message("".into());
                push_custom_prompts(&app, &store.borrow());
                save_local_store(&app, &store.borrow());
            }
        });
    }
}

const MAX_CUSTOM_PROMPT_REFERENCES: usize = 8;

fn custom_prompt_profile_reference_paths(profile: &CustomPromptProfile) -> Vec<String> {
    let source = if profile.reference_paths.is_empty() {
        std::slice::from_ref(&profile.reference_path)
    } else {
        profile.reference_paths.as_slice()
    };
    source
        .iter()
        .filter(|path| !path.trim().is_empty())
        .take(MAX_CUSTOM_PROMPT_REFERENCES)
        .cloned()
        .collect()
}

fn custom_prompt_reference_paths(app: &AppWindow) -> Vec<String> {
    let model = app.global::<AppState>().get_custom_prompt_reference_items();
    (0..model.row_count())
        .filter_map(|index| model.row_data(index))
        .map(|item| item.source_path.to_string())
        .take(MAX_CUSTOM_PROMPT_REFERENCES)
        .collect()
}

fn set_custom_prompt_references(app: &AppWindow, items: Vec<ReferenceItem>) {
    let state = app.global::<AppState>();
    if let Some(first) = items.first() {
        state.set_custom_prompt_reference_path(first.source_path.clone());
        state.set_custom_prompt_reference_image(first.image.clone());
    } else {
        state.set_custom_prompt_reference_path("".into());
        state.set_custom_prompt_reference_image(Image::default());
    }
    state.set_custom_prompt_reference_items(ModelRc::new(VecModel::from(items)));
}

fn reset_custom_prompt_editor(app: &AppWindow) {
    let state = app.global::<AppState>();
    state.set_custom_prompt_name("".into());
    state.set_custom_prompt_input("".into());
    state.set_custom_prompt_editor_session_id(Uuid::new_v4().to_string().into());
    state.set_custom_prompt_category("default".into());
    state.set_custom_prompt_format("json".into());
    state.set_custom_prompt_negative("".into());
    set_custom_prompt_references(app, Vec::new());
    state.set_custom_prompt_message("".into());
    state.set_custom_prompt_analyzing(false);
    state.set_custom_prompt_editing_original("".into());
}

fn open_custom_prompt_editor(app: &AppWindow) {
    let state = app.global::<AppState>();
    let current_page = state.get_page().to_string();
    if current_page != "custom-prompt-editor" {
        let return_page = match current_page.as_str() {
            "generation" | "settings" => current_page,
            _ => "settings".to_string(),
        };
        state.set_custom_prompt_editor_return_page(return_page.into());
    }
    state.set_custom_prompt_editor_open(true);
    navigate_to(app, "custom-prompt-editor");
}

pub(super) fn close_custom_prompt_editor(app: &AppWindow, context: &AppContext) {
    release_custom_prompt_recovered_result(app, context);
    let state = app.global::<AppState>();
    let return_page = match state.get_custom_prompt_editor_return_page().as_str() {
        "generation" => "generation",
        _ => "settings",
    };
    state.set_custom_prompt_editor_open(false);
    state.set_custom_prompt_editor_session_id("".into());
    state.set_custom_prompt_analyzing(false);
    state.set_custom_prompt_message("".into());
    navigate_to_with_store(app, &context.store.borrow(), return_page);
}

pub(super) fn normalized_custom_prompt_category(value: &str) -> String {
    match value {
        "character" | "scene" | "ui" | "effect" => value.to_string(),
        _ => "default".to_string(),
    }
}

pub(super) fn normalized_custom_prompt_format(value: &str) -> String {
    if value == "txt" {
        "txt".to_string()
    } else {
        "json".to_string()
    }
}

#[allow(dead_code)]
fn legacy_reference_style(rgba: &[u8], width: u32, height: u32, english: bool) -> Option<String> {
    let pixel_count = rgba.len() / 4;
    if pixel_count == 0 || width == 0 || height == 0 {
        return None;
    }

    let sample_step = (pixel_count / 50_000).max(1);
    let mut samples = 0_f64;
    let mut red = 0_f64;
    let mut green = 0_f64;
    let mut blue = 0_f64;
    let mut luminance = 0_f64;
    let mut luminance_squared = 0_f64;
    let mut saturation = 0_f64;

    for pixel in rgba.chunks_exact(4).step_by(sample_step) {
        if pixel[3] == 0 {
            continue;
        }
        let r = pixel[0] as f64 / 255.0;
        let g = pixel[1] as f64 / 255.0;
        let b = pixel[2] as f64 / 255.0;
        let maximum = r.max(g).max(b);
        let minimum = r.min(g).min(b);
        let value = 0.2126 * r + 0.7152 * g + 0.0722 * b;

        samples += 1.0;
        red += r;
        green += g;
        blue += b;
        luminance += value;
        luminance_squared += value * value;
        saturation += if maximum <= f64::EPSILON {
            0.0
        } else {
            (maximum - minimum) / maximum
        };
    }

    if samples == 0.0 {
        return None;
    }

    let average_red = red / samples;
    let average_green = green / samples;
    let average_blue = blue / samples;
    let average_luminance = luminance / samples;
    let average_saturation = saturation / samples;
    let variance = (luminance_squared / samples - average_luminance * average_luminance).max(0.0);
    let contrast = variance.sqrt();
    let warm_balance = average_red - average_blue + (average_green - average_blue) * 0.12;

    let orientation = if width > height.saturating_mul(6) / 5 {
        if english {
            "landscape"
        } else {
            "横向"
        }
    } else if height > width.saturating_mul(6) / 5 {
        if english {
            "portrait"
        } else {
            "竖向"
        }
    } else if english {
        "square"
    } else {
        "方形"
    };
    let brightness = if average_luminance > 0.68 {
        if english {
            "bright and airy"
        } else {
            "明亮通透"
        }
    } else if average_luminance < 0.34 {
        if english {
            "deep low-key lighting"
        } else {
            "低调暗部"
        }
    } else if english {
        "balanced lighting"
    } else {
        "明暗均衡"
    };
    let temperature = if warm_balance > 0.07 {
        if english {
            "warm palette"
        } else {
            "暖色调"
        }
    } else if warm_balance < -0.06 {
        if english {
            "cool palette"
        } else {
            "冷色调"
        }
    } else if english {
        "neutral palette"
    } else {
        "中性色调"
    };
    let chroma = if average_saturation > 0.55 {
        if english {
            "vivid saturated color"
        } else {
            "色彩高饱和鲜明"
        }
    } else if average_saturation < 0.20 {
        if english {
            "soft restrained color"
        } else {
            "色彩低饱和柔和"
        }
    } else if english {
        "natural color saturation"
    } else {
        "色彩饱和度自然"
    };
    let tonal_contrast = if contrast > 0.24 {
        if english {
            "strong tonal contrast"
        } else {
            "强对比光影"
        }
    } else if contrast < 0.11 {
        if english {
            "soft low contrast"
        } else {
            "柔和低对比光影"
        }
    } else if english {
        "balanced tonal contrast"
    } else {
        "均衡对比光影"
    };
    let detail = if width.max(height) >= 2_000 {
        if english {
            "fine detailed texture"
        } else {
            "细节与纹理丰富"
        }
    } else if english {
        "clean controlled detail"
    } else {
        "细节简洁克制"
    };

    Some(if english {
        format!(
            "Reference style: {orientation} composition, {brightness}, {temperature}, \
             {chroma}, {tonal_contrast}, {detail}."
        )
    } else {
        format!(
            "参考图风格：{orientation}构图，{brightness}，{temperature}，{chroma}，\
             {tonal_contrast}，{detail}。"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_prompt_save_failure_never_acknowledges_recovered_result() {
        let acknowledged = std::cell::Cell::new(false);

        let result = persist_custom_prompt_before_ack(
            || Err(anyhow!("disk full")),
            || acknowledged.set(true),
        );

        assert!(result.is_err());
        assert!(!acknowledged.get());
    }

    #[test]
    fn custom_prompt_recovery_is_acknowledged_after_durable_save() {
        let events = RefCell::new(Vec::new());

        persist_custom_prompt_before_ack(
            || {
                events.borrow_mut().push("durable_save");
                Ok(())
            },
            || events.borrow_mut().push("acknowledge"),
        )
        .unwrap();

        assert_eq!(*events.borrow(), vec!["durable_save", "acknowledge"]);
    }

    #[test]
    fn custom_prompt_name_is_inserted_at_the_utf8_cursor_position() {
        let edit = insert_custom_prompt_name_at_byte_offset("古城，夜景", 9, "像素模板");

        assert_eq!(edit.text, "古城，\u{3000}像素模板 夜景");
        assert_eq!(edit.cursor_offset, 25);
    }

    #[test]
    fn custom_prompt_trigger_is_removed_at_a_middle_cursor_position() {
        let edit = remove_custom_prompt_trigger_before_cursor("前文 /后文", 8);

        assert_eq!(edit.text, "前文 后文");
        assert_eq!(edit.cursor_offset, 7);
    }

    #[test]
    fn inline_custom_prompt_occurrences_follow_their_text_order() {
        let replacements = vec![
            ("像素模板".to_string(), "PIXEL CONTENT".to_string()),
            ("镜头模板".to_string(), "CAMERA CONTENT".to_string()),
        ];

        let occurrences = inline_custom_prompt_occurrences(
            "前文 \u{3000}镜头模板 中段 \u{3000}像素模板 后文",
            &replacements,
        );

        assert_eq!(occurrences.len(), 2);
        assert_eq!(occurrences[0].name, "镜头模板");
        assert_eq!(occurrences[0].prefix, "前文 ");
        assert_eq!(occurrences[1].name, "像素模板");
        assert_eq!(occurrences[1].prefix, "前文 \u{3000}镜头模板 中段 ");
    }

    #[test]
    fn removing_one_inline_custom_prompt_preserves_the_other_text() {
        let edit = remove_custom_prompt_name(
            "前文 \u{3000}像素模板 中段 \u{3000}镜头模板 后文",
            "像素模板",
        );

        assert_eq!(edit.text, "前文 中段 \u{3000}镜头模板 后文");
        assert_eq!(edit.cursor_offset, 7);
    }

    #[test]
    fn removing_an_edge_inline_custom_prompt_does_not_leave_padding() {
        assert_eq!(
            remove_custom_prompt_name("\u{3000}像素模板 后文", "像素模板").text,
            "后文"
        );
        assert_eq!(
            remove_custom_prompt_name("前文 \u{3000}像素模板", "像素模板").text,
            "前文"
        );
    }

    #[test]
    fn renaming_an_inline_custom_prompt_preserves_its_position_and_other_names() {
        let edit = replace_custom_prompt_name(
            "前文 \u{3000}旧名称 中段 \u{3000}镜头模板 后文",
            "旧名称",
            "新名称",
        );

        assert_eq!(edit.text, "前文 \u{3000}新名称 中段 \u{3000}镜头模板 后文");
        assert_eq!(edit.cursor_offset, 19);
    }
}
