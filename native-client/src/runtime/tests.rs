#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quality_pixel_size_uses_longest_edge_limits() {
        assert_eq!(pixel_dimensions_for("9:16", "1K"), (576, 1024));
        assert_eq!(pixel_dimensions_for("16:9", "1K"), (1024, 576));
        assert_eq!(pixel_dimensions_for("9:16", "2K"), (1152, 2048));
        assert_eq!(pixel_dimensions_for("16:9", "4K"), (4096, 2304));

        assert_eq!(quality_from_actual_dimensions(1023, 1537), "2K");
        assert_eq!(quality_from_actual_dimensions(1024, 1024), "1K");
        assert_eq!(quality_from_actual_dimensions(2048, 1152), "2K");
        assert_eq!(quality_from_actual_dimensions(4096, 2304), "4K");
    }

    #[test]
    fn update_versions_and_download_urls_are_checked_before_prompting() {
        assert!(compare_versions("1.0.6", "1.0.5").is_gt());
        assert!(compare_versions("1.0.5", "1.0.5").is_eq());
        assert!(compare_versions("1.0.4", "1.0.5").is_lt());

        assert!(validated_update_download_url(
            "https://cdn.honeykid.cn/public/artforge_studio/ArtForgeStudio_macos_aarch64.dmg"
        )
        .is_ok());
        assert!(validated_update_download_url("http://cdn.honeykid.cn/update.dmg").is_err());
        assert!(validated_update_download_url("not-a-url").is_err());
    }

    #[test]
    fn update_prompt_has_optional_and_required_paths() {
        let dialog = include_str!("../../ui/dialogs/version-check-dialog.slint");
        let state = include_str!("../../ui/app-state.slint");
        let app = include_str!("../../ui/app.slint");

        assert!(dialog.contains("AppState.update-required"));
        assert!(dialog.contains("\"稍后再说\""));
        assert!(dialog.contains("\"下载更新\""));
        assert!(dialog.contains("\"离线使用\""));
        assert!(state.contains("in-out property <string> update-download-url"));
        assert!(!app.contains("UpdateProgressDialog"));
    }

    #[test]
    fn generation_api_preserves_exact_aspect_ratios() {
        for ratio in [
            "1:1", "3:2", "2:3", "4:3", "3:4", "5:4", "4:5", "16:9", "9:16", "2:1",
            "1:2", "21:9", "9:21",
        ] {
            assert_eq!(api_aspect_ratio(ratio), ratio);
            assert_eq!(client_ratio_from_api(ratio), ratio);
        }

        assert_eq!(client_ratio_from_api("square"), "1:1");
        assert_eq!(client_ratio_from_api("landscape"), "3:2");
        assert_eq!(client_ratio_from_api("portrait"), "2:3");
        assert_eq!(api_aspect_ratio("unsupported"), "1:1");
    }

    #[test]
    fn bigint_balances_and_cursors_remain_decimal_strings() {
        let value = "9007199254740993123";
        let credits: CreditAccount = serde_json::from_value(serde_json::json!({
            "available": value,
            "reserved": "0",
            "lifetime_granted": value,
            "lifetime_spent": "1"
        }))
        .unwrap();
        let meta: ApiMeta = serde_json::from_value(serde_json::json!({
            "next_cursor": value
        }))
        .unwrap();

        assert_eq!(credits.available, value);
        assert_eq!(credits.lifetime_granted, value);
        assert_eq!(meta.next_cursor.as_deref(), Some(value));
    }

    #[test]
    fn generated_images_preserve_provider_bytes_and_dimensions() {
        let source = image::RgbaImage::from_pixel(1254, 1254, image::Rgba([40, 80, 120, 255]));
        let bytes = encode_png_rgba(&source, 1254, 1254).unwrap();
        let (saved, _, width, height) = generated_image_from_bytes(&bytes).unwrap();

        assert_eq!(saved, bytes);
        assert_eq!((width, height), (1254, 1254));
    }

    #[test]
    fn app_contexts_do_not_share_generation_state() {
        let first = AppContext::default();
        let second = AppContext::default();
        insert_active_generation(
            &first,
            ActiveGeneration {
                task_id: "task-1".to_string(),
                category: "character".to_string(),
                ..ActiveGeneration::default()
            },
        );

        assert!(category_is_generating(&first, "character"));
        assert!(!category_is_generating(&second, "character"));
    }

    #[test]
    fn generation_prompt_keeps_selected_controls_and_dimensions() {
        let controls = PromptControls {
            category: "scene".to_string(),
            creation: "free".to_string(),
            style: "realistic".to_string(),
            view: "wide".to_string(),
            weather: "rain".to_string(),
            time: "night".to_string(),
            light: "neon".to_string(),
        };
        let quote = QuoteContext {
            title: String::new(),
            prompt: String::new(),
            ratio: String::new(),
            quality: String::new(),
            width: 0,
            height: 0,
        };

        let prompt = build_generation_prompt(
            "未来城市街道",
            &controls,
            &quote,
            "scene",
            "16:9",
            "2K",
            PromptLanguage::Chinese,
        );

        assert!(prompt.contains("未来城市街道"));
        assert!(prompt.contains("16:9"));
        assert!(prompt.contains("2K"));
    }

    #[test]
    fn slash_prompt_history_uses_latest_unique_local_prompts() {
        let mut prompts = vec!["  recent prompt  ".to_string(), String::new(), "recent prompt".to_string()];
        prompts.extend((0..25).map(|index| format!("prompt-{index}")));

        let history = recent_prompt_history(prompts.iter().map(String::as_str), 20);
        assert_eq!(history.len(), 20);
        assert_eq!(history[0], "recent prompt");
        assert_eq!(history[1], "prompt-0");
        assert_eq!(history[19], "prompt-18");

        let composer = include_str!("../../ui/components/prompt-composer.slint");
        let state = include_str!("../../ui/app-state.slint");
        let sync = include_str!("presentation/sync.rs");

        assert!(state.contains("in-out property <[string]> prompt-history"));
        assert!(state.contains("in-out property <bool> prompt-history-open"));
        assert!(composer.contains("event.text == \"/\""));
        assert!(composer.contains("AppState.prompt == \"\""));
        assert!(composer.contains("AppState.prompt-history-open = true"));
        assert!(composer.contains(
            "root.apply-selected-prompt(AppState.prompt-history[index])"
        ));
        assert!(sync.contains("recent_prompt_history"));
        assert!(sync.contains("20"));
    }

    #[test]
    fn prompt_history_is_a_compact_outside_click_popup() {
        let composer = include_str!("../../ui/components/prompt-composer.slint");
        assert!(composer.contains("history-popup := PopupWindow"));
        assert!(composer.contains("close-policy: close-on-click-outside"));
        assert!(composer.contains("y: root.prompt-input-y() + 32px;"));
        assert!(composer.contains("width: root.width - 48px"));
        assert!(composer.contains("history-popup.show()"));
        assert!(composer.contains("history-popup.close()"));
        assert!(!composer.contains("最近提示词"));
        assert!(!composer.contains("history-close"));
        assert!(composer.contains("horizontal-alignment: left"));
    }

    #[test]
    fn prompt_popups_close_when_their_slash_trigger_is_removed() {
        let composer = include_str!("../../ui/components/prompt-composer.slint");
        let edited_handler = composer
            .split("edited =>")
            .nth(1)
            .and_then(|value| value.split("key-pressed(event)").next())
            .expect("prompt edited handler");

        assert!(edited_handler.contains("self.text != \"/\""));
        assert!(edited_handler.contains("AppState.prompt-history-open = false"));
        assert!(edited_handler.contains("history-popup.close()"));
        assert!(edited_handler.contains("self.text != \"//\""));
        assert!(edited_handler.contains("AppState.custom-prompt-open = false"));
        assert!(edited_handler.contains("custom-prompt-popup.close()"));
    }

    #[test]
    fn slash_prompt_popups_support_keyboard_selection_and_confirmation() {
        let composer = include_str!("../../ui/components/prompt-composer.slint");

        assert!(composer.contains("property <int> prompt-history-selected-index: 0"));
        assert!(composer.contains("property <int> custom-prompt-selected-index: 0"));
        assert_eq!(composer.matches("event.text == Key.DownArrow").count(), 2);
        assert_eq!(composer.matches("event.text == Key.UpArrow").count(), 2);
        assert_eq!(composer.matches("event.text == Key.Escape").count(), 2);
        assert!(composer.contains(
            "AppState.prompt-history[root.prompt-history-selected-index]"
        ));
        assert!(composer.contains(
            "AppState.custom-prompts[root.custom-prompt-selected-index]"
        ));
        assert!(composer.contains("root.scroll-prompt-history-selection-into-view()"));
        assert!(composer.contains("root.scroll-custom-prompt-selection-into-view()"));
        assert!(composer.contains("index == root.prompt-history-selected-index"));
        assert!(composer.contains("index == root.custom-prompt-selected-index"));
    }

    #[test]
    fn prompt_action_status_wraps_below_controls_without_covering_the_editor() {
        let composer = include_str!("../../ui/components/prompt-composer.slint");
        let pill = include_str!("../../ui/components/pill-button.slint");

        assert!(composer.contains("function action-status-wraps() -> bool"));
        assert!(composer.contains("root.action-status-wraps() ? 48px : 20px"));
        assert!(composer.contains("root.action-status-wraps() ? 84px"));
        assert!(pill.contains("clip: true"));
        assert!(pill.contains("wrap: no-wrap"));
        assert!(pill.contains("overflow: elide"));
    }

    #[test]
    fn custom_prompts_are_normalized_deduplicated_and_bounded() {
        let normalized = normalize_custom_prompts(vec![
            "  first prompt  ".to_string(),
            String::new(),
            "first prompt".to_string(),
            "second prompt".to_string(),
        ]);
        assert_eq!(normalized, vec!["first prompt", "second prompt"]);

        let mut store = Store::default();
        assert_eq!(
            save_custom_prompt_to_store(&mut store, "", "  saved prompt  ", "2026-07-21 10:00"),
            SaveCustomPromptResult::Saved
        );
        assert_eq!(
            store.custom_prompt_times.get("saved prompt").map(String::as_str),
            Some("2026-07-21 10:00")
        );
        save_custom_prompt_profile(
            &mut store,
            "",
            "saved prompt",
            CustomPromptProfile {
                name: "Saved name".to_string(),
                category: "scene".to_string(),
                format: "json".to_string(),
                negative_prompt: "blur".to_string(),
                reference_path: "reference.png".to_string(),
            },
        );
        assert_eq!(
            store
                .custom_prompt_profiles
                .get("saved prompt")
                .map(|profile| profile.name.as_str()),
            Some("Saved name")
        );
        assert_eq!(
            save_custom_prompt_to_store(&mut store, "", "saved prompt", "2026-07-21 10:01"),
            SaveCustomPromptResult::Duplicate
        );
        assert_eq!(
            save_custom_prompt_to_store(&mut store, "", "   ", "2026-07-21 10:02"),
            SaveCustomPromptResult::Empty
        );
        assert_eq!(
            save_custom_prompt_to_store(
                &mut store,
                "saved prompt",
                "edited prompt",
                "2026-07-21 10:03",
            ),
            SaveCustomPromptResult::Saved
        );
        assert!(!store.custom_prompt_times.contains_key("saved prompt"));
        assert_eq!(
            store.custom_prompt_times.get("edited prompt").map(String::as_str),
            Some("2026-07-21 10:03")
        );
        save_custom_prompt_profile(
            &mut store,
            "saved prompt",
            "edited prompt",
            CustomPromptProfile {
                name: "Edited name".to_string(),
                ..CustomPromptProfile::default()
            },
        );
        assert!(!store.custom_prompt_profiles.contains_key("saved prompt"));
        assert_eq!(
            store
                .custom_prompt_profiles
                .get("edited prompt")
                .map(|profile| profile.name.as_str()),
            Some("Edited name")
        );
        assert_eq!(
            save_custom_prompt_to_store(&mut store, "missing", "other", "2026-07-21 10:04"),
            SaveCustomPromptResult::Missing
        );
        for index in 0..110 {
            let _ = save_custom_prompt_to_store(
                &mut store,
                "",
                &format!("prompt-{index}"),
                "2026-07-21 10:05",
            );
        }
        assert_eq!(store.custom_prompts.len(), MAX_CUSTOM_PROMPTS);
        assert!(remove_custom_prompt_from_store(&mut store, "prompt-109"));
        assert!(!remove_custom_prompt_from_store(&mut store, "missing prompt"));
    }

    #[test]
    fn double_slash_opens_locally_persisted_custom_prompts() {
        let state = include_str!("../../ui/app-state.slint");
        let app = include_str!("../../ui/app.slint");
        let settings = include_str!("../../ui/pages/settings-page.slint");
        let custom_settings = include_str!("../../ui/components/custom-prompt-settings.slint");
        let custom_dialog = include_str!("../../ui/dialogs/custom-prompt-dialog.slint");
        let composer = include_str!("../../ui/components/prompt-composer.slint");
        let local_store = include_str!("storage/local_store.rs");
        let callbacks = include_str!("callbacks/custom_prompt.rs");

        assert!(state.contains("in-out property <[string]> custom-prompts"));
        assert!(state.contains("in-out property <[CustomPromptItem]> custom-prompt-items"));
        assert!(state.contains("in-out property <bool> custom-prompt-editor-open"));
        assert!(state.contains("callback save-custom-prompt(string, string)"));
        assert!(state.contains("callback remove-custom-prompt(string)"));
        assert!(app.contains("CustomPromptDialog"));
        assert!(settings.contains("CustomPromptSettings"));
        assert!(settings.contains("自定义提示词"));
        assert!(custom_settings.contains("text: AppState.en ? \"Add\" : \"新增\""));
        assert!(custom_settings.contains("AppState.begin-new-custom-prompt()"));
        assert!(custom_settings.contains("for item in AppState.custom-prompt-items"));
        assert!(custom_settings.contains("text: item.name"));
        assert!(custom_settings.contains("text: item.preview"));
        assert!(custom_settings.contains("clip: true"));
        assert!(custom_settings.contains("text: item.time"));
        assert!(custom_settings.contains("assets/icons/edit.svg"));
        assert!(custom_settings.contains("AppState.pending-delete-kind = \"custom-prompt\""));
        assert!(custom_settings.contains("AppState.delete-confirm-open = true"));
        assert!(custom_dialog.contains("AppState.save-custom-prompt"));

        assert!(composer.contains("event.text == \"/\" && AppState.prompt == \"/\""));
        assert!(composer.contains("AppState.prompt = \"//\";"));
        let double_slash_handler = composer
            .split("event.text == \"/\" && AppState.prompt == \"/\"")
            .nth(1)
            .and_then(|value| value.split("if event.text == Key.Return").next())
            .expect("double slash handler");
        assert!(double_slash_handler.contains("return accept;"));
        assert!(double_slash_handler.contains("prompt-input.set-selection-offsets(2, 2);"));
        let write_position = double_slash_handler
            .find("AppState.prompt = \"//\";")
            .expect("double slash value assignment");
        let cursor_position = double_slash_handler
            .find("prompt-input.set-selection-offsets(2, 2);")
            .expect("double slash cursor assignment");
        assert!(write_position < cursor_position);
        assert!(!double_slash_handler.contains("event.text == Key.Backspace"));
        assert!(composer.contains("history-popup.close()"));
        assert!(composer.contains("custom-prompt-popup.show()"));
        let composer_normalized = composer.replace("\r\n", "\n");
        assert!(composer_normalized.contains(
            "custom-prompt-popup.show();\n                        prompt-input.focus();"
        ));
        assert!(composer_normalized
            .contains("history-popup.show();\n                        prompt-input.focus();"));
        assert!(composer.contains("for preview[index] in AppState.custom-prompt-previews"));
        assert!(composer.contains("close-policy: close-on-click-outside"));

        assert!(local_store.contains("custom_prompts: store.custom_prompts.clone()"));
        assert!(local_store.contains("custom_prompt_times: store.custom_prompt_times.clone()"));
        assert!(local_store.contains("normalize_custom_prompts(data.custom_prompts)"));
        assert!(callbacks.contains("save_local_store(&app, &store.borrow())"));
        assert!(callbacks.contains("state.on_save_custom_prompt"));
        assert!(callbacks.contains("state.set_custom_prompt_editor_open(false)"));
        assert!(callbacks.contains("state.on_begin_new_custom_prompt"));
        assert!(callbacks.contains("state.on_begin_edit_custom_prompt"));
        assert!(callbacks.contains("state.on_choose_custom_prompt_reference"));
        assert!(local_store.contains(
            "custom_prompt_profiles: store.custom_prompt_profiles.clone()"
        ));
    }

    #[test]
    fn custom_prompt_editor_uses_the_structured_reference_form() {
        let dialog = include_str!("../../ui/dialogs/custom-prompt-dialog.slint");
        let state = include_str!("../../ui/app-state.slint");

        for field in [
            "custom-prompt-name",
            "custom-prompt-category",
            "custom-prompt-format",
            "custom-prompt-negative",
            "custom-prompt-reference-path",
            "custom-prompt-reference-image",
        ] {
            assert!(state.contains(field), "missing state field {field}");
        }
        assert!(dialog.contains("提示词名称 *"));
        assert!(dialog.contains("PromptCategorySelect"));
        assert!(dialog.contains("上传风格参考图"));
        assert!(dialog.contains("AI 分析生成风格"));
        assert!(dialog.contains("保存格式"));
        assert!(dialog.contains("提示词内容 *"));
        assert!(dialog.contains("反向提示词（仅 JSON 格式有效）"));
        assert!(dialog.contains("AppState.choose-custom-prompt-reference()"));
        assert!(dialog.contains("AppState.clear-custom-prompt-reference()"));
    }

    #[test]
    fn custom_prompt_editor_allows_ime_to_handle_composition_keys() {
        let dialog = include_str!("../../ui/dialogs/custom-prompt-dialog.slint");
        let prompt_input = dialog
            .split("prompt-input := TextInput")
            .nth(1)
            .and_then(|value| value.split("if AppState.custom-prompt-input").next())
            .expect("custom prompt content input");

        assert!(dialog.contains("init => { prompt-name-input.focus(); }"));
        assert!(dialog.matches("input-type: text;").count() >= 3);
        assert!(!prompt_input.contains("key-pressed(event)"));
        assert!(!prompt_input.contains("root.save-prompt()"));
    }

    #[test]
    fn windows_file_drag_runs_on_the_pointer_thread_after_releasing_capture() {
        let drag = include_str!("../drag_preview.rs");
        let handler = drag
            .split("pub fn start_thumbnail_file_drag(path: PathBuf) -> bool")
            .nth(1)
            .and_then(|value| value.split("#[cfg(not(target_os = \"windows\"))]").next())
            .expect("Windows file drag handler");

        assert!(handler.contains("ReleaseCapture()"));
        assert!(handler.contains("windows_file_drag::run(path).is_ok()"));
        assert!(!handler.contains("std::thread::spawn"));
        assert!(drag.contains("DoDragDrop(&data_object, &drop_source, DROPEFFECT_COPY, &mut effect).ok()"));
    }

    #[test]
    fn thumbnail_file_drag_is_not_intercepted_by_slint_internal_drag_area() {
        let thumbnail = include_str!("../../ui/components/thumbnail-card.slint");

        assert!(!thumbnail.contains("DragArea {"));
        assert!(thumbnail.contains("Math.abs(hover.mouse-x - hover.pressed-x) < 7px"));
        assert!(thumbnail.contains("AppState.start-thumbnail-file-drag(drag-data)"));
    }

    #[test]
fn generation_loading_thumbnail_exposes_a_stop_button() {
    let card = include_str!("../../ui/components/generation-loading-card.slint");
    let state = include_str!("../../ui/app-state.slint");
    let callbacks = include_str!("callbacks/generation.rs");

        assert!(card.contains("stop-button := Rectangle"));
        assert!(card.contains("stop-touch := TouchArea"));
        assert!(card.contains("AppState.stop-generation()"));
        assert!(card.contains("AppTheme.danger"));
    assert!(state.contains("callback stop-generation();"));
    assert!(callbacks.contains("state.on_stop_generation"));
}

#[test]
fn loading_dots_use_staggered_bouncing_motion() {
    let dots = include_str!("../../ui/components/loading-dots.slint");

    assert!(dots.contains("dot-one := Rectangle"));
    assert!(dots.contains("dot-two := Rectangle"));
    assert!(dots.contains("dot-three := Rectangle"));
    assert!(dots.contains("interval: 120ms"));
    assert!(dots.matches("animate y").count() >= 3);
}

    #[test]
    fn legacy_double_slash_prompt_drafts_are_cleared_without_touching_real_prompts() {
        let mut drafts = PromptDrafts {
            scene: "//".to_string(),
            ui: "keep // inside this prompt".to_string(),
            ..PromptDrafts::default()
        };

        assert!(normalize_reserved_prompt_drafts(&mut drafts));
        assert_eq!(drafts.scene, "");
        assert_eq!(drafts.ui, "keep // inside this prompt");
        assert!(!normalize_reserved_prompt_drafts(&mut drafts));
    }

    #[test]
    fn prompt_popups_show_ten_single_line_previews_without_losing_full_values() {
        assert_eq!(
            single_line_prompt_preview("first line\nsecond\tline  end"),
            "first line second line end"
        );

        let composer = include_str!("../../ui/components/prompt-composer.slint");
        assert!(composer.matches("min(10, AppState.").count() >= 2);
        assert_eq!(composer.matches("wrap: no-wrap;").count(), 3);
        assert!(composer.contains(
            "root.apply-selected-prompt(AppState.prompt-history[index])"
        ));
        assert!(composer.contains(
            "root.apply-selected-prompt(AppState.custom-prompts[index])"
        ));
        assert!(composer.contains("viewport-height: AppState.prompt-history.length * 32px"));
        assert!(composer.contains("viewport-height: AppState.custom-prompts.length * 32px"));
    }

    #[test]
    fn custom_prompt_selection_writes_after_focus_and_empty_state_links_to_creation() {
        let composer = include_str!("../../ui/components/prompt-composer.slint");
        let state = include_str!("../../ui/app-state.slint");
        let settings = include_str!("../../ui/pages/settings-page.slint");

        let apply_prompt = composer
            .split("function apply-selected-prompt(value: string)")
            .nth(1)
            .and_then(|value| value.split("function ").next())
            .expect("selected prompt helper");
        let focus_position = apply_prompt
            .find("prompt-input.focus()")
            .expect("prompt input focus");
        let write_position = apply_prompt
            .find("AppState.prompt = value")
            .expect("prompt value assignment");
        assert!(focus_position < write_position);
        assert!(composer.contains("暂无自定义提示词，点击创建"));
        assert!(composer.contains("AppState.settings-section = \"prompts\""));
        assert!(composer.contains("AppState.navigate(\"settings\")"));
        assert!(state.contains("in-out property <string> settings-section: \"basic\""));
        assert!(settings.contains("AppState.settings-section"));
    }

    #[test]
    fn enter_confirms_inputs_and_alt_enter_keeps_prompt_line_breaks() {
        let field = include_str!("../../ui/components/field.slint");
        let auth = include_str!("../../ui/dialogs/auth-dialog.slint");
        let invoice = include_str!("../../ui/dialogs/invoice-dialog.slint");
        let prompt = include_str!("../../ui/components/prompt-composer.slint");

        assert!(field.contains("callback accepted();"));
        assert!(field.contains("accepted => { root.accepted(); }"));

        assert!(auth.contains("function confirm-auth()"));
        assert_eq!(auth.matches("accepted => { root.confirm-auth(); }").count(), 2);

        assert!(invoice.contains("function submit-form()"));
        assert_eq!(invoice.matches("accepted => { root.submit-form(); }").count(), 3);

        assert!(prompt.contains("event.text == Key.Return"));
        assert!(prompt.contains("event.modifiers.alt"));
        assert!(prompt.contains("return reject"));
        assert!(prompt.contains("AppState.generate()"));
        assert!(prompt.contains("return accept"));
    }

    #[test]
    fn long_prompt_input_scrolls_inside_its_fixed_viewport() {
        let prompt = include_str!("../../ui/components/prompt-composer.slint");

        assert!(prompt.contains("prompt-scroll := ScrollView"));
        assert!(prompt.contains("viewport-height: max(self.visible-height, prompt-input.preferred-height);"));
        assert!(prompt.contains("page-height: prompt-scroll.visible-height;"));
        assert!(prompt.contains("cursor-position-changed(position)"));
        assert!(prompt.contains("prompt-scroll.viewport-y"));
    }

    #[test]
    fn studio_generation_settings_use_three_compact_dropdowns_and_a_taller_prompt() {
        let chooser = include_str!("../../ui/components/inline-card-chooser.slint");
        let panel = include_str!("../../ui/components/studio-work-panel.slint");
        let prompt = include_str!("../../ui/components/prompt-composer.slint");

        assert_eq!(chooser.matches("\n        CompactSelectButton {").count(), 3);
        assert!(chooser.contains("ratio-popup := PopupWindow"));
        assert!(chooser.contains("quality-popup := PopupWindow"));
        assert!(chooser.contains("count-popup := PopupWindow"));
        assert!(chooser.contains("\"比例 · \""));
        assert!(chooser.contains("\"清晰度 · \""));
        assert!(chooser.contains("\"张数 · \""));
        assert!(chooser.contains("disabled: AppState.asset-type == \"action-sequence\""));
        assert!(chooser.contains("AppState.membership-max-quality == \"1K\""));
        assert!(chooser.contains("AppState.membership-max-quality != \"4K\""));
        assert!(panel.contains("InlineCardChooser {"));
        assert!(!panel.contains("viewport-height: AppState.ratio-more-open"));
        assert!(panel.contains("parent.height - 254px"));
        assert!(prompt.contains("? 650px : 600px"));
    }

    #[test]
    fn infinite_canvas_blank_click_clears_node_interactions_without_breaking_pan() {
        let page = include_str!("../../ui/pages/infinite-canvas-page.slint");

        assert!(page.contains("function clear-node-interaction()"));
        assert!(page.contains("AppState.canvas-node-info-open = false"));
        assert!(page.contains("AppState.clear-canvas-selection()"));
        assert!(page.contains("Math.abs(self.mouse-x - self.start-pointer-x) < 4px"));
        assert!(page.contains("Math.abs(self.mouse-y - self.start-pointer-y) < 4px"));
        assert!(page.contains("} else if root.temporary-pan-active"));
        assert!(page.contains("root.clear-node-interaction();"));
        assert!(page.contains("&& AppState.canvas-selected-id == root.note.id"));
    }

    #[test]
    fn auth_dialog_can_be_closed_without_changing_auth_state_contract() {
        let auth = include_str!("../../ui/dialogs/auth-dialog.slint");
        assert!(auth.contains("import { DialogCloseButton }"));
        assert!(auth.contains("DialogCloseButton"));
        assert!(auth.contains("AppState.auth-open = false"));
    }

    #[test]
    fn model_picker_height_tracks_visible_options() {
        let picker = include_str!("../../ui/components/model-picker.slint");
        let state = include_str!("../../ui/app-state.slint");
        let sync = include_str!("presentation/sync.rs");

        assert!(picker.contains("height: root.popup-height();"));
        assert!(picker.contains("function option-count() -> int"));
        assert!(picker.contains("12px + root.option-count() * 42px"));
        assert!(picker.contains("AppState.model-image-options"));
        assert!(picker.contains("AppState.model-reasoning-options"));
        assert!(!picker.contains("visible: group.kind == root.kind"));
        assert!(state.contains("model-image-options"));
        assert!(state.contains("model-reasoning-options"));
        assert!(sync.contains("model_picker_options(store, \"image\")"));
        assert!(sync.contains("model_picker_options(store, \"reasoning\")"));
    }

    #[test]
    fn generation_model_pickers_are_left_aligned() {
        let top_bar = include_str!("../../ui/components/top-bar.slint").replace("\r\n", "\n");

        assert!(top_bar.contains("x: 18px;\n            y: 0px;"));
        assert!(top_bar.contains("width: max(360px, parent.width - 18px - root.actions-width() - 32px);"));
        assert!(top_bar.contains("(root.width - 18px - root.actions-width() - 70px) / 2"));
        assert!(top_bar.contains("x: 0px;\n                    y: 6px;\n                    kind: \"image\";"));
        assert!(top_bar.contains("x: root.model-picker-width() + 18px;\n                    y: 6px;\n                    kind: \"reasoning\";"));
        assert!(!top_bar.contains("root.models-width()"));
    }

    #[test]
    fn generated_filename_removes_path_separators() {
        let value = sanitize_filename("角色/场景\\测试:*?");
        assert!(!value.contains('/'));
        assert!(!value.contains('\\'));
        assert!(!value.contains(':'));
        assert!(!value.contains('*'));
        assert!(!value.contains('?'));
    }

    #[test]
    fn notification_page_distinguishes_success_details_from_failure_reasons() {
        let page = include_str!("../../ui/pages/notifications-page.slint");
        let api = include_str!("api/notifications.rs");
        let callbacks = include_str!("callbacks/notification.rs");

        assert!(page.contains("text: item.success"));
        assert!(page.contains("\"成功说明：\" + item.reason"));
        assert!(page.contains("\"失败原因：\" + item.reason"));
        assert!(page.contains("color: item.success ? AppTheme.success : AppTheme.danger"));
        assert!(page.contains("AppState.pending-delete-kind = \"notification\""));
        assert!(page.contains("AppState.pending-delete-kind = \"notifications-all\""));
        assert!(page.contains("一键删除"));
        assert!(api.contains("Method::DELETE"));
        assert!(api.contains("/v1/notifications/{id}"));
        assert!(api.contains("/v1/notifications"));
        assert!(callbacks.contains("store.notifications.retain(|item| item.id != id)"));
        assert!(callbacks.contains("store.notifications.clear()"));

        let failed = ServerNotification {
            id: "failed-generation".to_string(),
            notification_type: "generation.settled".to_string(),
            title: "生成失败".to_string(),
            body: "任务未能完成，未消耗的积分已经退回。".to_string(),
            metadata: serde_json::json!({ "status": "failed" }),
            created_at: "2026-07-20T00:00:00Z".to_string(),
            read_at: None,
        };
        assert!(!notification_is_success(&failed));

        let completed = ServerNotification {
            id: "completed-generation".to_string(),
            notification_type: "generation.settled".to_string(),
            title: "生成完成".to_string(),
            body: "图片已经生成。".to_string(),
            metadata: serde_json::json!({ "status": "succeeded" }),
            created_at: "2026-07-20T00:00:00Z".to_string(),
            read_at: None,
        };
        assert!(notification_is_success(&completed));
    }

    #[test]
    fn permanent_delete_actions_require_shared_confirmation() {
        let state = include_str!("../../ui/app-state.slint");
        let dialog = include_str!("../../ui/dialogs/delete-confirm.slint");
        let prompts = include_str!("../../ui/components/custom-prompt-settings.slint");
        let notifications = include_str!("../../ui/pages/notifications-page.slint");
        let viewer_callbacks = include_str!("callbacks/viewer.rs");

        assert!(state.contains("in-out property <string> pending-delete-kind"));
        assert!(dialog.contains("AppState.pending-delete-kind == \"custom-prompt\""));
        assert!(dialog.contains("AppState.pending-delete-kind == \"notification\""));
        assert!(dialog.contains("AppState.pending-delete-kind == \"notifications-all\""));
        assert!(dialog.contains("AppState.pending-delete-kind == \"canvas-link\""));
        assert!(dialog.contains("AppState.remove-custom-prompt(AppState.pending-delete-id)"));
        assert!(dialog.contains("AppState.delete-notification(AppState.pending-delete-id)"));
        assert!(dialog.contains("AppState.clear-all-notifications()"));
        assert!(dialog.contains("AppState.remove-canvas-link(AppState.pending-delete-id)"));
        assert!(dialog.contains("AppState.confirm-delete()"));

        assert!(prompts.contains("AppState.pending-delete-kind = \"custom-prompt\""));
        assert!(prompts.contains("AppState.delete-confirm-open = true"));
        assert!(!prompts.contains(
            "clicked => { AppState.remove-custom-prompt(item.content); }"
        ));
        assert!(notifications.contains("AppState.pending-delete-kind = \"notification\""));
        assert!(notifications.contains("AppState.pending-delete-kind = \"notifications-all\""));
        assert!(!notifications.contains(
            "clicked => { AppState.delete-notification(item.id); }"
        ));
        assert!(!notifications.contains(
            "clicked => { AppState.clear-all-notifications(); }"
        ));
        assert!(viewer_callbacks.contains("state.set_pending_delete_kind(\"asset\".into())"));
    }

    #[test]
    fn model_management_is_a_settings_section() {
        let app = include_str!("../../ui/app.slint");
        let sidebar = include_str!("../../ui/components/sidebar.slint");
        let settings = include_str!("../../ui/pages/settings-page.slint");
        let model_page = include_str!("../../ui/pages/models-page.slint");
        let model_picker = include_str!("../../ui/components/model-picker.slint");
        let required_dialog = include_str!("../../ui/dialogs/model-required-dialog.slint");

        assert!(!app.contains("AppState.page == \"models\""));
        assert!(!sidebar.contains("page: \"models\""));
        assert!(settings.contains("import { ModelsPage }"));
        assert!(settings.contains("AppState.settings-section == \"models\""));
        assert!(settings.contains("ModelsPage"));
        assert!(settings.contains("AppState.catalog-models.length * 148px"));
        assert!(!model_page.contains("ScrollView"));

        for source in [model_picker, required_dialog] {
            assert!(source.contains("AppState.settings-section = \"models\""));
            assert!(source.contains("AppState.navigate(\"settings\")"));
            assert!(!source.contains("AppState.navigate(\"models\")"));
        }
    }

    #[test]
    fn infinite_canvas_is_a_local_workspace_below_the_workbench() {
        let app = include_str!("../../ui/app.slint");
        let state = include_str!("../../ui/app-state.slint");
        let types = include_str!("../../ui/types.slint");
        let sidebar = include_str!("../../ui/components/sidebar.slint");
        let glyph = include_str!("../../ui/components/nav-glyph.slint");
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let page = std::fs::read_to_string(manifest.join("ui/pages/infinite-canvas-page.slint"))
            .unwrap_or_default();
        let callbacks = std::fs::read_to_string(manifest.join("src/runtime/callbacks/infinite_canvas.rs"))
            .unwrap_or_default();
        let local_store = include_str!("storage/local_store.rs");
        let sync = include_str!("presentation/sync.rs");

        let workbench = sidebar.find("CategoryWorkspaceMenu {").expect("workbench menu");
        let canvas = sidebar.find("page: \"canvas\"").expect("canvas nav item");
        let assets = sidebar.find("page: \"assets\"").expect("assets nav item");
        assert!(workbench < canvas && canvas < assets);
        assert!(app.contains("import { InfiniteCanvasPage }"));
        assert!(app.contains("AppState.page == \"canvas\""));
        assert!(glyph.contains("root.kind == \"canvas\""));
        assert!(types.contains("export struct CanvasNote"));
        assert!(types.contains("export struct CanvasLink"));
        assert!(types.contains("linked-input: string"));
        assert!(types.contains("kind: string"));
        assert!(types.contains("width: float"));
        assert!(types.contains("height: float"));
        assert!(state.contains("in-out property <[CanvasNote]> canvas-notes"));
        assert!(state.contains("in-out property <[CanvasLink]> canvas-links"));
        assert!(state.contains("callback add-canvas-node(string, float, float)"));
        assert!(state.contains("callback update-canvas-node(string, string, float, float)"));
        assert!(state.contains("callback remove-canvas-node(string)"));
        assert!(state.contains("callback finish-canvas-link(string, float, float, float) -> string"));
        assert!(state.contains("callback remove-canvas-link(string)"));
        assert!(state.contains("callback undo-canvas()"));
        assert!(state.contains("callback redo-canvas()"));

        assert!(page.contains("scroll-event(event)"));
        assert!(page.contains("root.zoom-percent"));
        assert!(page.contains("root.pan-x"));
        assert!(page.contains("root.pan-y"));
        for kind in ["text", "image"] {
            assert!(page.contains(&format!("root.add-node(\"{kind}\")")));
        }
        for kind in ["video", "audio"] {
            assert!(!page.contains(&format!("root.add-node(\"{kind}\")")));
        }
        assert!(page.contains("AppState.group-canvas-selection"));
        assert!(page.contains("AppState.undo-canvas()"));
        assert!(page.contains("AppState.redo-canvas()"));
        assert!(page.contains("canvas-minimap-open"));
        assert!(page.contains("canvas-grid-style"));
        assert!(page.contains("canvas-show-image-info"));
        assert!(page.contains("zoom-track"));
        assert!(page.contains("for note in AppState.canvas-notes"));
        assert!(page.contains("for link in AppState.canvas-links"));
        assert!(page.contains("AppState.update-canvas-node"));
        assert!(page.contains("AppState.pending-delete-kind = \"canvas-note\""));
        assert!(include_str!("../../ui/dialogs/delete-confirm.slint")
            .contains("AppState.remove-canvas-node(AppState.pending-delete-id)"));

        assert!(callbacks.contains("state.on_add_canvas_node"));
        assert!(callbacks.contains("state.on_update_canvas_node"));
        assert!(callbacks.contains("state.on_remove_canvas_node"));
        assert!(callbacks.contains("state.on_finish_canvas_link"));
        assert!(callbacks.contains("state.on_remove_canvas_link"));
        assert!(callbacks.contains("state.on_undo_canvas"));
        assert!(callbacks.contains("state.on_redo_canvas"));
        assert!(callbacks.contains("CanvasController"));
        assert!(callbacks.contains("save_local_store"));
        assert!(local_store.contains("canvas_notes: store.canvas_notes.clone()"));
        assert!(local_store.contains("canvas_links: store.canvas_links.clone()"));
        assert!(local_store.contains("store_mut.canvas_notes = data.canvas_notes"));
        assert!(sync.contains("push_canvas_notes(app, store)"));
    }

    #[test]
    fn legacy_canvas_notes_default_to_top_level_and_unselected() {
        let note: CanvasNoteData = serde_json::from_str(
            r#"{"id":"n1","kind":"text","content":"","x":10.0,"y":20.0,"width":320.0,"height":210.0}"#,
        )
        .expect("legacy canvas note");

        assert_eq!(note.parent_group_id, "");
        assert_eq!(note.z_index, 0);
        assert!(!note.selected);
    }

    #[test]
    fn infinite_canvas_exposes_multi_selection_commands() {
        let state = include_str!("../../ui/app-state.slint");
        let callbacks = include_str!("callbacks/infinite_canvas.rs");

        for declaration in [
            "in-out property <int> canvas-selected-count: 0",
            "callback select-canvas-node(string, bool)",
            "callback select-canvas-rect(float, float, float, float, bool)",
            "callback clear-canvas-selection()",
            "callback select-all-canvas-nodes()",
            "callback move-canvas-selection(float, float)",
            "callback copy-canvas-selection()",
            "callback paste-canvas-selection(float, float)",
            "callback duplicate-canvas-selection()",
            "callback remove-canvas-selection()",
            "callback group-canvas-selection(float, float)",
            "callback ungroup-canvas-selection()",
        ] {
            assert!(state.contains(declaration), "missing {declaration}");
        }
        for registration in [
            "on_select_canvas_node",
            "on_select_canvas_rect",
            "on_clear_canvas_selection",
            "on_select_all_canvas_nodes",
            "on_move_canvas_selection",
            "on_copy_canvas_selection",
            "on_paste_canvas_selection",
            "on_duplicate_canvas_selection",
            "on_remove_canvas_selection",
            "on_group_canvas_selection",
            "on_ungroup_canvas_selection",
        ] {
            assert!(
                callbacks.contains(registration),
                "missing {registration}"
            );
        }
    }

    #[test]
    fn infinite_canvas_selection_and_pan_modes_use_desktop_shortcuts() {
        let page = include_str!("../../ui/pages/infinite-canvas-page.slint");

        for interaction in [
            "marquee-active",
            "marquee-start-x",
            "marquee-start-y",
            "space-pan-active",
            "temporary-pan-active",
            "AppState.select-canvas-rect",
            "AppState.move-canvas-selection",
            "event.modifiers.control",
            "Key.Backspace",
            "Key.Delete",
            "Key.Escape",
            "Key.Space",
            "root.focus-selection()",
        ] {
            assert!(page.contains(interaction), "missing {interaction}");
        }
        assert!(page.contains("AppState.canvas-tool == \"pan\""));
        assert!(include_str!("../../ui/app-state.slint")
            .contains("in-out property <string> canvas-tool: \"pan\""));
        assert!(!page.contains("label: AppState.en ? \"Select\" : \"选择\""));
        assert!(page.contains(
            "AppState.select-canvas-node(root.note.id, event.modifiers.shift);"
        ));
        assert!(page.contains("root.marquee-additive = event.modifiers.shift;"));
        assert!(!page.contains(
            "AppState.select-canvas-node(root.note.id, event.modifiers.control);"
        ));
        assert!(!page.contains("root.marquee-additive = event.modifiers.control;"));
        assert!(page.contains("link.source-selected"));
        assert!(page.contains("link.target-selected"));
    }

    #[test]
    fn infinite_canvas_groups_are_nested_resizable_containers() {
        let state = include_str!("../../ui/app-state.slint");
        let page = include_str!("../../ui/pages/infinite-canvas-page.slint");
        let callbacks = include_str!("callbacks/infinite_canvas.rs");
        let sync = include_str!("presentation/sync.rs");

        assert!(state.contains("callback resize-canvas-group(string, float, float)"));
        assert!(callbacks.contains("on_resize_canvas_group"));
        assert!(page.contains("group-resize-touch"));
        assert!(page.contains("nwse-resize"));
        assert!(page.contains("AppState.resize-canvas-group"));
        assert!(page.contains("AppState.group-canvas-selection"));
        assert!(page.contains("AppState.ungroup-canvas-selection"));
        assert!(sync.contains("group_depth"));
    }

    #[test]
    fn infinite_canvas_supports_link_highlight_and_atomic_reconnect() {
        let state = include_str!("../../ui/app-state.slint");
        let page = include_str!("../../ui/pages/infinite-canvas-page.slint");
        let callbacks = include_str!("callbacks/infinite_canvas.rs");

        assert!(state.contains("canvas-link-hover-target-id"));
        assert!(state.contains("canvas-link-hover-valid"));
        assert!(state.contains("callback preview-canvas-link-target"));
        assert!(state.contains("callback canvas-input-link(string) -> string"));
        assert!(state.contains("callback finish-canvas-reconnect"));
        assert!(page.contains("root.begin-reconnect"));
        assert!(page.contains("root.finish-reconnect"));
        assert!(page.contains("connection-replacing-link-id"));
        assert!(page.contains("AppState.canvas-link-hover-valid ? AppTheme.accent : #e5484d"));
        assert!(callbacks.contains("connect_nodes"));
        assert!(callbacks.contains("on_finish_canvas_reconnect"));
    }

    #[test]
    fn infinite_canvas_connection_search_is_world_anchored_and_keyboard_accessible() {
        let state = include_str!("../../ui/app-state.slint");
        let page = include_str!("../../ui/pages/infinite-canvas-page.slint");
        let callbacks = include_str!("callbacks/infinite_canvas.rs");

        for property in [
            "node-search-open",
            "node-search-query",
            "node-search-source-id",
            "node-search-world-x",
            "node-search-world-y",
            "node-search-index",
        ] {
            assert!(page.contains(property), "missing {property}");
        }
        assert!(state.contains("canvas-node-search-results"));
        assert!(state.contains("callback search-canvas-node-types"));
        assert!(state.contains("callback add-connected-canvas-node"));
        assert!(page.contains("Key.DownArrow"));
        assert!(page.contains("Key.UpArrow"));
        assert!(page.contains("Key.Return"));
        assert!(page.contains("Key.Escape"));
        assert!(page.contains("root.pan-x + root.node-search-world-x"));
        assert!(!page.contains("node-search-world-x, parent.width"));
        assert!(callbacks.contains("on_search_canvas_node_types"));
        assert!(callbacks.contains("on_add_connected_canvas_node"));
    }

    #[test]
    fn infinite_canvas_reports_capacity_without_mutating_the_server_contract() {
        let callbacks = include_str!("callbacks/infinite_canvas.rs");
        let state = include_str!("../../ui/app-state.slint");

        assert!(callbacks.contains("const MAX_CANVAS_NODES: usize = 200"));
        assert!(callbacks.contains("const MAX_CANVAS_LINKS: usize = 400"));
        assert!(callbacks.contains("Canvas limit reached (200 nodes / 400 connections)."));
        assert!(callbacks.contains("画布已达到上限（200 个节点 / 400 条连线）。"));
        assert!(!state.contains("server-canvas"));
        assert!(!state.contains("upload-canvas"));
    }

    #[test]
    fn invalid_canvas_group_relationships_are_removed_without_moving_nodes() {
        let mut notes = vec![
            CanvasNoteData {
                id: "group-a".into(),
                kind: "group".into(),
                parent_group_id: "group-b".into(),
                x: 10.0,
                y: 20.0,
                ..CanvasNoteData::default()
            },
            CanvasNoteData {
                id: "group-b".into(),
                kind: "group".into(),
                parent_group_id: "group-a".into(),
                ..CanvasNoteData::default()
            },
            CanvasNoteData {
                id: "node".into(),
                parent_group_id: "missing".into(),
                x: 30.0,
                y: 40.0,
                ..CanvasNoteData::default()
            },
        ];

        normalize_canvas_groups(&mut notes);

        assert!(notes[0].parent_group_id.is_empty() || notes[1].parent_group_id.is_empty());
        assert!(notes[2].parent_group_id.is_empty());
        assert_eq!((notes[0].x, notes[0].y), (10.0, 20.0));
        assert_eq!((notes[2].x, notes[2].y), (30.0, 40.0));
    }

    #[test]
    fn infinite_canvas_nodes_drag_from_their_entire_surface_until_editing() {
        let page = include_str!("../../ui/pages/infinite-canvas-page.slint");
        let node = page
            .split("component CanvasNodeCard")
            .nth(1)
            .and_then(|value| value.split("export component InfiniteCanvasPage").next())
            .expect("canvas node component");

        assert!(node.contains("in-out property <bool> editing: false"));
        assert!(node.contains("node-drag-touch := TouchArea"));
        assert!(node.contains("width: parent.width"));
        assert!(node.contains("height: parent.height"));
        assert!(node.contains("root.drag-offset-x"));
        assert!(node.contains("root.drag-offset-y"));
        assert!(node.contains("root.commit-position()"));
        assert!(node.contains("if !root.editing"));
        assert!(node.contains("&& root.editing"));
        assert!(node.contains("&& AppState.canvas-selected-id == root.note.id: TextInput"));
        assert!(node.contains("source: @image-url(\"../../assets/icons/edit.svg\")"));
    }

    #[test]
    fn infinite_canvas_node_press_updates_selection_without_replacing_the_drag_source_model() {
        let callbacks = include_str!("callbacks/infinite_canvas.rs");
        let handler = callbacks
            .split("state.on_select_canvas_node")
            .nth(1)
            .and_then(|value| value.split("state.on_select_canvas_rect").next())
            .expect("canvas node selection handler");

        assert!(handler.contains("sync_canvas_selection_rows(&app, &store_mut)"));
        assert!(!handler.contains("sync_canvas_selection(&app, &store_mut)"));
        assert!(callbacks.contains("canvas_notes.set_row_data"));
        assert!(callbacks.contains("canvas_links.set_row_data"));
    }

    #[test]
    fn infinite_canvas_only_offers_text_and_image_media_nodes_for_new_work() {
        let page = include_str!("../../ui/pages/infinite-canvas-page.slint");
        let callbacks = include_str!("callbacks/infinite_canvas.rs");
        let toolbar = page
            .split("toolbar := Rectangle")
            .nth(1)
            .and_then(|value| value.split("if root.appearance-open").next())
            .expect("canvas toolbar");
        let search = callbacks
            .split("state.on_search_canvas_node_types")
            .nth(1)
            .and_then(|value| value.split("state.on_add_connected_canvas_node").next())
            .expect("canvas node search handler");

        assert!(!toolbar.contains("root.add-node(\"video\")"));
        assert!(!toolbar.contains("root.add-node(\"audio\")"));
        assert!(toolbar.contains("root.add-node(\"text\")"));
        assert!(toolbar.contains("root.add-node(\"image\")"));
        assert!(!search.contains("(\"video\","));
        assert!(!search.contains("(\"audio\","));
    }

    #[test]
    fn infinite_canvas_image_and_text_info_actions_open_the_shared_node_dialog() {
        let page = include_str!("../../ui/pages/infinite-canvas-page.slint");
        let state = include_str!("../../ui/app-state.slint");
        let callbacks = include_str!("callbacks/infinite_canvas.rs");
        let node = page
            .split("component CanvasNodeCard")
            .nth(1)
            .and_then(|value| value.split("export component InfiniteCanvasPage").next())
            .expect("canvas node component");

        assert!(state.contains("callback show-canvas-node-info(string)"));
        assert!(state.contains("canvas-node-info-open"));
        assert!(page.contains("component CanvasNodeInfoDialog"));
        assert!(page.contains("AppState.canvas-node-info-tab == \"json\""));
        assert!(page.contains("AppState.canvas-node-info-width + \" x \""));
        assert!(page.contains("AppState.canvas-node-info-x + \", \""));
        assert!(node.matches("AppState.show-canvas-node-info(root.note.id)").count() >= 2);
        assert!(callbacks.contains("state.on_show_canvas_node_info"));
        assert!(callbacks.contains("serde_json::to_string_pretty"));
        assert!(callbacks.contains("\"status\": \"idle\""));
    }

    #[test]
    fn infinite_canvas_uploaded_image_preview_is_persisted_and_clipped_inside_the_node() {
        let page = include_str!("../../ui/pages/infinite-canvas-page.slint");
        let state = include_str!("../../ui/app-state.slint");
        let types = include_str!("../../ui/types.slint");
        let model = include_str!("model.rs");
        let callbacks = include_str!("callbacks/infinite_canvas.rs");
        let sync = include_str!("presentation/sync.rs");

        assert!(state.contains("callback choose-canvas-node-image(string)"));
        assert!(types.contains("image-path: string"));
        assert!(types.contains("preview-image: image"));
        assert!(model.contains("image_path: String"));
        assert!(callbacks.contains("state.on_choose_canvas_node_image"));
        assert!(callbacks.contains("app_data_dir().join(\"canvas\").join(\"uploads\")"));
        assert!(callbacks.contains("atomic_write_file(&destination, &bytes)"));
        assert!(callbacks.contains("image_path = destination.display().to_string()"));
        assert!(sync.contains("load_image(Path::new(&note.image_path))"));
        assert!(page.contains("root.note.kind == \"image\" && root.note.image-path != \"\""));
        assert!(page.contains("source: root.note.preview-image"));
        assert!(page.contains("image-fit: contain"));
        assert!(page.contains("clip: true"));
        assert!(page.contains("AppState.choose-canvas-node-image(root.note.id)"));
    }

    #[test]
    fn infinite_canvas_links_nodes_and_feeds_upstream_prompts_downstream() {
        let page = include_str!("../../ui/pages/infinite-canvas-page.slint");
        let state = include_str!("../../ui/app-state.slint");
        let dialog = include_str!("../../ui/dialogs/delete-confirm.slint");

        assert!(page.contains("component CanvasConnectionCurve"));
        assert!(page.contains("connection-started(string, length, length)"));
        assert!(page.contains("root.begin-connection(source-id, start-x, start-y)"));
        assert!(page.contains("AppState.finish-canvas-link(source-id"));
        assert!(page.contains("for link in AppState.canvas-links"));
        assert!(page.contains("function effective-prompt()"));
        assert!(page.contains("AppState.prompt = root.effective-prompt()"));
        assert!(page.contains("已连接输入："));
        assert!(page.contains(
            "node-drag-touch.has-hover || input-connector-touch.has-hover || output-connector-touch.has-hover"
        ));
        assert!(page.contains("toolbar.y - self.height - 10px"));
        assert!(state.contains("canvas-drag-preview-id"));
        assert!(dialog.contains("确认删除这条连接？"));
    }

    #[test]
    fn infinite_canvas_links_are_selectable_and_backspace_requests_confirmation() {
        let page = include_str!("../../ui/pages/infinite-canvas-page.slint");
        let curve = page
            .split("component CanvasConnectionCurve")
            .nth(1)
            .and_then(|value| value.split("component CanvasNodeCard").next())
            .expect("canvas connection component");

        assert!(curve.contains("function estimated-curve-length()"));
        assert!(curve.contains("property <int> dash-count:"));
        assert!(curve.contains("property <int> hit-count:"));
        assert!(curve.contains("for dash-index in root.dash-count"));
        assert!(curve.contains("for hit-index in root.hit-count"));
        assert!(!curve.contains("for dash-index in 42"));
        assert!(!curve.contains("for hit-index in 42"));
        assert!(curve.contains("callback link-selected(string)"));
        assert!(curve.contains("root.link-selected(root.link.id)"));
        assert!(page.contains("canvas-keyboard := FocusScope"));
        assert!(page.contains("event.text == Key.Backspace"));
        assert!(page.contains("root.request-selected-delete()"));
        assert!(page.contains("AppState.canvas-selected-link-id = link-id"));
        assert!(page.contains("canvas-keyboard.focus()"));
    }

    #[test]
    fn infinite_canvas_text_nodes_match_the_reference_interaction_style() {
        let page = include_str!("../../ui/pages/infinite-canvas-page.slint");
        let state = include_str!("../../ui/app-state.slint");
        let canvas_callbacks = include_str!("callbacks/infinite_canvas.rs");
        let generation_callbacks = include_str!("callbacks/generation.rs");
        let model = include_str!("model.rs");
        let node = page
            .split("component CanvasNodeCard")
            .nth(1)
            .and_then(|value| value.split("export component InfiniteCanvasPage").next())
            .expect("canvas node component");

        assert!(node.contains("root.note.kind == \"text\" || root.is-visual-media()"));
        assert!(node.contains("text-action-bar := Rectangle"));
        assert!(node.contains("node-drag-touch := TouchArea"));
        assert!(node.contains("double-clicked"));
        assert!(node.contains("text-editor.focus()"));
        assert!(node.contains("AppState.optimize-canvas-text-node"));
        assert!(node.contains("AppState.en ? \"AI Optimize\" : \"AI优化\""));
        assert!(node.contains("AppState.en ? \"Generate\" : \"生图\""));
        assert!(node.contains("root.generate-from-text()"));
        assert!(node.contains("font-size: root.note.font-size * 1px * root.node-scale()"));
        assert!(state.contains("callback adjust-canvas-text-font-size(string, float)"));
        assert!(state.contains("callback optimize-canvas-text-node(string, string)"));
        assert!(canvas_callbacks.contains("on_adjust_canvas_text_font_size"));
        assert!(canvas_callbacks.contains(".clamp(8.0, 72.0)"));
        assert!(generation_callbacks.contains("on_optimize_canvas_text_node"));
        assert!(generation_callbacks.contains("PromptResultTarget::CanvasNode"));
        assert!(model.contains("default_canvas_font_size"));
    }

    #[test]
    fn infinite_canvas_media_nodes_expand_reference_style_editors_when_selected() {
        let page = include_str!("../../ui/pages/infinite-canvas-page.slint");
        let node = page
            .split("component CanvasNodeCard")
            .nth(1)
            .and_then(|value| value.split("export component InfiniteCanvasPage").next())
            .expect("canvas node component");

        assert!(node.contains("function is-visual-media()"));
        assert!(node.contains("media-action-bar := Rectangle"));
        assert!(node.contains("media-editor-panel := Rectangle"));
        assert!(node.contains(
            "if root.is-visual-media() && AppState.canvas-selected-id == root.note.id && root.zoom-percent >= 30: media-action-bar"
        ));
        assert!(node.contains(
            "if root.is-visual-media() && AppState.canvas-selected-id == root.note.id && root.zoom-percent >= 30: media-editor-panel"
        ));
        assert!(!node.contains(
            "AppState.canvas-selected-id == root.note.id && root.zoom-percent >= 45: media-action-bar"
        ));
        assert!(node.contains("540px : 580px) * root.node-scale()"));
        assert!(node.contains("image-model-popup := PopupWindow"));
        assert!(node.contains("image-settings-popup := PopupWindow"));
        assert!(node.contains("video-settings-popup := PopupWindow"));
        assert!(node.contains("audio-settings-popup := PopupWindow"));
        assert!(node.contains("空图片节点"));
        assert!(node.contains("空视频节点"));
        assert!(node.contains("空音频节点"));
        assert!(node.contains("上传图片"));
        assert!(node.contains("上传视频"));
        assert!(node.contains("上传音频"));
        assert!(node.contains("AppState.model-image-options"));
        assert!(node.contains("AppState.count = 4"));
        assert!(node.contains("AppState.quality = \"1K\""));
        assert!(node.contains("AppState.quality = \"2K\""));
        assert!(node.contains("AppState.quality = \"4K\""));
        assert!(node.contains("AppState.quality + \" · \" + AppState.ratio"));
        assert!(node.contains("audio-voice: \"Alloy\""));
        assert!(node.contains("audio-format: \"MP3\""));
        assert!(node.contains("audio-speed: \"1x\""));
        assert!(node.contains("function media-editor-y()"));
        assert!(node.contains("function settings-popup-x"));
        assert!(node.contains("audio-settings-scroll := Flickable"));
        assert!(page.contains("viewport-width: canvas.width"));
        assert!(node.contains("AppState.generate()"));
    }

    #[test]
    fn infinite_canvas_node_visuals_and_overlays_share_zoom_scale() {
        let page = include_str!("../../ui/pages/infinite-canvas-page.slint");
        let action = page
            .split("component CanvasMediaAction")
            .nth(1)
            .and_then(|value| value.split("component CanvasMediaChip").next())
            .expect("canvas media action component");
        let node = page
            .split("component CanvasNodeCard")
            .nth(1)
            .and_then(|value| value.split("export component InfiniteCanvasPage").next())
            .expect("canvas node component");
        let chip = page
            .split("component CanvasMediaChip")
            .nth(1)
            .and_then(|value| value.split("component CanvasOptionPill").next())
            .expect("canvas media chip component");

        assert!(action.contains("in property <float> scale-factor"));
        assert!(action.contains("height: 38px * root.scale-factor"));
        assert!(action.contains("width: 16px * root.scale-factor"));
        assert!(action.contains("font-size: 13px * root.scale-factor"));
        assert!(chip.contains("in property <float> scale-factor"));
        assert!(chip.contains("height: 38px * root.scale-factor"));
        assert!(chip.contains("font-size: 13px * root.scale-factor"));
        assert!(node.contains("function node-scale() -> float"));
        assert!(node.contains("height: 46px * root.node-scale()"));
        assert!(node.contains("scale-factor: root.node-scale()"));
        assert!(node.contains("width: 64px * root.node-scale()"));
        assert!(node.contains("width: 28px * root.node-scale()"));
        assert!(node.contains("font-size: 13px * root.node-scale()"));
        assert!(node.contains("return 180px * root.node-scale()"));
        assert!(node.contains("x: (parent.width - self.width) / 2"));
        assert!(!node.contains("max(312px, 312px * root.zoom-percent / 100)"));
        assert!(!node.contains("max(54px, 64px * root.zoom-percent / 100)"));
    }

    #[test]
    fn infinite_canvas_media_editor_stays_below_node_at_every_zoom() {
        let page = include_str!("../../ui/pages/infinite-canvas-page.slint");
        let node = page
            .split("component CanvasNodeCard")
            .nth(1)
            .and_then(|value| value.split("export component InfiniteCanvasPage").next())
            .expect("canvas node component");
        let editor_y = node
            .split("function media-editor-y()")
            .nth(1)
            .and_then(|value| value.split("function dropdown-popup-x").next())
            .expect("media editor y function");

        assert!(editor_y.contains("return root.height + 20px * root.node-scale();"));
        assert!(!editor_y.contains("root.viewport-height"));
        assert!(!editor_y.contains("-root.media-editor-height()"));
    }

    #[test]
    fn infinite_canvas_action_bars_stay_above_nodes_at_every_zoom() {
        let page = include_str!("../../ui/pages/infinite-canvas-page.slint");
        let node = page
            .split("component CanvasNodeCard")
            .nth(1)
            .and_then(|value| value.split("export component InfiniteCanvasPage").next())
            .expect("canvas node component");
        let action_bar_y = node
            .split("function action-bar-y()")
            .nth(1)
            .and_then(|value| value.split("function dropdown-popup-x").next())
            .expect("action bar y function");
        let text_bar = node
            .split("text-action-bar := Rectangle")
            .nth(1)
            .and_then(|value| value.split("media-action-bar := Rectangle").next())
            .expect("text action bar");
        let media_bar = node
            .split("media-action-bar := Rectangle")
            .nth(1)
            .and_then(|value| value.split("media-editor-panel := Rectangle").next())
            .expect("media action bar");

        assert!(action_bar_y.contains("return -62px * root.node-scale();"));
        assert!(!action_bar_y.contains("root.y"));
        assert!(!action_bar_y.contains("root.viewport-height"));
        assert!(text_bar.contains("y: root.action-bar-y();"));
        assert!(media_bar.contains("y: root.action-bar-y();"));
        assert!(!text_bar.contains("root.y <"));
        assert!(!media_bar.contains("root.y <"));
    }

    #[test]
    fn infinite_canvas_action_bar_buttons_evenly_fill_the_background() {
        let page = include_str!("../../ui/pages/infinite-canvas-page.slint");
        let node = page
            .split("component CanvasNodeCard")
            .nth(1)
            .and_then(|value| value.split("export component InfiniteCanvasPage").next())
            .expect("canvas node component");
        let text_bar = node
            .split("text-action-bar := Rectangle")
            .nth(1)
            .and_then(|value| value.split("media-action-bar := Rectangle").next())
            .expect("text action bar");
        let media_bar = node
            .split("media-action-bar := Rectangle")
            .nth(1)
            .and_then(|value| value.split("media-editor-panel := Rectangle").next())
            .expect("media action bar");
        let video_actions = media_bar
            .split("if root.note.kind == \"video\": HorizontalLayout")
            .nth(1)
            .and_then(|value| {
                value
                    .split("if root.note.kind != \"video\": HorizontalLayout")
                    .next()
            })
            .expect("video action layout");
        let other_actions = media_bar
            .split("if root.note.kind != \"video\": HorizontalLayout")
            .nth(1)
            .expect("image and audio action layout");

        assert_eq!(
            text_bar
                .matches("CanvasMediaAction { horizontal-stretch: 1;")
                .count(),
            7
        );
        for label in ["信息", "删除", "存素材", "编辑", "生图", "缩小", "放大"] {
            assert!(text_bar.contains(label), "missing text node action: {label}");
        }
        assert!(!text_bar.contains("编辑文字"));
        assert!(text_bar.contains("AppState.adjust-canvas-text-font-size(root.note.id, -1)"));
        assert!(text_bar.contains("AppState.adjust-canvas-text-font-size(root.note.id, 1)"));
        assert_eq!(
            video_actions
                .matches("CanvasMediaAction { horizontal-stretch: 1;")
                .count(),
            4
        );
        assert_eq!(
            other_actions
                .matches("CanvasMediaAction { horizontal-stretch: 1;")
                .count(),
            3
        );
        assert!(
            !text_bar.contains("CanvasMediaAction { scale-factor: root.node-scale(); width:")
        );
        assert!(!media_bar
            .contains("CanvasMediaAction { scale-factor: root.node-scale(); width:"));
    }

    #[test]
    fn infinite_canvas_hides_subpixel_node_details_at_minimum_zoom() {
        let page = include_str!("../../ui/pages/infinite-canvas-page.slint");
        let node = page
            .split("component CanvasNodeCard")
            .nth(1)
            .and_then(|value| value.split("export component InfiniteCanvasPage").next())
            .expect("canvas node component");

        assert!(node.contains(
            "if root.is-visual-media() && AppState.canvas-selected-id == root.note.id && root.zoom-percent >= 30: media-action-bar"
        ));
        assert!(node.contains(
            "if root.is-visual-media() && AppState.canvas-selected-id == root.note.id && root.zoom-percent >= 30: media-editor-panel"
        ));
        assert!(node.contains(
            "visible: root.note.kind == \"group\" && root.zoom-percent >= 30"
        ));
    }

    #[test]
    fn infinite_canvas_nodes_connect_from_both_sides() {
        let page = include_str!("../../ui/pages/infinite-canvas-page.slint");
        let node = page
            .split("component CanvasNodeCard")
            .nth(1)
            .and_then(|value| value.split("export component InfiniteCanvasPage").next())
            .expect("canvas node component");

        let input_connector = node
            .split("input-connector-touch := TouchArea")
            .nth(1)
            .and_then(|value| value.split("output-connector-touch := TouchArea").next())
            .expect("left connector touch area");
        let output_connector = node
            .split("output-connector-touch := TouchArea")
            .nth(1)
            .and_then(|value| value.split("image-model-popup := PopupWindow").next())
            .expect("right connector touch area");

        assert!(input_connector.contains(
            "root.reconnect-started(root.note.id, root.x, root.y + root.height / 2)"
        ));
        assert!(input_connector.contains("root.reconnect-finished"));
        assert!(output_connector.contains(
            "root.connection-started(root.note.id, root.x + root.width, root.y + root.height / 2)"
        ));
        assert!(output_connector.contains("root.connection-finished"));
    }

    #[test]
    fn infinite_canvas_zoom_control_matches_the_compact_reference_style() {
        let page = include_str!("../../ui/pages/infinite-canvas-page.slint");
        let zoom_panel = page
            .split("zoom-panel := Rectangle")
            .nth(1)
            .and_then(|value| value.split("toolbar := Rectangle").next())
            .expect("zoom panel");

        assert!(page.contains("component CanvasZoomButton"));
        assert!(zoom_panel.contains("width: min(250px"));
        assert!(zoom_panel.contains("height: 48px"));
        assert!(zoom_panel.contains("compass.svg"));
        assert!(zoom_panel.contains("focus.svg"));
        assert!(zoom_panel.contains("help.svg"));
        assert!(zoom_panel.contains("height: 4px"));
        assert!(zoom_panel.contains("background: #f2eee9"));
        assert!(zoom_panel.contains("property <length> thumb-center-x"));
        assert!(zoom_panel.contains("x: 0px"));
        assert!(zoom_panel.contains("width: zoom-track.thumb-center-x"));
        assert!(zoom_panel.contains("x: zoom-track.thumb-center-x - 7px"));
        assert!(!zoom_panel.contains("parent.width * (root.zoom-percent - 5) / 495"));
        assert!(!zoom_panel.contains("background: AppTheme.accent"));
    }

    #[test]
    fn atomic_image_write_propagates_disk_errors_without_final_file() {
        let root = std::env::temp_dir().join(format!("artforge-atomic-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let target = root.join("image.png");
        atomic_write_file(&target, b"image").unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"image");

        let not_a_directory = root.join("not-a-directory");
        fs::write(&not_a_directory, b"file").unwrap();
        let invalid_target = not_a_directory.join("image.png");
        assert!(atomic_write_file(&invalid_target, b"image").is_err());
        assert!(!invalid_target.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn payment_ui_uses_direct_alipay_qr_flow() {
        let credit_page = include_str!("../../ui/pages/credits-page.slint");
        let checkout = include_str!("../../ui/dialogs/ali-pay-qr-dialog.slint");
        let membership = include_str!("../../ui/components/membership-plans.slint");
        let purchase_agreements = include_str!("../../ui/components/purchase-agreements.slint");
        let callbacks = include_str!("callbacks/payment.rs");
        let payment_window = include_str!("payment_window.rs");
        let top_bar = include_str!("../../ui/components/top-bar.slint");

        assert!(credit_page.contains(
            "clicked => { AppState.recharge-credits(AppState.selected-credit-pack-code); }"
        ));
        assert!(!credit_page.contains("AppState.credit-pay-open = true"));
        assert!(membership.contains("clicked => { AppState.purchase-membership(plan.code); }"));

        assert!(checkout.contains("AppState.payment-qr-open"));
        assert!(checkout.contains("支付宝扫码支付"));
        assert!(checkout.contains("正在等待支付宝付款结果"));
        assert!(!checkout.contains("payment-qr-summary"));
        assert!(!checkout.contains("PurchaseAgreement"));
        assert!(!checkout.contains("生成支付二维码"));
        assert!(payment_window.contains(".with_html(config.checkout_html)"));
        assert!(!payment_window.contains(".with_url(config.checkout.to_string())"));
        assert!(!callbacks.contains("state.set_payment_qr_message(message.clone().into());"));
        assert!(callbacks.contains("暂时无法确认支付结果，请稍后查看订单状态"));
        assert!(membership.contains("PurchaseAgreements"));
        assert!(credit_page.contains("PurchaseAgreements"));
        assert!(purchase_agreements.contains("purchase-membership-accepted"));
        assert!(purchase_agreements.contains("purchase-credit-rules-accepted"));
        assert!(callbacks.contains("agreements_api.accept_agreements(&acceptances)?;"));
        assert!(callbacks.contains("apply_agreements_from_payment_error"));
        assert!(callbacks.contains("agreement_acceptance_required"));
        assert!(callbacks.contains("cancel_active_payment"));
        assert!(callbacks.contains("支付已取消，可重新发起支付"));
        assert!(!callbacks.contains(
            "if started.kind == PaymentOrderKind::Membership {\n            state.set_membership_open(false);"
        ));
        assert!(top_bar.contains("关闭支付码"));

        let combined = format!("{checkout}\n{membership}\n{top_bar}");
        for removed in ["支付宝收银台", "打开支付宝支付", "关闭收银台"] {
            assert!(!combined.contains(removed), "obsolete payment copy: {removed}");
        }
    }

    #[test]
    fn credits_page_contains_recharge_and_subscription_tabs() {
        let credits = include_str!("../../ui/pages/credits-page.slint");
        let profile = include_str!("../../ui/dialogs/profile-dialog.slint");
        let app = include_str!("../../ui/app.slint");
        let state = include_str!("../../ui/app-state.slint");
        let membership = include_str!("../../ui/components/membership-plans.slint");

        assert!(state.contains("in-out property <string> credits-tab: \"recharge\";"));
        assert!(credits.contains("text: AppState.en ? \"Recharge\" : \"充值\";"));
        assert!(credits.contains("text: AppState.en ? \"Subscription\" : \"订阅\";"));
        assert!(credits.contains("active: AppState.credits-tab == \"recharge\";"));
        assert!(credits.contains("active: AppState.credits-tab == \"membership\";"));
        assert!(credits.contains("MembershipPlans { horizontal-stretch: 1; }"));
        assert!(membership.contains("AppState.purchase-membership(plan.code)"));
        assert!(membership.contains("PurchaseAgreements"));
        assert!(profile.contains("AppState.navigate(\"credits\")"));
        assert!(profile.contains("AppState.credits-tab = \"membership\""));
        assert!(!app.contains("MembershipDialog"));
    }

    #[test]
    fn invoice_application_ui_is_required_and_reachable() {
        let credits = include_str!("../../ui/pages/credits-page.slint");
        let app = include_str!("../../ui/app.slint");
        let state = include_str!("../../ui/app-state.slint");
        let order_dialog_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("ui/dialogs/invoice-order-dialog.slint");
        let order_dialog = std::fs::read_to_string(order_dialog_path).unwrap_or_default();
        let dialog_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("ui/dialogs/invoice-dialog.slint");
        let dialog = std::fs::read_to_string(dialog_path).unwrap_or_default();

        assert!(credits.contains("申请开票"));
        assert!(credits.contains("callback open-invoice-orders()"));
        assert!(credits.contains("root.open-invoice-orders()"));
        assert!(app.contains("InvoiceOrderDialog"));
        assert!(app.contains("property <bool> invoice-orders-open"));
        assert!(app.contains("root.invoice-orders-open = true"));
        assert!(app.contains("InvoiceDialog"));
        assert!(app.contains("property <bool> invoice-open"));
        assert!(app.contains("root.invoice-orders-open = false"));
        assert!(app.contains("root.invoice-open = true"));
        assert!(!state.contains("invoice-open"));
        assert!(!state.contains("submit-invoice-request"));
        assert!(state.contains("in-out property <[InvoiceOrderView]> invoice-orders: []"));
        assert!(order_dialog.contains("for order in AppState.invoice-orders"));
        assert!(order_dialog.contains("disabled: !root.item.eligible"));
        assert!(order_dialog.contains("select-order("));
        assert!(order_dialog.contains("单次充值满 ¥100.00 可申请开票"));
        assert!(dialog.contains("in-out property <bool> open"));
        assert!(dialog.contains("in property <string> selected-order-id"));
        assert!(dialog.contains("所选订单"));
        assert!(!dialog.contains("AppState.invoice-"));

        for label in [
            "发票类型",
            "抬头类型",
            "发票抬头",
            "税号",
            "接收邮箱",
        ] {
            assert!(dialog.contains(label), "missing required field: {label}");
        }
        assert!(dialog.contains("电子增值税普通发票"));
        assert!(dialog.contains("个人"));
        assert!(!dialog.contains("事业单位"));
        assert!(!dialog.contains("\"institution\""));
        assert!(dialog.contains("企业"));
        assert_eq!(dialog.matches("RequiredLabel { text:").count(), 2);
        assert_eq!(dialog.matches("required: true;").count(), 3);
        assert!(dialog.contains("function requires-tax-id() -> bool"));
        assert!(dialog.contains("!root.requires-tax-id() || root.invoice-tax-id != \"\""));
        assert!(dialog.contains("if root.requires-tax-id(): Field"));
        assert!(dialog.contains("root.invoice-tax-id = \"\""));
        assert!(dialog.contains("viewport-width: self.width"));
        assert!(dialog.contains("clip: true"));
        assert!(dialog.contains("disabled: !root.form-valid()"));
        assert!(dialog.contains("将在12小时内自动推送至您的电子邮箱内"));
        assert!(dialog.contains("超过6个月未申请开票的订单暂不支持线上开具"));
        assert!(dialog.contains("具有同等法律效力"));
        assert!(dialog.contains("仅为实际支付金额"));
    }

    #[test]
    fn credit_plans_fill_the_recharge_row() {
        let credits = include_str!("../../ui/pages/credits-page.slint");
        let plan = include_str!("../../ui/components/credit-plan.slint");

        let plans = credits
            .split("for pack in AppState.credit-packs: CreditPlan")
            .nth(1)
            .and_then(|value| value.split("PurchaseAgreements").next())
            .expect("credit plan row");
        assert!(plans.contains("horizontal-stretch: 1;"));
        assert!(!plans.contains("Rectangle { horizontal-stretch: 1; background: transparent; }"));
        assert!(plan.contains("AppTheme.accent.with-alpha(0.12)"));
        assert!(plan.contains("visible: AppState.selected-credit-pack-code == root.code;"));
        assert!(!plan.contains("AppState.en ? \"Select\" : \"选择\""));
    }

    #[test]
    fn free_membership_copy_is_vertically_centered_without_an_action_button() {
        let membership = include_str!("../../ui/components/membership-plans.slint");

        assert!(membership.contains("if plan.code == \"free\": Rectangle { vertical-stretch: 1;"));
        assert!(membership.contains("Rectangle { vertical-stretch: 1; background: transparent; }"));
        assert!(membership.contains("if plan.code != \"free\": PillButton"));
    }

    #[test]
    fn dynamic_pages_and_dialogs_keep_content_inside_visible_bounds() {
        let profile = include_str!("../../ui/dialogs/profile-dialog.slint");
        let auth = include_str!("../../ui/dialogs/auth-dialog.slint");
        let agreement_update = include_str!("../../ui/dialogs/agreement-update-dialog.slint");
        let agreement_viewer = include_str!("../../ui/dialogs/agreement-viewer-dialog.slint");
        let update_prompt = include_str!("../../ui/dialogs/version-check-dialog.slint");
        let models = include_str!("../../ui/pages/models-page.slint");
        let notifications = include_str!("../../ui/pages/notifications-page.slint");
        let settings = include_str!("../../ui/pages/settings-page.slint");

        assert!(profile.contains("height: min(650px, root.height - 48px);"));
        assert!(profile.contains(
            "viewport-height: max(self.height, AppState.account-sessions.length * 68px);"
        ));
        assert!(profile.contains("width: min(920px, root.width - 48px);"));

        assert!(auth.contains("height: min(700px, root.height - 40px);"));
        assert!(agreement_update.contains("height: min(380px, root.height - 40px);"));
        assert!(agreement_viewer.contains("width: min(860px, root.width - 32px);"));
        assert!(agreement_viewer.contains("height: parent.height - 120px;"));
        assert!(update_prompt.contains("width: min(520px, root.width - 32px);"));
        assert!(update_prompt.contains("min(360px, root.height - 40px)"));

        assert!(!models.contains("ScrollView"));
        assert!(settings.contains("function models-height() -> length"));
        assert!(settings.contains("AppState.catalog-models.length * 148px"));
        assert!(settings.contains("function page-height() -> length"));
        assert!(notifications.contains("function list-height() -> length"));
        assert!(notifications.contains("viewport-height: root.list-height();"));
        assert!(settings.contains("viewport-height: max(root.page-height(), parent.height);"));
    }

    #[test]
    fn thumbnail_hover_delete_reuses_confirmation_with_explicit_source() {
        let card = include_str!("../../ui/components/thumbnail-card.slint");
        let state = include_str!("../../ui/app-state.slint");
        let viewer = include_str!("../../ui/dialogs/viewer-overlay.slint");
        let callbacks = include_str!("callbacks/viewer.rs");

        assert!(card.contains("@image-url(\"../../assets/icons/trash.svg\")"));
        assert!(card.contains("visible: hover.has-hover && root.can-delete()"));
        assert!(card.contains("root.delete-hit()"));
        assert!(card.contains("root.source == \"asset\" || root.source == \"generation\""));
        assert!(!card.contains("root.source == \"inspiration\""));
        assert!(card.contains(
            "AppState.request-delete-thumbnail(root.item.id, root.source)"
        ));
        assert!(state.contains("callback request-delete-thumbnail(string, string);"));
        assert!(callbacks.contains("state.on_request_delete_thumbnail"));

        assert!(state.contains("callback request-delete-asset(string);"));
        assert!(viewer.contains("AppState.request-delete-asset(AppState.viewer-id)"));
    }

    #[test]
    fn completed_generation_opens_its_image_viewer() {
        let model = include_str!("model.rs");
        let poll = include_str!("generation/poll.rs");
        let state = include_str!("generation/state.rs");

        assert!(model.contains("latest_success_id: Option<String>"));
        assert!(state.contains("task.latest_success_id = success_id;"));
        assert!(poll.contains("open-viewer-after-finish"));
        assert!(poll.contains("open_viewer(&app, &store.borrow(), &viewer_id, \"generation\")"));
    }

    #[test]
    fn viewer_metadata_is_four_compact_tags_in_the_top_row() {
        let viewer = include_str!("../../ui/dialogs/viewer-overlay.slint");

        assert!(viewer.contains("component ViewerInfoTag"));
        assert!(viewer.contains("info-tags := HorizontalLayout"));
        assert!(viewer.contains("y: 24px;"));
        assert_eq!(viewer.matches("ViewerInfoTag {").count(), 4);
        assert!(!viewer.contains("InfoCard"));
        assert!(!viewer.contains("图像信息"));
    }

    #[test]
    fn sidebar_can_collapse_to_icon_only_navigation() {
        let app_state = include_str!("../../ui/app-state.slint");
        let sidebar = include_str!("../../ui/components/sidebar.slint");
        let nav_item = include_str!("../../ui/components/nav-item.slint");
        let workspace = include_str!("../../ui/components/category-workspace-menu.slint");

        assert!(app_state.contains("in-out property <bool> sidebar-collapsed: false;"));
        assert!(sidebar.contains("width: AppState.sidebar-collapsed ? 72px : 204px;"));
        assert!(sidebar.contains("AppState.sidebar-collapsed = !AppState.sidebar-collapsed"));
        assert!(nav_item.contains("in property <bool> collapsed: false;"));
        assert!(nav_item.contains("if !root.collapsed: Text"));
        assert!(workspace.contains("in property <bool> collapsed: false;"));
        assert!(workspace.contains("if !root.collapsed && root.open"));
    }

    #[test]
    fn rounded_thumbnail_image_fills_the_hover_outline() {
        let card = include_str!("../../ui/components/thumbnail-card.slint");
        let content_index = card
            .find("content := Rectangle")
            .expect("thumbnail image content");
        let outline_index = card
            .find("hover-outline := Rectangle")
            .expect("thumbnail hover outline");

        assert!(card.contains("property <length> outline-pad: 0px;"));
        assert!(card.contains(
            "border-radius: AppState.card-style == \"rounded\" ? 10px : 0px;"
        ));
        assert!(
            content_index < outline_index,
            "the outline must be painted over the full-bleed image"
        );
    }

    #[test]
    fn failed_generation_thumbnail_hover_requests_confirmed_delete() {
        let card = include_str!("../../ui/components/thumbnail-card.slint");
        let callbacks = include_str!("callbacks/viewer.rs");

        assert!(card.contains("failed-hover := TouchArea"));
        assert!(card.contains("failed-delete-touch := TouchArea"));
        assert!(card.contains(
            "visible: failed-hover.has-hover || failed-delete-touch.has-hover"
        ));
        assert!(card.contains(
            "AppState.request-delete-thumbnail(root.item.id, \"generation\")"
        ));
        assert!(card.contains("visible: root.item.source-path != \"failed\";"));
        assert!(callbacks.contains("store_mut.generations.retain(|a| a.id != id)"));
    }

    #[test]
    fn windows_uses_gpu_renderer_without_removing_software_override() {
        let app = include_str!("app.rs");
        let manifest = include_str!("../../Cargo.toml");
        assert!(app.contains("winit-femtovg"));
        assert!(app.contains("std::env::var_os(\"SLINT_BACKEND\")"));
        assert!(manifest.contains("\"renderer-femtovg\""));
        assert!(manifest.contains("\"renderer-software\""));
    }

    #[test]
    fn recovered_pending_payment_reopens_the_embedded_surface() {
        let callbacks = include_str!("callbacks/payment.rs");
        assert!(!callbacks.contains(
            "continue_payment_order(&app, context, backend, started, false);"
        ));
    }

    #[test]
    fn all_agreement_links_use_the_embedded_client_viewer() {
        let app = include_str!("../../ui/app.slint");
        let auth_dialog = include_str!("../../ui/dialogs/auth-dialog.slint");
        let update_dialog = include_str!("../../ui/dialogs/agreement-update-dialog.slint");
        let purchase_agreements = include_str!("../../ui/components/purchase-agreements.slint");
        let credits = include_str!("../../ui/pages/credits-page.slint");
        let auth_callbacks = include_str!("callbacks/auth.rs");
        let agreement_window = include_str!("agreement_window.rs");

        assert!(app.contains("AgreementViewerDialog"));
        assert!(auth_dialog.contains("AppState.open-agreement(title, url)"));
        assert!(update_dialog.contains("AppState.open-agreement(root.title, root.url)"));
        assert!(purchase_agreements.contains("AppState.open-agreement(root.title, root.url)"));
        assert!(credits.contains("AppState.open-agreement(AppState.purchase-credit-rules-title"));
        assert!(auth_callbacks.contains("open_agreement_window(&app, &url)"));
        assert!(!auth_callbacks.contains("open_external_url"));
        assert!(agreement_window.contains(".with_url(config.content_url)"));
        assert!(agreement_window.contains("NewWindowResponse::Deny"));
        assert!(agreement_window.contains("cdn.honeykid.cn"));
    }

    #[test]
    fn insufficient_credit_generation_opens_recharge_dialog_without_failed_record() {
        let backend = include_str!("generation/backend.rs");
        let poll = include_str!("generation/poll.rs");
        let model = include_str!("model.rs");
        let dialog = include_str!("../../ui/dialogs/credit-insufficient-dialog.slint");
        let api_error = include_str!("api/error.rs");

        assert!(api_error.contains("is_insufficient_credits"));
        assert!(model.contains("CreditInsufficient"));
        assert!(backend.contains("error.is_insufficient_credits()"));
        assert!(backend.contains("GenerationOutcome::CreditInsufficient"));
        assert!(backend.contains("remove_pending_generation(&request.client_request_id)"));
        assert!(poll.contains("GenerationOutcome::CreditInsufficient"));
        let credit_branch = poll
            .split("GenerationOutcome::CreditInsufficient")
            .nth(1)
            .and_then(|value| value.split("GenerationOutcome::Failure").next())
            .expect("credit insufficient branch");
        assert!(credit_branch.contains("state.set_credit_insufficient_open(true)"));
        assert!(credit_branch.contains("restore_stream_inputs("));
        assert!(credit_branch.contains("remove_conversation_placeholder(&state, &conversation_id)"));
        assert!(!credit_branch.contains("finish_conversation_placeholder(&state, &conversation_id"));
        assert!(dialog.contains("积分不足"));
        assert!(dialog.contains("前往充值"));
        assert!(dialog.contains("AppState.navigate(\"credits\")"));
    }

    #[test]
    fn generation_keeps_prompt_text_until_the_user_clears_it() {
        let backend = include_str!("generation/backend.rs");
        let controller = include_str!("generation/controller.rs");

        assert!(!backend.contains("state.set_prompt(\"\".into());"));
        let restore_inputs = controller
            .split("pub(super) fn restore_stream_inputs")
            .nth(1)
            .and_then(|value| value.split("pub(super) fn set_stream_final_status").next())
            .expect("stream input restore helper");
        assert!(!restore_inputs.contains("state.set_prompt("));
        assert!(!restore_inputs.contains("set_prompt_draft_for_category("));
    }
}
