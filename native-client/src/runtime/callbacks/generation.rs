use super::*;

#[derive(Clone)]
enum PromptResultTarget {
    Composer,
    CanvasNode {
        store: Rc<RefCell<Store>>,
        id: String,
    },
}

struct PromptTaskRequest {
    backend: Arc<BackendRuntime>,
    model_code: String,
    task_type: &'static str,
    prompt: String,
    target_language: Option<String>,
    optimize: bool,
    target: PromptResultTarget,
}

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
            start_generation(&app, context.clone(), None, true, None, None);
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
            let prompt = store
                .borrow()
                .generations
                .iter()
                .find(|g| g.id == id.to_string())
                .map(|g| g.prompt.clone());
            if let Some(conversation_id) = store
                .borrow()
                .generations
                .iter()
                .find(|g| g.id == id.to_string())
                .map(|g| g.conversation_id.clone())
            {
                app.global::<AppState>()
                    .set_current_conversation_id(conversation_id.into());
            }
            start_generation(&app, context.clone(), prompt, false, None, None);
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
    let raw_prompt = state.get_prompt().trim().to_string();
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
    let Some(backend) = context.backend.clone() else {
        state.set_generation_status("服务端尚未初始化，请重启客户端后重试".into());
        return;
    };
    let model_code = state.get_reasoning_model().to_string();
    if model_code.trim().is_empty() {
        state.set_generation_status("服务端没有可用的提示词模型".into());
        return;
    }
    state.set_generation_status(if visual_mode {
        "正在根据提示词优化（当前服务端版本不上传参考图内容）...".into()
    } else {
        "正在优化提示词...".into()
    });
    state.set_optimizing_prompt(true);
    start_backend_prompt_task(
        app,
        PromptTaskRequest {
            backend,
            model_code,
            task_type: "prompt_optimize",
            prompt: raw_prompt,
            target_language: None,
            optimize: true,
            target: PromptResultTarget::Composer,
        },
    );
}

fn optimize_canvas_text_node(app: &AppWindow, context: AppContext, id: String, prompt: String) {
    let state = app.global::<AppState>();
    if !require_online_operation(app, "优化提示词") || state.get_optimizing_prompt() {
        return;
    }
    let raw_prompt = prompt.trim().to_string();
    if raw_prompt.is_empty() {
        state.set_generation_status("请先输入需要优化的文字内容".into());
        return;
    }
    let Some(backend) = context.backend.clone() else {
        state.set_generation_status("服务端尚未初始化，请重启客户端后重试".into());
        return;
    };
    let model_code = state.get_reasoning_model().to_string();
    if model_code.trim().is_empty() {
        state.set_generation_status("服务端没有可用的提示词模型".into());
        return;
    }

    state.set_generation_status("正在优化文字节点提示词...".into());
    state.set_optimizing_prompt(true);
    start_backend_prompt_task(
        app,
        PromptTaskRequest {
            backend,
            model_code,
            task_type: "prompt_optimize",
            prompt: raw_prompt,
            target_language: None,
            optimize: true,
            target: PromptResultTarget::CanvasNode {
                store: context.store,
                id,
            },
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
    let raw_prompt = state.get_prompt().trim().to_string();
    if raw_prompt.is_empty() {
        state.set_translate_prompt(false);
        return;
    }
    let Some(backend) = context.backend.clone() else {
        state.set_generation_status("服务端尚未初始化，请重启客户端后重试".into());
        state.set_translate_prompt(false);
        return;
    };
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
        PromptTaskRequest {
            backend,
            model_code,
            task_type: "prompt_translate",
            prompt: raw_prompt,
            target_language: Some("English".to_string()),
            optimize: false,
            target: PromptResultTarget::Composer,
        },
    );
}

fn start_backend_prompt_task(app: &AppWindow, task: PromptTaskRequest) {
    let PromptTaskRequest {
        backend,
        model_code,
        task_type,
        prompt,
        target_language,
        optimize,
        target,
    } = task;
    let (sender, receiver) = mpsc::channel::<std::result::Result<String, String>>();
    std::thread::spawn(move || {
        let api = GenerationApi::new(backend.api.clone());
        let request_id = Uuid::new_v4().simple().to_string();
        let request = CreateGenerationTask {
            client_request_id: request_id,
            task_type: task_type.to_string(),
            model_code,
            prompt,
            quality: None,
            count: None,
            aspect_ratio: None,
            reference_file_ids: None,
            target_language,
        };
        let result = (|| {
            let mut detail = api
                .create_task(&request)
                .map_err(|error| error.user_message())?;
            loop {
                if detail.terminal() {
                    if matches!(detail.status.as_str(), "completed" | "partially_completed") {
                        return detail
                            .result_prompt
                            .filter(|value| !value.trim().is_empty())
                            .ok_or_else(|| "服务端任务未返回提示词结果".to_string());
                    }
                    return Err(detail
                        .failure
                        .map(|failure| failure.message)
                        .unwrap_or_else(|| "服务端提示词任务执行失败".to_string()));
                }
                std::thread::sleep(Duration::from_millis(IMAGE_POLL_INTERVAL_MS));
                detail = api.task(&detail.id).map_err(|error| error.user_message())?;
            }
        })();
        let _ = sender.send(result);
    });
    poll_backend_prompt_result(
        app.as_weak(),
        Rc::new(RefCell::new(Some(receiver))),
        optimize,
        target,
    );
}

fn poll_backend_prompt_result(
    app_weak: Weak<AppWindow>,
    receiver: Rc<RefCell<Option<mpsc::Receiver<std::result::Result<String, String>>>>>,
    optimize: bool,
    target: PromptResultTarget,
) {
    slint::Timer::single_shot(Duration::from_millis(100), move || {
        let result = {
            let mut slot = receiver.borrow_mut();
            let Some(rx) = slot.as_ref() else {
                return;
            };
            match rx.try_recv() {
                Ok(result) => {
                    slot.take();
                    Some(result)
                }
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    slot.take();
                    Some(Err("提示词任务已中断，请重试".to_string()))
                }
            }
        };
        let Some(result) = result else {
            poll_backend_prompt_result(app_weak, receiver, optimize, target);
            return;
        };
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let state = app.global::<AppState>();
        state.set_optimizing_prompt(false);
        state.set_translating_prompt(false);
        match result {
            Ok(prompt) => match target {
                PromptResultTarget::Composer => {
                    state.set_prompt(prompt.into());
                    state.set_generation_status(if optimize {
                        "提示词优化完成".into()
                    } else {
                        "提示词翻译完成".into()
                    });
                }
                PromptResultTarget::CanvasNode { store, id } => {
                    let position = store
                        .borrow()
                        .canvas_notes
                        .iter()
                        .find(|node| node.id == id && node.kind == "text")
                        .map(|node| (node.x, node.y));
                    if let Some((x, y)) = position {
                        state.invoke_update_canvas_node(id.into(), prompt.into(), x, y);
                        state.set_generation_status("文字节点提示词优化完成".into());
                    } else {
                        state.set_generation_status("文字节点已不存在，未写入优化结果".into());
                    }
                }
            },
            Err(reason) => {
                state.set_generation_status(format!("提示词处理失败：{reason}").into());
                if !optimize {
                    state.set_translate_prompt(false);
                }
            }
        }
    });
}
