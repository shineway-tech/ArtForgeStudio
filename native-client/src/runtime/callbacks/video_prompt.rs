use super::*;

pub(super) fn wire_video_prompt_callbacks(app: &AppWindow, context: AppContext) {
    let state = app.global::<AppState>();
    {
        let weak = app.as_weak();
        let context = context.clone();
        state.on_optimize_video_prompt(move || {
            let Some(app) = weak.upgrade() else { return };
            let state = app.global::<AppState>();
            if state.get_optimizing_video_prompt() || state.get_video_generating() {
                return;
            }
            if !require_online_operation(&app, "优化视频提示词") {
                state.set_video_prompt_status("视频提示词优化需要联网，请检查网络后重试".into());
                return;
            }
            let task = match video_prompt_task(&state) {
                Ok(task) => task,
                Err(reason) => {
                    state.set_video_prompt_status(reason.into());
                    return;
                }
            };
            state.set_video_prompt_status("正在整理视频提示词...".into());
            state.set_optimizing_video_prompt(true);
            start_backend_prompt_task(&app, context.clone(), task);
        });
    }
    {
        let weak = app.as_weak();
        state.on_open_video_prompt_editor(move || {
            let Some(app) = weak.upgrade() else { return };
            let state = app.global::<AppState>();
            if state.get_page() == "video-generation" {
                set_video_player_visible(false);
                state.set_video_prompt_expanded_open(true);
            }
        });
    }
    {
        let weak = app.as_weak();
        state.on_video_prompt_edited(move || {
            let Some(app) = weak.upgrade() else { return };
            let state = app.global::<AppState>();
            state.set_video_prompt_status("".into());
            let Some(owner) = context
                .current_user_id
                .lock()
                .unwrap_or_else(|value| value.into_inner())
                .clone()
            else {
                return;
            };
            if state.get_page() != "video-generation" || state.get_video_source_id().is_empty() {
                return;
            }
            store_video_prompt_draft(
                &mut context.store.borrow_mut().prompt_drafts,
                &owner,
                state.get_video_source_id().as_str(),
                state.get_video_prompt().as_str(),
            );
            save_local_store(&app, &context.store.borrow());
        });
    }
}

fn video_prompt_task(state: &AppState) -> std::result::Result<PromptTaskRequest, &'static str> {
    if state.get_page() != "video-generation" || state.get_video_source_id().trim().is_empty() {
        return Err("请先选择需要生成视频的图片");
    }
    let input = state.get_video_prompt().to_string();
    if input.trim().is_empty() {
        return Err("请先填写视频提示词");
    }
    let model_code = state.get_reasoning_model().to_string();
    if model_code.trim().is_empty() {
        return Err("服务端没有可用的提示词模型");
    }
    Ok(PromptTaskRequest {
        model_code,
        task_type: "prompt_optimize",
        prompt: format!(
            "请将以下内容整理为适合图生视频的提示词，优化表述、分段和格式。\
             保持原文语言、主体、场景、风格和约束不变；原文已包含的动作与镜头运动应表述清楚，\
             不要擅自添加人物、情节、运动或改变画面内容。\
             按原文已有信息合理分段，只返回可直接使用的提示词，不要解释、代码块或 Markdown 标记。\n\n{}",
            input.trim()
        ),
        target_language: None,
        optimize: true,
        target: PromptResultTarget::Video {
            source_id: state.get_video_source_id().to_string(),
            input,
        },
        reference_paths: Vec::new(),
    })
}

pub(super) fn store_video_prompt_draft(
    drafts: &mut PromptDrafts,
    owner: &str,
    source_id: &str,
    prompt: &str,
) {
    // Retain the latest video draft per account, without mixing it with image drafts.
    drafts.video_by_owner.insert(
        owner.to_string(),
        VideoPromptDraft {
            source_id: source_id.to_string(),
            prompt: prompt.to_string(),
        },
    );
}

pub(super) fn video_prompt_for_source(
    drafts: &PromptDrafts,
    owner: &str,
    source_id: &str,
    fallback: &str,
) -> String {
    drafts
        .video_by_owner
        .get(owner)
        .filter(|draft| draft.source_id == source_id)
        .map(|draft| draft.prompt.clone())
        .unwrap_or_else(|| fallback.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_prompt_failure_preserves_input_and_does_not_overwrite_quote_status() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().unwrap();
        wire_video_prompt_callbacks(&app, AppContext::default());
        let state = app.global::<AppState>();
        state.set_page("video-generation".into());
        state.set_session_state("online".into());
        state.set_video_source_id("image-a".into());
        state.set_video_prompt("original video prompt".into());
        state.set_prompt("original image prompt".into());
        state.set_reasoning_model("server-prompt-model".into());
        state.set_video_status("quote ready".into());
        state.invoke_optimize_video_prompt();
        assert!(!state.get_optimizing_video_prompt());
        assert!(!state.get_video_prompt_status().is_empty());
        assert_eq!(state.get_video_prompt(), "original video prompt");
        assert_eq!(state.get_prompt(), "original image prompt");
        assert_eq!(state.get_video_status(), "quote ready");
        state.set_video_generating(true);
        state.set_video_prompt_status("unchanged while generating".into());
        state.invoke_optimize_video_prompt();
        assert_eq!(
            state.get_video_prompt_status(),
            "unchanged while generating"
        );
    }

    #[test]
    fn video_prompt_task_uses_existing_api_and_captures_only_the_video_input() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().unwrap();
        let state = app.global::<AppState>();
        state.set_page("video-generation".into());
        state.set_video_source_id("image-a".into());
        state.set_video_prompt("  镜头缓慢推进，保持人物不变。\n  ".into());
        state.set_prompt("unrelated image prompt".into());
        state.set_reasoning_model("server-prompt-model".into());
        let task = video_prompt_task(&state).unwrap();
        assert_eq!(task.task_type, "prompt_optimize");
        assert_eq!(task.model_code, "server-prompt-model");
        assert!(task.prompt.contains("图生视频"));
        assert!(task.prompt.ends_with("镜头缓慢推进，保持人物不变。"));
        assert!(!task.prompt.contains("unrelated image prompt"));
        assert!(task.reference_paths.is_empty());
        assert!(
            matches!(task.target, PromptResultTarget::Video { source_id, input }
            if source_id == "image-a" && input == "  镜头缓慢推进，保持人物不变。\n  ")
        );
        state.set_video_prompt(" \n ".into());
        assert!(video_prompt_task(&state).is_err());
    }

    #[test]
    fn video_prompt_draft_is_scoped_to_account_and_source() {
        let mut drafts = PromptDrafts::default();
        drafts.scene = "image prompt".into();
        store_video_prompt_draft(&mut drafts, "user-a", "image-a", "edited video prompt");
        assert_eq!(
            video_prompt_for_source(&drafts, "user-a", "image-a", "original"),
            "edited video prompt"
        );
        assert_eq!(
            video_prompt_for_source(&drafts, "user-b", "image-a", "original"),
            "original"
        );
        assert_eq!(
            video_prompt_for_source(&drafts, "user-a", "image-b", "original"),
            "original"
        );
        assert_eq!(drafts.scene, "image prompt");
    }
}
