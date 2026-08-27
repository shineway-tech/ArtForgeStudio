use super::*;

pub(super) fn wire_generation_callbacks(app: &AppWindow, context: AppContext) {
    let state = app.global::<AppState>();
    let store = context.store.clone();

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_generate(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            start_generation(
                &app,
                context.clone(),
                None,
                true,
                None,
                None,
                ExistingGenerationPolicy::StopExisting,
            );
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_stop_generation(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            stop_generation(&app, &context);
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_remove_prompt_history(move |prompt| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut store_mut = store.borrow_mut();
            if dismiss_prompt_history_entry(&mut store_mut, prompt.as_str()) {
                save_local_store(&app, &store_mut);
            }
            push_prompt_history(&app, &store_mut);
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_clear_prompt_history(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut store_mut = store.borrow_mut();
            if clear_prompt_history_entries(&mut store_mut) {
                save_local_store(&app, &store_mut);
            }
            push_prompt_history(&app, &store_mut);
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_optimize_current_prompt(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            optimize_current_prompt(&app, context.clone(), false);
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_optimize_canvas_text_node(move |id, prompt| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            optimize_canvas_text_node(&app, context.clone(), id.to_string(), prompt.to_string());
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_visual_optimize_current_prompt(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            optimize_current_prompt(&app, context.clone(), true);
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_translate_current_prompt(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            translate_current_prompt(&app, context.clone());
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_open_conversation(move |id| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            app.global::<AppState>().set_current_conversation_id(id);
            push_generations(&app, &store.borrow());
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        let context = context.clone();
        state.on_regenerate(move |id| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let item = store
                .borrow()
                .generations
                .iter()
                .find(|g| g.id == id.to_string())
                .cloned();
            if let Some(item) = item {
                start_asset_regeneration(&app, context.clone(), item);
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_retry_generation(move |id| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            retry_failed_generation(&app, context.clone(), id.to_string());
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_retry_generation_delivery(move |id| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            retry_failed_delivery(&app, context.clone(), id.to_string());
        });
    }

    {
        let app_weak = app.as_weak();
        let context = context.clone();
        state.on_optimize_custom_prompt_content(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            optimize_custom_prompt_content(&app, context.clone());
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_dismiss_new_generation(move |id| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let id = id.to_string();
            let mut store_mut = store.borrow_mut();
            for item in store_mut.generations.iter_mut() {
                if item.id == id {
                    item.is_new = false;
                }
            }
            for item in store_mut.assets.iter_mut() {
                if item.id == id {
                    item.is_new = false;
                }
            }
            push_all(&app, &store_mut);
        });
    }

    {
        let app_weak = app.as_weak();
        let store = store.clone();
        state.on_quote_generation(move |id| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let id = id.to_string();
            if let Some(item) = store
                .borrow()
                .generations
                .iter()
                .find(|g| g.id == id)
                .cloned()
            {
                let state = app.global::<AppState>();
                state.set_quote_title(item.title.into());
                state.set_quote_prompt(item.prompt.into());
                state.set_quote_ratio(item.ratio.into());
                state.set_quote_quality(item.quality.into());
                state.set_quote_width(item.width);
                state.set_quote_height(item.height);
            }
        });
    }
}

pub(super) fn optimize_current_prompt(app: &AppWindow, context: AppContext, visual_mode: bool) {
    let state = app.global::<AppState>();
    if !require_online_operation(app, "优化提示词") {
        return;
    }
    if state.get_optimizing_prompt() {
        return;
    }
    let target_input = state.get_prompt().to_string();
    let raw_prompt = target_input.trim().to_string();
    if raw_prompt.is_empty() {
        state.set_generation_status("请输入需要优化的提示词".into());
        return;
    }
    if visual_mode {
        let category = resolve_category(&state.get_asset_type().to_string(), &raw_prompt);
        if references_for_category(&context.store.borrow().references, &category).is_empty() {
            state.set_generation_status("请先上传参考图".into());
            return;
        }
    }
    if context.backend.is_none() {
        state.set_generation_status("服务端尚未初始化，请重启客户端后重试".into());
        return;
    }
    let model_code = if visual_mode {
        let selection = sync_style_analysis_selection(&state);
        if !selection.available {
            state.set_generation_status("服务端没有可用的图片风格分析模型".into());
            return;
        }
        selection.model_code
    } else {
        let preferred_model = state.get_reasoning_model().to_string();
        if preferred_model.trim().is_empty() {
            state.set_generation_status("服务端没有可用的提示词模型".into());
            return;
        }
        preferred_model
    };
    let reference_paths = if visual_mode {
        let category = resolve_category(&state.get_asset_type().to_string(), &raw_prompt);
        references_for_category(&context.store.borrow().references, &category)
            .iter()
            .take(MAX_REFERENCE_IMAGES)
            .map(|reference| PathBuf::from(&reference.source_path))
            .collect()
    } else {
        Vec::new()
    };
    state.set_generation_status(if visual_mode {
        "正在上传参考图并分析风格...".into()
    } else {
        "正在优化提示词...".into()
    });
    state.set_optimizing_prompt(true);
    start_backend_prompt_task(
        app,
        context.clone(),
        PromptTaskRequest {
            model_code,
            task_type: if visual_mode {
                "image_style_analysis"
            } else {
                "prompt_optimize"
            },
            prompt: if visual_mode {
                format!(
                    "结合上传参考图的视觉风格优化以下生图描述，只返回优化后的提示词：{raw_prompt}"
                )
            } else {
                raw_prompt
            },
            target_language: None,
            optimize: true,
            target: PromptResultTarget::Composer {
                category: current_workspace_category(app),
                input: target_input,
            },
            reference_paths,
        },
    );
}

fn optimize_custom_prompt_content(app: &AppWindow, context: AppContext) {
    let state = app.global::<AppState>();
    if !require_online_operation(app, "优化提示词") {
        state.set_custom_prompt_message("提示词优化需要联网，请检查网络后重试".into());
        return;
    }
    if state.get_optimizing_prompt() {
        return;
    }
    let target_input = state.get_custom_prompt_input().to_string();
    let raw_prompt = target_input.trim().to_string();
    if raw_prompt.is_empty() {
        state.set_custom_prompt_message("请先输入需要优化的提示词内容".into());
        return;
    }
    if context.backend.is_none() {
        state.set_custom_prompt_message("服务端尚未初始化，请重启客户端后重试".into());
        return;
    }
    let model_code = state.get_reasoning_model().to_string();
    if model_code.trim().is_empty() {
        state.set_custom_prompt_message("服务端没有可用的提示词模型".into());
        return;
    }
    let target_id = state.get_custom_prompt_editor_session_id().to_string();
    state.set_optimizing_prompt(true);
    state.set_custom_prompt_message("正在优化提示词内容...".into());
    start_backend_prompt_task(
        app,
        context.clone(),
        PromptTaskRequest {
            model_code,
            task_type: "prompt_optimize",
            prompt: raw_prompt,
            target_language: None,
            optimize: true,
            target: PromptResultTarget::CustomPrompt {
                session_id: target_id,
                input: target_input,
                append_result: false,
            },
            reference_paths: Vec::new(),
        },
    );
}

fn optimize_canvas_text_node(app: &AppWindow, context: AppContext, id: String, prompt: String) {
    let state = app.global::<AppState>();
    if !require_online_operation(app, "优化提示词") || state.get_optimizing_prompt() {
        return;
    }
    let target_input = prompt;
    let raw_prompt = target_input.trim().to_string();
    if raw_prompt.is_empty() {
        state.set_generation_status("请先输入需要优化的文字内容".into());
        return;
    }
    if context.backend.is_none() {
        state.set_generation_status("服务端尚未初始化，请重启客户端后重试".into());
        return;
    }
    let model_code = state.get_reasoning_model().to_string();
    if model_code.trim().is_empty() {
        state.set_generation_status("服务端没有可用的提示词模型".into());
        return;
    }

    state.set_generation_status("正在优化文字节点提示词...".into());
    state.set_optimizing_prompt(true);
    start_backend_prompt_task(
        app,
        context.clone(),
        PromptTaskRequest {
            model_code,
            task_type: "prompt_optimize",
            prompt: raw_prompt,
            target_language: None,
            optimize: true,
            target: PromptResultTarget::CanvasNode { id, input: target_input },
            reference_paths: Vec::new(),
        },
    );
}

pub(super) fn translate_current_prompt(app: &AppWindow, context: AppContext) {
    let state = app.global::<AppState>();
    if !require_online_operation(app, "翻译提示词") {
        state.set_translate_prompt(false);
        return;
    }
    if state.get_translating_prompt() {
        return;
    }
    let target_input = state.get_prompt().to_string();
    let raw_prompt = target_input.trim().to_string();
    if raw_prompt.is_empty() {
        state.set_translate_prompt(false);
        return;
    }
    if context.backend.is_none() {
        state.set_generation_status("服务端尚未初始化，请重启客户端后重试".into());
        state.set_translate_prompt(false);
        return;
    }
    let model_code = state.get_reasoning_model().to_string();
    if model_code.trim().is_empty() {
        state.set_generation_status("服务端没有可用的提示词模型".into());
        state.set_translate_prompt(false);
        return;
    }
    state.set_translating_prompt(true);
    state.set_generation_status("正在翻译提示词...".into());
    start_backend_prompt_task(
        app,
        context.clone(),
        PromptTaskRequest {
            model_code,
            task_type: "prompt_translate",
            prompt: raw_prompt,
            target_language: Some("English".to_string()),
            optimize: false,
            target: PromptResultTarget::Composer {
                category: current_workspace_category(app),
                input: target_input,
            },
            reference_paths: Vec::new(),
        },
    );
}
