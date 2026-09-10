#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quality_pixel_size_uses_standard_2k_and_4k_dimensions() {
        assert_eq!(pixel_dimensions_for("9:16", "1K"), (576, 1024));
        assert_eq!(pixel_dimensions_for("16:9", "1K"), (1024, 576));
        assert_eq!(pixel_dimensions_for("16:9", "2K"), (2560, 1440));
        assert_eq!(pixel_dimensions_for("9:16", "2K"), (1440, 2560));
        assert_eq!(pixel_dimensions_for("16:9", "4K"), (3840, 2160));
        assert_eq!(pixel_dimensions_for("9:16", "4K"), (2160, 3840));

        assert_eq!(quality_from_actual_dimensions(1023, 1537), "2K");
        assert_eq!(quality_from_actual_dimensions(1024, 1024), "1K");
        assert_eq!(quality_from_actual_dimensions(2048, 1152), "2K");
        assert_eq!(quality_from_actual_dimensions(2560, 1440), "2K");
        assert_eq!(quality_from_actual_dimensions(3840, 2160), "4K");
    }

    #[test]
    fn update_versions_and_download_urls_are_checked_before_prompting() {
        assert!(compare_versions("1.0.6", "1.0.5").is_gt());
        assert!(compare_versions("1.0.5", "1.0.5").is_eq());
        assert!(compare_versions("1.0.4", "1.0.5").is_lt());

        assert!(validated_update_download_url(
            "https://static.honeykid.cn/public/art_forge/ElunviCanvas_macos_aarch64.dmg"
        )
        .is_ok());
        assert_eq!(
            canonical_update_download_url(
                "https://cdn.honeykid.cn/public/art_forge/ElunviCanvas_macos_aarch64.dmg"
            )
            .as_deref(),
            Some("https://static.honeykid.cn/public/art_forge/ElunviCanvas_macos_aarch64.dmg")
        );
        assert!(validated_update_download_url("http://static.honeykid.cn/update.dmg").is_err());
        assert!(validated_update_download_url(
            "https://static.honeykid.cn.attacker.example/update.dmg"
        )
        .is_err());
        assert!(validated_update_download_url("https://attacker.example/update.dmg").is_err());
        assert!(validated_update_download_url("not-a-url").is_err());
        assert!(valid_update_artifact_metadata(42, &"a".repeat(64)));
        assert!(!valid_update_artifact_metadata(0, &"a".repeat(64)));
        assert!(!valid_update_artifact_metadata(42, "not-a-sha256"));
        assert_eq!(
            shell_quote("ArtForge's update"),
            "'ArtForge'\"'\"'s update'"
        );
        assert!(is_update_temp_dir_name(&format!(
            "artforge-update-{}",
            Uuid::new_v4()
        )));
        assert!(!is_update_temp_dir_name("artforge-update-not-a-uuid"));
    }

    #[test]
    fn update_manifest_accepts_integrity_metadata_without_breaking_legacy_download_fields() {
        let manifest: UpdateManifest = serde_json::from_value(serde_json::json!({
            "version": "1.0.10",
            "downloads": {
                "macos_aarch64": "https://static.honeykid.cn/public/art_forge/1.0.10/ElunviCanvas_macos_aarch64.dmg",
                "macos_x64": "https://static.honeykid.cn/public/art_forge/1.0.10/ElunviCanvas_macos_x64.dmg",
                "windows_x64": "https://static.honeykid.cn/public/art_forge/1.0.10/ElunviCanvas_windows_x64_setup.exe"
            },
            "artifacts": {
                "macos_aarch64": { "size_bytes": 42, "sha256": "a".repeat(64) },
                "macos_x64": { "size_bytes": 43, "sha256": "b".repeat(64) },
                "windows_x64": { "size_bytes": 44, "sha256": "c".repeat(64) }
            }
        }))
        .unwrap();

        assert!(manifest.downloads.windows_x64.contains("/1.0.10/"));
        assert_eq!(manifest.artifacts.macos_aarch64.size_bytes, 42);
        assert_eq!(manifest.artifacts.windows_x64.sha256, "c".repeat(64));

        let legacy: UpdateManifest = serde_json::from_value(serde_json::json!({
            "version": "1.0.9",
            "downloads": {
                "macos_aarch64": "",
                "macos_x64": "",
                "windows_x64": ""
            }
        }))
        .unwrap();
        assert_eq!(legacy.artifacts.windows_x64.size_bytes, 0);
        assert!(legacy.artifacts.windows_x64.sha256.is_empty());
    }

    #[test]
    fn update_prompt_has_optional_and_required_paths() {
        let dialog = include_str!("../../ui/dialogs/version-check-dialog.slint");
        let state = include_str!("../../ui/app-state.slint");
        let app = include_str!("../../ui/app.slint");
        let updater = include_str!("storage/updater.rs");
        let installer = include_str!("../../../installer/ElunviCanvas.iss");
        let release_workflow = include_str!("../../../.github/workflows/release-desktop.yml");
        let manifest_script = include_str!("../../../scripts/create-update-manifest.js");

        assert!(dialog.contains("AppState.update-required"));
        assert!(dialog.contains("\"稍后再说\""));
        assert!(dialog.contains("\"立即更新\""));
        assert!(dialog.contains("\"离线使用\""));
        assert!(dialog.contains("\"重新检查\""));
        assert!(dialog.contains("\"已是最新版本\""));
        assert!(dialog.contains("\"关闭\""));
        assert!(dialog.contains("min(420px, root.width - 32px)"));
        assert!(dialog.contains("min(240px, root.height - 40px)"));
        assert!(dialog.contains("width: 160px;"));
        assert!(state.contains("in-out property <string> update-download-url"));
        assert!(state.contains("in-out property <string> update-download-sha256"));
        assert!(state.contains("in-out property <string> update-stage"));
        assert!(state.contains("callback cancel-update()"));
        assert!(dialog.contains("AppState.update-download-progress"));
        assert!(dialog.contains("visible-progress: max(0, min(100"));
        assert!(dialog.contains("progress-fill := Rectangle"));
        assert!(dialog.contains("x: 0px;"));
        assert!(dialog.contains("width: max(0px, active-progress.width - 32px);"));
        assert!(dialog.contains("animate width { duration: 220ms; easing: ease-out; }"));
        assert!(dialog.contains("\"正在核对文件大小与 SHA-256\""));
        assert!(dialog.contains("AppState.cancel-update()"));
        assert!(state.contains("in-out property <bool> update-check-failed"));
        assert!(updater.contains("Sha256"));
        assert!(updater.contains("hdiutil verify"));
        assert!(updater.contains("codesign --verify --deep --strict"));
        assert!(installer.contains("skipifnotsilent"));
        assert!(release_workflow.contains("actions/download-artifact@v8"));
        assert!(manifest_script.contains("size_bytes"));
        assert!(manifest_script.contains("sha256"));
        assert!(!app.contains("UpdateProgressDialog"));
    }

    #[test]
    fn generation_api_preserves_exact_aspect_ratios() {
        for ratio in [
            "1:1", "3:2", "2:3", "4:3", "3:4", "5:4", "4:5", "16:9", "9:16", "2:1", "1:2", "21:9",
            "9:21",
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
            "lifetime_spent": "1",
            "version": value
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
    fn local_crop_uses_normalized_bounds_and_applies_transforms() {
        let source_path =
            std::env::temp_dir().join(format!("artforge-crop-source-{}.png", Uuid::new_v4()));
        let mut source = image::RgbaImage::new(4, 2);
        for y in 0..2 {
            for x in 0..4 {
                source.put_pixel(
                    x,
                    y,
                    if x < 2 {
                        image::Rgba([220, 20, 20, 255])
                    } else {
                        image::Rgba([20, 40, 220, 255])
                    },
                );
            }
        }
        source.save(&source_path).unwrap();

        let cropped = process_crop_result(&source_path, "", (0.5, 0.0, 0.5, 1.0)).unwrap();
        let cropped = image::load_from_memory(&cropped).unwrap().to_rgba8();
        assert_eq!(cropped.dimensions(), (2, 2));
        assert!(cropped.pixels().all(|pixel| pixel.0 == [20, 40, 220, 255]));

        let rotated = process_crop_result(&source_path, "R", (0.0, 0.0, 1.0, 1.0)).unwrap();
        let rotated = image::load_from_memory(&rotated).unwrap();
        assert_eq!((rotated.width(), rotated.height()), (2, 4));

        fs::remove_file(source_path).unwrap();
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
            "watermark, blurry",
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
        assert!(prompt.contains("watermark, blurry"));
    }

    #[test]
    fn ui_generation_prompt_requires_an_isolated_component_atlas() {
        let controls = PromptControls {
            category: "ui".to_string(),
            creation: "ui-hud".to_string(),
            style: "fantasy".to_string(),
            view: "free".to_string(),
            weather: "natural".to_string(),
            time: "natural".to_string(),
            light: "soft".to_string(),
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
            "暗黑地牢风格的战斗界面",
            "",
            &controls,
            &quote,
            "ui",
            "1:1",
            "2K",
            PromptLanguage::Chinese,
        );

        assert!(prompt.contains("UI component atlas rule (mandatory)"));
        assert!(prompt.contains("clean 2D mobile RPG game UI sprite sheet"));
        assert!(prompt.contains("smooth solid color fills"));
        assert!(prompt.contains("simple two-step cel shading"));
        assert!(prompt.contains("about 40 isolated front-facing sprites"));
        assert!(prompt.contains("balanced 6-column atlas"));
        for required_component in [
            "portrait frames",
            "health or energy bars",
            "inventory slots",
            "skill icons",
            "icon-only buttons",
            "virtual joystick",
            "minimap frame",
            "coins or gems",
            "settings gear",
            "treasure chests",
            "dialog or inventory panels",
        ] {
            assert!(prompt.contains(required_component));
        }
        assert!(prompt.contains("Button faces stay blank"));
        assert!(prompt.contains("only isolated UI sprites and whitespace"));
        assert!(prompt.contains("暗黑地牢风格的战斗界面"));
    }

    #[test]
    fn ui_default_controls_do_not_add_free_style_or_natural_light_noise() {
        let controls = PromptControls {
            category: "ui".to_string(),
            creation: "free".to_string(),
            style: "free".to_string(),
            view: "free".to_string(),
            weather: "natural".to_string(),
            time: "natural".to_string(),
            light: "free".to_string(),
        };

        let prompt = prompt_with_controls(
            "clean fantasy inventory",
            &controls,
            PromptLanguage::English,
        );

        assert_eq!(prompt, "clean fantasy inventory");
    }

    #[test]
    fn ui_component_atlas_instruction_is_hidden_from_display_prompt() {
        let generated = append_category_generation_instruction(
            "fantasy inventory icons",
            "ui",
            PromptLanguage::English,
        );

        assert!(generated.contains("UI component atlas rule (mandatory)"));
        assert_eq!(
            display_generation_prompt(&generated),
            "fantasy inventory icons"
        );
    }

    #[test]
    fn non_ui_generation_does_not_add_the_component_atlas_instruction() {
        let prompt = append_category_generation_instruction(
            "a misty mountain village",
            "scene",
            PromptLanguage::English,
        );

        assert_eq!(prompt, "a misty mountain village");
    }

    #[test]
    fn empty_negative_prompt_does_not_add_an_exclusion_section() {
        let prompt =
            append_negative_prompt_instruction("a quiet forest", "   ", PromptLanguage::English);
        assert_eq!(prompt, "a quiet forest");
    }

    #[test]
    fn negative_prompt_drafts_are_scoped_by_workspace_category() {
        let mut drafts = PromptDrafts::default();
        set_negative_prompt_draft_for_category(
            &mut drafts,
            "character",
            "extra fingers".to_string(),
        );
        set_negative_prompt_draft_for_category(&mut drafts, "scene", "people".to_string());

        assert_eq!(
            negative_prompt_draft_for_category(&drafts, "character"),
            "extra fingers"
        );
        assert_eq!(
            negative_prompt_draft_for_category(&drafts, "scene"),
            "people"
        );
        assert_eq!(negative_prompt_draft_for_category(&drafts, "ui"), "");
    }

    #[test]
    fn slash_prompt_history_uses_latest_unique_local_prompts() {
        let mut prompts = vec![
            "  recent prompt  ".to_string(),
            String::new(),
            "recent prompt".to_string(),
        ];
        prompts.extend((0..25).map(|index| format!("prompt-{index}")));

        let history = recent_prompt_history(prompts.iter().map(String::as_str), 20);
        assert_eq!(history.len(), 20);
        assert_eq!(history[0], "recent prompt");
        assert_eq!(history[1], "prompt-0");
        assert_eq!(history[19], "prompt-18");

        let composer = include_str!("../../ui/components/prompt-composer.slint");
        let state = include_str!("../../ui/app-state.slint");
        let sync = include_str!("presentation/sync.rs");
        let callbacks = include_str!("callbacks/generation.rs");
        let local_store = include_str!("storage/local_store.rs");

        assert!(state.contains("in-out property <[string]> prompt-history"));
        assert!(state.contains("in-out property <bool> prompt-history-open"));
        assert!(state.contains("callback remove-prompt-history(string)"));
        assert!(state.contains("callback clear-prompt-history()"));
        assert!(composer.contains("event.text == \"/\""));
        assert!(composer.contains("AppState.prompt == \"\""));
        assert!(composer.contains("AppState.prompt-history-open = true"));
        assert!(composer.contains("root.apply-selected-prompt(AppState.prompt-history[index])"));
        assert!(sync.contains("recent_prompt_history"));
        assert!(sync.contains("dismissed_prompt_history"));
        assert!(sync.contains("20"));
        assert!(callbacks.contains("state.on_remove_prompt_history"));
        assert!(callbacks.contains("state.on_clear_prompt_history"));
        assert!(local_store
            .contains("dismissed_prompt_history: store.dismissed_prompt_history.clone()"));
        assert!(local_store
            .contains("store_mut.dismissed_prompt_history = data.dismissed_prompt_history"));
    }

    #[test]
    fn prompt_history_dismissal_is_independent_and_reversible() {
        let mut store = Store::default();

        assert!(dismiss_prompt_history_entry(
            &mut store,
            "  keep me hidden  "
        ));
        assert!(store.dismissed_prompt_history.contains("keep me hidden"));
        assert!(!dismiss_prompt_history_entry(&mut store, "keep me hidden"));
        assert!(reveal_prompt_history_entry(&mut store, "keep me hidden"));
        assert!(store.dismissed_prompt_history.is_empty());
    }

    #[test]
    fn local_json_replacement_overwrites_existing_files_and_recovers_backups() {
        let directory = std::env::temp_dir().join(format!(
            "artforge-local-json-replacement-{}",
            Uuid::new_v4()
        ));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("local-store.json");
        fs::write(&path, "old").unwrap();

        replace_json_file(&path, "new").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "new");
        assert!(!json_backup_path(&path).exists());
        assert!(!path.with_extension("json.tmp").exists());

        fs::rename(&path, json_backup_path(&path)).unwrap();
        restore_json_backup_if_needed(&path);
        assert_eq!(fs::read_to_string(&path).unwrap(), "new");
        fs::remove_dir_all(directory).unwrap();
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
        assert!(composer.contains("@image-url(\"../../assets/icons/trash.svg\")"));
        assert!(composer.contains("AppState.remove-prompt-history(AppState.prompt-history[index])"));
        assert!(composer.contains("text: AppState.en ? \"Clear all\" : \"全部清空\""));
        assert!(composer.contains("AppState.clear-prompt-history()"));
        assert!(composer.contains("label-font-size: AppState.settings-font-size * 1px - 2px"));
        assert!(composer.contains("visual-opacity: 0.62"));
        assert!(composer.contains("width: 88px;"));
        assert!(composer.contains("height: 24px;"));
    }

    #[test]
    fn prompt_history_hover_opens_full_prompt_preview_on_the_right() {
        let composer = include_str!("../../ui/components/prompt-composer.slint");
        let work_panel = include_str!("../../ui/components/studio-work-panel.slint");
        let split_page = include_str!("../../ui/pages/studio-split-page.slint");
        let preview = fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/ui/components/prompt-history-preview.slint"
        ))
        .unwrap_or_default();
        let history_popup = composer
            .split("history-popup := PopupWindow")
            .nth(1)
            .and_then(|value| value.split("custom-prompt-popup := PopupWindow").next())
            .expect("history popup");
        let history_popup_geometry = history_popup
            .split("history-list := Rectangle")
            .next()
            .expect("history popup geometry");

        assert!(composer.contains("property <int> prompt-history-hovered-index: -1"));
        assert!(!history_popup.contains("history-preview-popup := PopupWindow"));
        assert!(history_popup.contains("history-list := Rectangle"));
        assert!(history_popup_geometry.contains("width: root.history-list-width();"));
        assert!(history_popup_geometry.contains("height: root.history-list-height();"));
        assert!(!history_popup.contains("history-preview := Rectangle"));
        assert!(!composer.contains("function history-popup-width()"));
        assert!(!composer.contains("function history-popup-height()"));
        assert!(!history_popup.contains("history-preview-popup.show();"));
        assert!(!history_popup.contains("history-preview-popup.close();"));
        assert!(history_popup.contains("root.prompt-history-selected-index = index;"));
        assert!(history_popup.contains("index == root.prompt-history-hovered-index"));
        assert!(history_popup.contains("root.sync-history-preview(index, self.has-hover);"));
        assert!(composer.contains("out property <bool> history-preview-open"));
        assert!(composer.contains("out property <string> history-preview-text"));
        assert!(composer.contains("out property <length> history-preview-anchor-y"));
        assert!(work_panel.contains("out property <bool> prompt-history-preview-open"));
        assert!(work_panel.contains("composer.absolute-position.x - root.absolute-position.x"));
        assert!(work_panel.contains("composer.absolute-position.y - root.absolute-position.y"));
        assert!(preview.contains("export component PromptHistoryPreview"));
        assert!(preview.contains("in property <string> prompt"));
        assert!(preview.contains("text: root.prompt"));
        assert!(preview.contains("wrap: word-wrap;"));
        assert!(split_page.contains("history-preview := PromptHistoryPreview"));
        assert!(
            split_page.rfind("GenerationResultPanel {").unwrap()
                < split_page
                    .find("history-preview := PromptHistoryPreview")
                    .unwrap()
        );
    }

    #[test]
    fn prompt_history_preview_stays_open_during_pointer_handoff() {
        let composer = include_str!("../../ui/components/prompt-composer.slint");

        assert!(composer.contains("property <bool> history-preview-close-pending: false"));
        assert!(composer.contains("interval: 160ms;"));
        assert!(composer.contains("running: root.history-preview-close-pending;"));
        assert!(composer.contains("root.sync-history-preview(index, self.has-hover);"));
        assert!(!composer.contains("preview-hover := TouchArea"));
    }

    #[test]
    fn prompt_popups_close_when_their_slash_trigger_is_removed() {
        let composer = include_str!("../../ui/components/prompt-composer.slint");
        let edited_handler = composer
            .split("edited =>")
            .nth(1)
            .and_then(|value| value.split("key-pressed(event)").next())
            .expect("prompt edited handler");

        assert!(edited_handler.contains("AppState.prompt != \"/\""));
        assert!(edited_handler.contains("AppState.prompt-history-open = false"));
        assert!(edited_handler.contains("history-popup.close()"));
        assert!(edited_handler.contains("AppState.prompt != \"//\""));
        assert!(edited_handler.contains("AppState.custom-prompt-open = false"));
        assert!(edited_handler.contains("custom-prompt-popup.close()"));
    }

    #[test]
    fn slash_prompt_popups_support_keyboard_selection_and_confirmation() {
        let composer = include_str!("../../ui/components/prompt-composer.slint");

        assert!(composer.contains("property <int> prompt-history-selected-index: 0"));
        assert!(composer.contains("property <int> custom-prompt-selected-index: -1"));
        assert_eq!(composer.matches("event.text == Key.DownArrow").count(), 2);
        assert_eq!(composer.matches("event.text == Key.UpArrow").count(), 2);
        assert_eq!(composer.matches("event.text == Key.Escape").count(), 2);
        assert!(composer.contains("AppState.prompt-history[root.prompt-history-selected-index]"));
        assert!(composer.contains("AppState.apply-inline-custom-prompt("));
        assert!(composer.contains("root.scroll-prompt-history-selection-into-view()"));
        assert!(composer.contains("index == root.prompt-history-selected-index"));
        assert!(composer.contains("index == root.custom-prompt-selected-index"));
        assert!(composer.contains("root.custom-prompt-selected-index = index"));
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
            store
                .custom_prompt_times
                .get("saved prompt")
                .map(String::as_str),
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
                reference_paths: vec!["reference.png".to_string()],
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
            store
                .custom_prompt_times
                .get("edited prompt")
                .map(String::as_str),
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
        assert!(!remove_custom_prompt_from_store(
            &mut store,
            "missing prompt"
        ));
    }

    #[test]
    fn double_slash_opens_locally_persisted_custom_prompts() {
        let state = include_str!("../../ui/app-state.slint");
        let app = include_str!("../../ui/app.slint");
        let settings = include_str!("../../ui/pages/settings-page.slint");
        let custom_settings = include_str!("../../ui/components/custom-prompt-settings.slint");
        let custom_editor = include_str!("../../ui/pages/custom-prompt-editor-page.slint");
        let composer = include_str!("../../ui/components/prompt-composer.slint");
        let local_store = include_str!("storage/local_store.rs");
        let callbacks = include_str!("callbacks/custom_prompt.rs");

        assert!(state.contains("in-out property <[string]> custom-prompts"));
        assert!(state.contains("in-out property <[CustomPromptItem]> custom-prompt-items"));
        assert!(state.contains("in-out property <bool> custom-prompt-editor-open"));
        assert!(state.contains("callback save-custom-prompt(string, string)"));
        assert!(state.contains("callback remove-custom-prompt(string)"));
        assert!(app.contains("if AppState.page == \"custom-prompt-editor\""));
        assert!(app.contains("CustomPromptEditorPage"));
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
        assert!(custom_editor.contains("AppState.save-custom-prompt"));

        assert!(composer.contains("AppState.prepare-custom-prompt-insertion("));
        assert!(composer.contains("prompt-input.cursor-position-byte-offset"));
        let double_slash_handler = composer
            .split("let trigger-edit = AppState.prepare-custom-prompt-insertion(")
            .nth(1)
            .and_then(|value| value.split("if event.text == Key.Return").next())
            .expect("double slash handler");
        assert!(double_slash_handler.contains("return accept;"));
        assert!(double_slash_handler.contains("root.prompt-editor-text = trigger-edit.text;"));
        assert!(double_slash_handler.contains("root.custom-prompt-insert-offset = trigger-edit.cursor-offset;"));
        let write_position = double_slash_handler
            .find("root.prompt-editor-text = trigger-edit.text;")
            .expect("trigger removal assignment");
        let cursor_position = double_slash_handler
            .find("prompt-input.set-selection-offsets(")
            .expect("trigger cursor assignment");
        assert!(write_position < cursor_position);
        assert!(!double_slash_handler.contains("event.text == Key.Backspace"));
        assert!(composer.contains("history-popup.close()"));
        assert!(composer.contains("custom-prompt-popup.show()"));
        let composer_normalized = composer.replace("\r\n", "\n");
        assert!(composer_normalized.contains(
            "custom-prompt-popup.show();\n                            prompt-input.focus();"
        ));
        assert!(composer_normalized
            .contains("history-popup.show();\n                        prompt-input.focus();"));
        assert!(composer.contains("for item[index] in AppState.custom-prompt-items"));
        assert!(composer.contains("text: item.name"));
        assert!(composer.contains("root.queue-custom-prompt-selection(item.content)"));
        assert!(composer.contains("close-policy: close-on-click-outside"));

        assert!(local_store.contains("custom_prompts: store.custom_prompts.clone()"));
        assert!(
            local_store.contains("selected_custom_prompts: store.selected_custom_prompts.clone()")
        );
        assert!(local_store.contains("custom_prompt_times: store.custom_prompt_times.clone()"));
        assert!(local_store.contains("normalize_custom_prompts(data.custom_prompts)"));
        // The current callback keeps local persistence, now through captured
        // ordered Store enqueue and an actual acknowledgment before editor close.
        let save_entry = core_toolbox_contract_block(callbacks,
            "state.on_save_custom_prompt(", "state.on_remove_custom_prompt(");
        assert!(save_entry.contains("start_custom_prompt_save(&app,context.clone(),save_state.clone(),original.to_string(),prompt.to_string())"));
        let save = core_toolbox_contract_block(callbacks,
            "fn start_custom_prompt_save(", "\nfn poll_custom_prompt_save(");
        assert!(save.contains("captured_custom_editor(&context)"));
        assert!(save.find("persistence.prepare_ordered_save()").unwrap()
            < save.find("save_custom_prompt_to_store(").unwrap());
        assert!(save.find("save_custom_prompt_to_store(").unwrap()
            < save.find("enqueue(local_store_data(app,&store))").unwrap());
        assert!(save.contains("receiver.recv().map_err("));
        assert!(!save.contains("save_local_store("));
        let saved = core_toolbox_contract_block(callbacks,
            "fn poll_custom_prompt_save(", "\n#[cfg(test)]\nthread_local!");
        assert!(saved.find("finish_delivery_preparation(&cancel)").unwrap()
            < saved.find("receiver.try_recv()").unwrap());
        assert!(saved.find("let identity=match result").unwrap()
            < saved.find("state.set_custom_prompt_editor_open(false)").unwrap());
        assert!(callbacks.contains("state.on_save_custom_prompt"));
        let open = core_toolbox_contract_block(callbacks,
            "fn custom_open_editor_metadata(", "\nfn custom_flush_or_mutate");
        assert!(open.contains("state.set_custom_prompt_editor_open(true)"));
        assert!(open.contains("state.set_page(\"custom-prompt-editor\".into())"));
        let begin = core_toolbox_contract_block(callbacks,
            "state.on_begin_new_custom_prompt(", "state.on_begin_edit_custom_prompt(");
        assert!(begin.contains("capture.apply(&app,"));
        assert!(begin.contains("custom_open_editor_metadata(&app)"));
        assert!(callbacks.contains("state.set_custom_prompt_editor_open(false)"));
        assert!(callbacks.contains("state.on_begin_new_custom_prompt"));
        assert!(callbacks.contains("state.on_begin_edit_custom_prompt"));
        assert!(callbacks.contains("state.on_prepare_custom_prompt_insertion"));
        assert!(callbacks.contains("state.on_apply_inline_custom_prompt"));
        assert!(callbacks.contains("state.on_choose_custom_prompt_reference"));
        assert!(
            local_store.contains("custom_prompt_profiles: store.custom_prompt_profiles.clone()")
        );
    }

    #[test]
    fn double_slash_custom_prompts_are_multi_select_inline_names() {
        let composer = include_str!("../../ui/components/prompt-composer.slint");
        let state = include_str!("../../ui/app-state.slint");
        let types = include_str!("../../ui/types.slint");
        let callbacks = include_str!("callbacks/custom_prompt.rs");
        let local_store = include_str!("storage/local_store.rs");
        let controller = include_str!("generation/controller.rs");
        let popup = composer
            .split("custom-prompt-popup := PopupWindow")
            .nth(1)
            .and_then(|value| {
                value
                    .split("function scroll-prompt-history-selection")
                    .next()
            })
            .expect("custom prompt popup");

        assert!(types.contains("selected: bool"));
        assert!(state.contains("in-out property <[CustomPromptItem]> selected-custom-prompt-items"));
        assert!(state.contains("callback toggle-custom-prompt-selection(string)"));
        assert!(callbacks.contains("state.on_toggle_custom_prompt_selection"));
        let target = core_toolbox_contract_block(callbacks,
            "impl CustomEffectTarget{", "\n#[derive(Clone)]\nstruct CustomEffectCapture");
        assert!(target.contains("category:current_workspace_category(app)"));
        let toggle = core_toolbox_contract_block(callbacks,
            "state.on_toggle_custom_prompt_selection(", "state.on_begin_new_custom_prompt(");
        assert!(toggle.find("CustomEffectCapture::capture(&app,&context)").unwrap()
            < toggle.find("slint::Timer::single_shot(Duration::ZERO").unwrap());
        assert!(toggle.contains("CustomStoreMutation::Toggle(prompt.to_string())"));
        assert!(toggle.contains("toggle_custom_prompt_selection_for_category(store,&capture.target.category,&prompt)"));
        assert!(local_store.contains("let was_selected = store"));
        assert!(local_store.contains("selected.insert(prompt.to_string());"));
        assert!(!local_store.contains("selected.clear();"));
        assert!(composer.contains("for item[index] in AppState.selected-custom-prompt-items"));
        assert!(popup.contains("text: item.name"));
        assert!(popup.contains("item.selected ? AppTheme.accent"));
        assert!(popup.contains("root.queue-custom-prompt-selection(item.content)"));
        assert!(!popup.contains("AppState.toggle-custom-prompt-selection(item.content)"));
        assert!(popup.contains("tag-title.preferred-width + 28px"));
        assert!(!composer.contains("function custom-prompt-tag-width()"));
        assert!(!popup.contains("root.apply-selected-prompt(item.content)"));
        assert!(!popup.contains("text: item.content"));
        assert!(!popup.contains("text: item.preview"));
        assert!(controller.contains("compose_inline_custom_prompts"));
        assert!(controller.contains("selected_custom_prompt_replacements_for_category"));
    }

    #[test]
    fn double_slash_selection_shows_a_colored_name_inline_with_the_editable_prompt() {
        let composer = include_str!("../../ui/components/prompt-composer.slint");
        let overlay = include_str!("../../ui/components/inline-custom-prompt-overlay.slint");
        let state = include_str!("../../ui/app-state.slint");
        let callbacks = include_str!("callbacks/custom_prompt.rs");
        let popup = composer
            .split("custom-prompt-popup := PopupWindow")
            .nth(1)
            .and_then(|value| {
                value
                    .split("function scroll-prompt-history-selection")
                    .next()
            })
            .expect("custom prompt popup");

        assert!(composer.contains("prompt-entry-area := Rectangle"));
        assert!(composer.contains("prompt-cursor-area := TouchArea"));
        assert!(composer.contains("mouse-cursor: text;"));
        assert!(composer.contains("property <string> prompt-editor-text: AppState.prompt;"));
        assert!(!composer.contains("property <string> selected-custom-prompt-prefix:"));
        assert!(composer.contains("text <=> root.prompt-editor-text;"));
        assert!(composer.contains("AppState.normalize-prompt-editor-text("));
        assert!(composer.contains(
            "for item[index] in AppState.selected-custom-prompt-items: InlineCustomPromptOverlay"
        ));
        assert!(composer.contains("prefix: item.prefix;"));
        assert!(composer.contains("name: item.name;"));
        assert!(overlay.contains("text: \"　\" + root.name;"));
        assert!(overlay.contains("color: root.marker-color;"));
        assert!(overlay.contains("colorize: root.marker-color;"));
        assert!(composer.contains("color-index: index;"));
        assert!(overlay.contains("font-weight: 600;"));
        assert!(overlay.contains("../../assets/icons/custom-prompt-text.svg"));
        assert!(overlay.contains("width: root.editor-font-size;"));
        assert!(overlay.contains("height: root.editor-font-size;"));
        assert!(!composer.contains("root.width * 0.42"));
        assert!(!composer.contains("for item in AppState.selected-custom-prompt-items: Rectangle"));
        assert!(!composer.contains("selected-prompt-tags := Rectangle"));
        assert!(!composer.contains("selected-prompt-row := HorizontalLayout"));
        assert!(!composer.contains(
            "width: min(max(72px, selected-title.preferred-width + 38px), root.width - 104px);"
        ));
        assert!(!composer
            .contains("x: AppState.selected-custom-prompt-items.length > 0 ? 270px : 24px;"));
        assert!(composer.contains("y: root.prompt-input-y();"));
        assert!(!composer.contains("AppState.selected-custom-prompt-items[0].content"));
        assert!(composer.contains("property <bool> custom-prompt-selection-pending: false;"));
        assert!(composer.contains("function queue-custom-prompt-selection(value: string)"));
        assert!(composer.contains("interval: 1ms;"));
        assert!(composer.contains("running: root.custom-prompt-selection-pending;"));
        assert!(composer.contains("AppState.custom-prompt-open = false"));
        assert!(composer.contains("custom-prompt-popup.close()"));
        assert!(composer.contains("prompt-input.set-selection-offsets(edit.cursor-offset, edit.cursor-offset)"));
        assert!(popup.contains("root.queue-custom-prompt-selection(item.content)"));
        assert!(!popup.contains("AppState.toggle-custom-prompt-selection(item.content)"));
        assert!(composer.contains("root.custom-prompt-selected-index = -1;"));
        assert!(composer.contains("root.custom-prompt-selected-index < 0"));
        assert!(!composer.contains("selected-close-touch := TouchArea"));
        assert!(!composer.contains("selected-title.preferred-width + 38px"));
        assert!(composer.contains("tag-title.preferred-width + 28px"));
        assert!(state.contains("callback normalize-prompt-editor-text(string, string) -> string;"));
        assert!(state.contains("callback apply-inline-custom-prompt(string, string, int) -> PromptTextEdit;"));
        assert!(callbacks.contains("state.on_normalize_prompt_editor_text"));
        assert!(callbacks.contains("insert_custom_prompt_name_at_byte_offset"));
        assert!(callbacks.contains("inline_custom_prompt_occurrences"));
    }

    #[test]
    fn prompt_editors_keep_comfortable_text_metrics() {
        let composer = include_str!("../../ui/components/prompt-composer.slint");
        let expanded = include_str!("../../ui/components/prompt-expanded-editor.slint");

        assert!(composer.contains(
            "property <length> prompt-editor-font-size: AppState.settings-font-size * 1px + 2px;"
        ));
        assert!(composer.contains("font-size: root.prompt-editor-font-size;"));
        assert!(composer.contains("editor-font-size: root.prompt-editor-font-size;"));
        assert!(expanded.contains(
            "property <length> prompt-editor-font-size: AppState.settings-font-size * 1px + 3px;"
        ));
        assert!(expanded.contains("font-size: root.prompt-editor-font-size;"));
        assert!(expanded.contains("editor-font-size: root.prompt-editor-font-size;"));
    }

    #[test]
    fn custom_prompt_palettes_are_distinct_colorful_and_separate_from_each_theme_accent() {
        let cases = [
            ("sprite", (0, 217, 130)),
            ("light", (79, 70, 229)),
            ("ocean", (14, 165, 233)),
            ("warm", (245, 158, 11)),
            ("forest", (34, 197, 94)),
            ("rose", (244, 63, 94)),
            ("cyber", (217, 70, 239)),
            ("oled", (16, 185, 129)),
            ("cream", (201, 107, 115)),
            ("user", (91, 95, 199)),
        ];

        for (theme, accent) in cases {
            let colors = custom_prompt_palette(theme);
            assert_eq!(colors.len(), 6, "{theme} should expose six rotating colors");

            for (index, color) in colors.iter().copied().enumerate() {
                let channels = [color.0, color.1, color.2];
                let spread = channels.iter().max().unwrap() - channels.iter().min().unwrap();
                assert!(spread >= 48, "{theme} color {index} must not be black, white, or gray");

                let accent_distance = (i32::from(color.0) - i32::from(accent.0)).pow(2)
                    + (i32::from(color.1) - i32::from(accent.1)).pow(2)
                    + (i32::from(color.2) - i32::from(accent.2)).pow(2);
                assert!(
                    accent_distance >= 4_096,
                    "{theme} color {index} is too close to the theme accent"
                );
            }

            for left in 0..colors.len() {
                for right in left + 1..colors.len() {
                    assert_ne!(colors[left], colors[right], "{theme} palette repeats a color");
                }
            }
        }

        assert_eq!(custom_prompt_palette("blue"), custom_prompt_palette("ocean"));
        assert_eq!(custom_prompt_palette("system"), custom_prompt_palette("sprite"));
        assert_eq!(custom_prompt_palette("unknown"), custom_prompt_palette("light"));
    }

    #[test]
    fn prompt_editor_expands_into_a_large_editable_overlay() {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let composer = include_str!("../../ui/components/prompt-composer.slint");
        let state = include_str!("../../ui/app-state.slint");
        let app = include_str!("../../ui/app.slint");
        let expanded = std::fs::read_to_string(
            manifest.join("ui/components/prompt-expanded-editor.slint"),
        )
        .unwrap_or_default();

        assert!(state.contains("in-out property <bool> prompt-expanded-open: false;"));
        assert!(composer.contains("prompt-expand-button := Rectangle"));
        assert!(composer.contains("../../assets/icons/fit.svg"));
        assert!(composer.contains("AppState.prompt-expanded-open = true;"));
        assert!(app.contains("import { PromptExpandedEditor }"));
        assert!(app.contains("PromptExpandedEditor { width: root.width; height: root.height; }"));
        assert!(expanded.contains("visible: AppState.prompt-expanded-open;"));
        assert!(expanded.contains("text <=> AppState.prompt;"));
        assert!(expanded.contains("single-line: false;"));
        assert!(expanded.contains("wrap: word-wrap;"));
        assert!(expanded.contains("AppState.invalidate-deep-prompt-binding();"));
        assert!(expanded.contains("AppState.prompt-expanded-open = false;"));
    }

    #[test]
    fn selected_custom_prompt_mask_never_reaches_wrapped_prompt_lines() {
        let overlay = include_str!("../../ui/components/inline-custom-prompt-overlay.slint");
        assert!(overlay.contains("export component InlineCustomPromptOverlay inherits Rectangle"));
        assert!(overlay.contains("clip: true;"));
        assert!(overlay.contains("height: max(token-metric.preferred-height, root.editor-font-size + 3px);"));
        assert!(!overlay.contains("height: root.editor-visible-height;\n        background: root.editor-background"));
    }

    #[test]
    fn inline_custom_prompt_mask_does_not_cover_the_following_caret_lane() {
        let overlay = include_str!("../../ui/components/inline-custom-prompt-overlay.slint");
        let mask = overlay
            .split("token-mask := Rectangle")
            .nth(1)
            .and_then(|value| value.split("Timer {").next())
            .expect("inline custom prompt token mask");

        assert!(mask.contains("width: token-metric.preferred-width;"));
        assert!(
            !mask.contains("token-metric.preferred-width +"),
            "the token mask must not extend into the following caret lane"
        );
    }

    #[test]
    fn multiple_custom_prompt_selections_are_isolated_by_workspace_category() {
        let mut store = Store {
            custom_prompts: vec![
                "角色提示词".to_string(),
                "角色提示词二".to_string(),
                "场景提示词".to_string(),
            ],
            ..Store::default()
        };

        toggle_custom_prompt_selection_for_category(&mut store, "character", "角色提示词");
        assert!(custom_prompt_selected_for_category(
            &store,
            "character",
            "角色提示词"
        ));
        assert!(!custom_prompt_selected_for_category(
            &store,
            "scene",
            "角色提示词"
        ));
        assert_eq!(
            selected_custom_prompts_for_category(&store, "character"),
            vec!["角色提示词".to_string()]
        );
        assert!(selected_custom_prompts_for_category(&store, "scene").is_empty());

        toggle_custom_prompt_selection_for_category(&mut store, "character", "角色提示词二");
        assert_eq!(
            selected_custom_prompts_for_category(&store, "character"),
            vec!["角色提示词".to_string(), "角色提示词二".to_string()]
        );

        toggle_custom_prompt_selection_for_category(&mut store, "scene", "场景提示词");
        assert_eq!(
            selected_custom_prompts_for_category(&store, "scene"),
            vec!["场景提示词".to_string()]
        );
        assert_eq!(
            selected_custom_prompts_for_category(&store, "character"),
            vec!["角色提示词".to_string(), "角色提示词二".to_string()]
        );

        toggle_custom_prompt_selection_for_category(&mut store, "character", "角色提示词二");
        assert_eq!(
            selected_custom_prompts_for_category(&store, "character"),
            vec!["角色提示词".to_string()]
        );
        assert_eq!(
            selected_custom_prompts_for_category(&store, "scene"),
            vec!["场景提示词".to_string()]
        );
    }

    #[test]
    fn selected_custom_prompt_replacements_include_their_display_names() {
        let first = "PIXEL CONTENT".to_string();
        let second = "CAMERA CONTENT".to_string();
        let mut store = Store {
            custom_prompts: vec![first.clone(), second.clone()],
            ..Store::default()
        };
        store.custom_prompt_profiles.insert(
            first.clone(),
            CustomPromptProfile {
                name: "像素模板".to_string(),
                ..CustomPromptProfile::default()
            },
        );
        store.custom_prompt_profiles.insert(
            second.clone(),
            CustomPromptProfile {
                name: "镜头模板".to_string(),
                ..CustomPromptProfile::default()
            },
        );
        toggle_custom_prompt_selection_for_category(&mut store, "scene", &first);
        toggle_custom_prompt_selection_for_category(&mut store, "scene", &second);

        assert_eq!(
            selected_custom_prompt_replacements_for_category(&store, "scene"),
            vec![
                ("像素模板".to_string(), first),
                ("镜头模板".to_string(), second),
            ]
        );
    }

    #[test]
    fn selected_custom_prompt_name_keeps_the_default_placeholder_hidden_without_focus() {
        let composer = include_str!("../../ui/components/prompt-composer.slint");
        let placeholder = composer
            .split("text: root.prompt-placeholder()")
            .next()
            .and_then(|value| value.rsplit("Text {").next())
            .expect("prompt placeholder");

        assert!(placeholder.contains("root.prompt-editor-text == \"\""));
        assert!(placeholder.contains("AppState.selected-custom-prompt-items.length == 0"));
        assert!(placeholder.contains("!prompt-input.has-focus"));
    }

    #[test]
    fn selected_custom_prompt_contents_are_composed_only_for_interactive_generation() {
        let selected = vec![
            ("灯光模板".to_string(), "portrait lighting".to_string()),
            ("纹理模板".to_string(), "  ink texture  ".to_string()),
        ];

        assert_eq!(
            compose_inline_custom_prompts("main subject", &selected),
            "portrait lighting\n\nink texture\n\nmain subject"
        );
        assert_eq!(
            compose_inline_custom_prompts("//", &selected),
            "portrait lighting\n\nink texture"
        );
        assert_eq!(compose_inline_custom_prompts("", &[]), "");
    }

    #[test]
    fn inline_custom_prompt_contents_replace_names_without_changing_text_order() {
        let replacements = vec![
            ("像素模板".to_string(), "PIXEL CONTENT".to_string()),
            ("镜头模板".to_string(), "CAMERA CONTENT".to_string()),
        ];

        assert_eq!(
            compose_inline_custom_prompts(
                "前文 \u{3000}像素模板 中段 \u{3000}镜头模板 后文",
                &replacements,
            ),
            "前文 PIXEL CONTENT 中段 CAMERA CONTENT 后文"
        );
    }

    #[test]
    fn custom_prompt_choices_wrap_inside_the_popup_and_remain_selectable() {
        use i_slint_backend_testing::{ElementHandle, TestingBackend, TestingBackendOptions};
        use slint::platform::{Key, PointerEventButton, WindowEvent};

        slint::platform::set_platform(Box::new(TestingBackend::new(TestingBackendOptions {
            mock_time: true,
            renderer_name: Some("software".into()),
            ..Default::default()
        }))).unwrap();
        let app = AppWindow::new().unwrap();
        let state = app.global::<AppState>();
        state.set_logged_in(true);
        state.set_page("generation".into());
        state.set_contact_popup_open(false);
        let names = ["CG电影", "测试标题3", "测试标题2", "测试标题1", "中国古风质感人物与场景", "像素模板", "横板闯关场景", "电影镜头"];
        state.set_custom_prompt_items(ModelRc::new(VecModel::from(names.iter().enumerate().map(|(index, name)| CustomPromptItem {
            name: (*name).into(), content: format!("prompt-{index}").into(), ..Default::default()
        }).collect::<Vec<_>>())));
        // Exercise the real editor trigger without writing any user drafts to disk.
        let weak = app.as_weak();
        state.on_normalize_prompt_editor_text(move |text, _| {
            weak.unwrap().global::<AppState>().set_prompt(text.clone());
            text
        });
        let weak = app.as_weak();
        state.on_prepare_custom_prompt_insertion(move |text, offset| {
            let edit = remove_custom_prompt_trigger_before_cursor(&text, offset.max(0) as usize);
            weak.unwrap().global::<AppState>().set_prompt(edit.text.clone().into());
            PromptTextEdit { text: edit.text.into(), cursor_offset: edit.cursor_offset }
        });
        let selections = Rc::new(RefCell::new(Vec::<String>::new()));
        let observed = selections.clone();
        state.on_apply_inline_custom_prompt(move |content, text, cursor_offset| {
            observed.borrow_mut().push(content.to_string());
            PromptTextEdit { text, cursor_offset }
        });
        let press = |text: SharedString| {
            app.window().dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
            app.window().dispatch_event(WindowEvent::KeyReleased { text });
        };
        app.show().unwrap();

        for (width, theme, font_size) in [(1050.0, "light", 14), (1440.0, "ocean", 14), (1920.0, "sprite", 18)] {
            apply_theme(&app, theme);
            state.set_settings_font_size(font_size);
            app.window().set_size(slint::LogicalSize::new(width, 900.0));
            state.set_prompt("/".into());
            ElementHandle::find_by_element_id(&app, "PromptComposer::prompt-input")
                .next().unwrap().mock_single_click(PointerEventButton::Left);
            press(Key::End.into());
            press("/".into());
            assert!(state.get_custom_prompt_open(), "double slash opens the picker");
            let viewport = ElementHandle::find_by_element_id(&app, "PromptComposer::custom-prompt-scroll")
                .next().unwrap();
            let labels = ElementHandle::find_by_element_id(&app, "PromptComposer::tag-title").collect::<Vec<_>>();
            assert_eq!(labels.len(), names.len());
            let top = labels[0].absolute_position().y;
            assert!(labels.iter().any(|label| label.absolute_position().y > top + 20.0),
                "choices must wrap to additional rows at window width {width}");
            let bounds = viewport.absolute_position();
            let size = viewport.size();
            for label in &labels {
                let pos = label.absolute_position();
                assert!(pos.x >= bounds.x && pos.x + label.size().width <= bounds.x + size.width + 1.0,
                    "every choice must fit horizontally without scrolling: {pos:?}, viewport {bounds:?} {size:?}");
                assert!(pos.y >= bounds.y && pos.y + label.size().height <= bounds.y + size.height + 1.0,
                    "popup must expand to show its wrapped choices");
            }
            let footer = ElementHandle::find_by_accessible_label(&app, "管理").next().unwrap();
            assert!(footer.absolute_position().y >= bounds.y + size.height,
                "footer must remain below the wrapped choices");
            assert!(viewport.query_descendants().match_inherits("ScrollBar").find_all().is_empty(),
                "the choice list must not render scrollbar tracks");
            if let Some(directory) = std::env::var_os("ELUNVI_TEST_ARTIFACT_DIR") {
                let directory = PathBuf::from(directory);
                fs::create_dir_all(&directory).unwrap();
                let pixels = app.window().take_snapshot().unwrap();
                image::save_buffer(directory.join(format!("custom-prompt-wrap-{width}.png")), pixels.as_bytes(), pixels.width(), pixels.height(), image::ColorType::Rgba8).unwrap();
            }
            press(Key::Escape.into());
            assert!(!state.get_custom_prompt_open());
        }

        state.set_prompt("/".into());
        ElementHandle::find_by_element_id(&app, "PromptComposer::prompt-input")
            .next().unwrap().mock_single_click(PointerEventButton::Left);
        press(Key::End.into());
        press("/".into());
        let choice = ElementHandle::find_by_element_id(&app, "PromptComposer::custom-row").last().unwrap();
        let editor = ElementHandle::find_by_element_id(&app, "PromptComposer::prompt-input").next().unwrap();
        // Popup element coordinates are popup-local in the testing backend.
        // Translate to the main window using the editor-anchored popup origin.
        let position = slint::LogicalPosition::new(
            editor.absolute_position().x + choice.absolute_position().x + choice.size().width / 2.0,
            editor.absolute_position().y + 32.0 + choice.absolute_position().y + choice.size().height / 2.0,
        );
        app.window().dispatch_event(WindowEvent::PointerMoved { position });
        app.window().dispatch_event(WindowEvent::PointerPressed { position, button: PointerEventButton::Left });
        app.window().dispatch_event(WindowEvent::PointerReleased { position, button: PointerEventButton::Left });
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(20));
        slint::platform::update_timers_and_animations();
        assert_eq!(selections.borrow().as_slice(), ["prompt-7"]);
        assert!(!state.get_custom_prompt_open());

        state.set_prompt("/".into());
        ElementHandle::find_by_element_id(&app, "PromptComposer::prompt-input")
            .next().unwrap().mock_single_click(PointerEventButton::Left);
        press(Key::End.into());
        press("/".into());
        press(Key::UpArrow.into());
        press(Key::Return.into());
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(20));
        slint::platform::update_timers_and_animations();
        assert_eq!(selections.borrow().as_slice(), ["prompt-7", "prompt-7"]);
        assert!(!state.get_custom_prompt_open());

        state.set_custom_prompt_items(ModelRc::new(VecModel::from((0..100).map(|index| CustomPromptItem {
            name: if index == 50 { "很长的自定义提示词名称".repeat(12).into() } else { format!("自定义提示词{index}").into() },
            content: format!("long-list-{index}").into(), ..Default::default()
        }).collect::<Vec<_>>())));
        state.set_prompt("/".into());
        ElementHandle::find_by_element_id(&app, "PromptComposer::prompt-input")
            .next().unwrap().mock_single_click(PointerEventButton::Left);
        press(Key::End.into());
        press("/".into());
        press(Key::UpArrow.into());
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(20));
        slint::platform::update_timers_and_animations();
        let last = ElementHandle::find_by_accessible_label(&app, "自定义提示词99").next().unwrap();
        let viewport = ElementHandle::find_by_element_id(&app, "PromptComposer::custom-prompt-scroll").next().unwrap();
        assert!(last.absolute_position().y >= viewport.absolute_position().y);
        assert!(last.absolute_position().y + last.size().height <= viewport.absolute_position().y + viewport.size().height + 1.0,
            "keyboard selection must bring the last wrapped row into view");
        assert!(viewport.size().height <= 360.0, "a long list must stay inside the window");
        press(Key::Return.into());
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(20));
        slint::platform::update_timers_and_animations();
        assert_eq!(selections.borrow().last().unwrap(), "long-list-99");
    }

    #[test]
    fn image_generation_defaults_to_2k_quality() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().expect("create app window");

        assert_eq!(app.global::<AppState>().get_quality().as_str(), "2K");
    }

    #[test]
    fn populated_custom_prompt_popup_exposes_low_emphasis_create_and_manage_actions() {
        let composer = include_str!("../../ui/components/prompt-composer.slint");
        let popup = composer
            .split("custom-prompt-popup := PopupWindow")
            .nth(1)
            .and_then(|value| {
                value
                    .split("function scroll-prompt-history-selection")
                    .next()
            })
            .expect("custom prompt popup");

        assert_eq!(
            popup
                .matches("if AppState.custom-prompt-items.length > 0: PillButton")
                .count(),
            2
        );
        assert!(popup.contains("text: AppState.en ? \"Manage\" : \"管理\""));
        assert!(popup.contains("text: AppState.en ? \"Create\" : \"创建\""));
        assert_eq!(popup.matches("primary: false").count(), 2);
        assert!(popup.contains("AppState.settings-section = \"prompts\""));
        assert!(popup.contains("AppState.navigate(\"settings\")"));
        assert!(popup.contains("AppState.begin-new-custom-prompt()"));
    }

    #[test]
    fn custom_prompt_editor_uses_the_structured_reference_form() {
        let page = include_str!("../../ui/pages/custom-prompt-editor-page.slint");
        let app = include_str!("../../ui/app.slint");
        let state = include_str!("../../ui/app-state.slint");
        let callbacks = include_str!("callbacks/custom_prompt.rs");
        let prompt_tasks = include_str!("callbacks/prompt_tasks.rs");
        let picker = callbacks.split_once("fn start_custom_picker(").unwrap().1
            .split_once("fn prepare_custom_existing_previews(").unwrap().0;
        let run = prompt_tasks.split_once("fn run_prompt_record(").unwrap().1
            .split_once("fn require_prompt_patch(").unwrap().0;
        let request = prompt_tasks.split_once("fn prompt_task_create_request(").unwrap().1
            .split_once("fn prompt_task_api_error_is_transient(").unwrap().0;

        for field in [
            "custom-prompt-name",
            "custom-prompt-category",
            "custom-prompt-format",
            "custom-prompt-negative",
            "custom-prompt-reference-path",
            "custom-prompt-reference-image",
            "custom-prompt-reference-items",
        ] {
            assert!(state.contains(field), "missing state field {field}");
        }
        assert!(app.contains("if AppState.page == \"custom-prompt-editor\""));
        assert!(page.contains("left-panel := Rectangle"));
        assert!(page.contains("right-panel := Rectangle"));
        assert!(page.contains("x: left-panel.width + 18px"));
        assert!(page.contains("提示词名称 *"));
        assert!(page.contains("PromptCategorySelect"));
        assert!(page.contains("上传参考图"));
        assert!(page.contains("AI 分析风格"));
        assert!(page.contains("保存格式"));
        assert!(page.contains("提示词内容 *"));
        assert!(page.contains("反向提示词（仅 JSON 格式有效）"));
        assert!(page.contains("AppState.choose-custom-prompt-reference()"));
        assert!(page.contains("for item[index] in AppState.custom-prompt-reference-items"));
        assert!(page.contains("Math.mod(index, 4)"));
        assert!(page.contains("AppState.open-custom-prompt-reference(item.id)"));
        assert!(page.contains("AppState.remove-custom-prompt-reference(item.id)"));
        assert!(page.contains("AppState.custom-prompt-reference-items.length >= 8"));
        assert!(!page.contains("text: AppState.custom-prompt-reference-path"));
        assert!(page.contains("AppState.close-custom-prompt-editor()"));
        assert!(state.contains("callback analyze-custom-prompt-reference();"));
        assert!(state.contains("callback close-custom-prompt-editor();"));
        assert!(page.contains("AppState.analyze-custom-prompt-reference();"));
        assert!(page.contains("disabled: !AppState.style-analysis-available"));
        assert!(page.contains("AppState.custom-prompt-reference-items.length == 0"));
        assert!(!page.contains("等待服务端开放图片风格分析"));
        assert!(callbacks.contains("state.on_analyze_custom_prompt_reference"));
        assert!(callbacks.contains("state.on_remove_custom_prompt_reference"));
        assert!(callbacks.contains("state.on_open_custom_prompt_reference"));
        assert!(picker.contains(".pick_files()"));
        assert!(callbacks.contains("MAX_CUSTOM_PROMPT_REFERENCES: usize = 8"));
        assert!(!page.contains("Analyzed locally; the image is not uploaded"));
        assert!(!page.contains("由本地客户端分析，不会上传参考图"));
        assert!(state.contains("custom-prompt-analyzing"));
        assert!(callbacks.contains("sync_style_analysis_selection(&state)"));
        assert!(callbacks.contains("start_backend_prompt_task("));
        // Structure only: original held namespace input replaces the retired
        // raw-path/scoped-upload adapter; this does not prove successful upload.
        assert!(run.contains("GenerationApi::new(capture.backend.api.clone())"));
        assert!(run.contains("api.upload_reference_for_namespace_checked("));
        assert!(run.contains("&capture.authority,&capture.scope"));
        assert!(run.contains("&record.reference_sha256[index],record.reference_size_bytes[index]"));
        assert!(request.contains("reference_file_ids: (!record.uploaded_file_ids.is_empty())"));
        assert!(run.contains("detail.result_prompt"));
        assert!(!callbacks.contains("analyze_reference_style("));
    }

    #[test]
    fn custom_prompt_reference_analysis_uses_the_selected_server_model() {
        let callbacks = include_str!("callbacks/custom_prompt.rs");
        let prompt_tasks = include_str!("callbacks/prompt_tasks.rs");

        let analysis = callbacks.split_once("state.on_analyze_custom_prompt_reference(").unwrap().1
            .split_once("state.on_save_custom_prompt(").unwrap().0;
        let request = prompt_tasks.split_once("fn prompt_task_create_request(").unwrap().1
            .split_once("fn prompt_task_api_error_is_transient(").unwrap().0;
        let run = prompt_tasks.split_once("fn run_prompt_record(").unwrap().1
            .split_once("fn require_prompt_patch(").unwrap().0;
        assert!(analysis.contains("selection=sync_style_analysis_selection(&state)"));
        assert!(analysis.contains("if !selection.available"));
        assert!(analysis.contains("model_code:selection.model_code,task_type:\"image_style_analysis\""));
        assert!(analysis.contains("reference_paths:paths"));
        assert!(analysis.contains("start_backend_prompt_task(&app,context.clone(),request)"));
        assert!(request.contains("task_type: record.task_type.clone()"));
        assert!(request.contains("model_code: record.model_code.clone()"));
        assert!(request.contains("reference_file_ids: (!record.uploaded_file_ids.is_empty())"));
        assert!(request.contains("record.uploaded_file_ids.clone()"));
        // New submission and retained replay consume the original saved representation.
        assert!(run.contains("api.create_task_billing(&prompt_task_create_request(&record),&scope)"));
        assert!(run.contains("SavedReplayRequest::prompt(capture.authority.clone(),&capture.scope,&record.client_request_id)"));
        assert!(run.contains("worker.wait(Duration::from_millis(IMAGE_POLL_INTERVAL_MS))"));
    }

    #[test]
    fn style_analysis_actions_use_server_catalog_capability_and_standard_price() {
        let state = include_str!("../../ui/app-state.slint");
        let types = include_str!("../../ui/types.slint");
        let auth = include_str!("callbacks/auth.rs");
        let selector = include_str!("callbacks/model_catalog.rs");
        let generation = include_str!("callbacks/generation.rs");
        let custom_prompt = include_str!("callbacks/custom_prompt.rs");
        let composer = include_str!("../../ui/components/prompt-composer.slint");
        let custom_prompt_page = include_str!("../../ui/pages/custom-prompt-editor-page.slint");

        for property in [
            "style-analysis-available",
            "style-analysis-model-code",
            "style-analysis-display-name",
            "style-analysis-credit-cost",
        ] {
            assert!(
                state.contains(property),
                "missing style selector state {property}"
            );
        }
        assert!(types.contains("price-standard: string"));
        assert!(auth.contains("model_credit_cost(model, \"standard\")"));
        assert!(selector.contains("model.supports_style_analysis"));
        assert!(selector.contains("!model.price_standard.trim().is_empty()"));
        assert!(generation.contains("sync_style_analysis_selection(&state)"));
        assert!(custom_prompt.contains("sync_style_analysis_selection(&state)"));
        for page in [composer, custom_prompt_page] {
            assert!(page.contains("AppState.style-analysis-available"));
            assert!(page.contains("AppState.style-analysis-credit-cost"));
        }
        assert!(!state.contains("style-analysis-credit-cost: \"5\""));
        assert!(!composer.contains("style-analysis-credit-cost + \"5\""));
        assert!(!custom_prompt_page.contains("style-analysis-credit-cost + \"5\""));
    }

    #[test]
    fn prompt_task_results_decode_json_and_unicode_wrappers() {
        assert_eq!(normalize_prompt_task_result(r#"("\u5730\u7262")"#), "地牢");
        assert_eq!(
            normalize_prompt_task_result(r#"{"result_prompt":"\u6e38\u620f UI"}"#),
            "游戏 UI"
        );
        assert_eq!(normalize_prompt_task_result("普通提示词"), "普通提示词");
    }

    #[test]
    fn prompt_composer_accepts_local_and_browser_image_drops() {
        let composer = include_str!("../../ui/components/prompt-composer.slint");
        let upload_card = include_str!("../../ui/components/upload-card.slint");
        let callbacks = include_str!("callbacks/reference.rs");
        let viewer = include_str!("callbacks/viewer.rs");
        let platform = include_str!("../platform.rs");
        let app = include_str!("app.rs");
        let picker = callbacks.split_once("state.on_add_reference(").unwrap().1
            .split_once("state.on_paste_reference(").unwrap().0;
        let transfer = callbacks.split_once("state.on_add_reference_from_transfer(").unwrap().1
            .split_once("state.on_remove_reference(").unwrap().0;
        let import = callbacks.split_once("fn start_reference_import(").unwrap().1
            .split_once("fn start_reference_url_for_context(").unwrap().0;
        let external = callbacks.split_once("fn process_captured_external_image_drops(").unwrap().1
            .split_once("fn external_drop_inside_reference_input(").unwrap().0;

        assert!(composer.contains("label: AppState.en ? \"Add image\" : \"添加图片\""));
        assert!(upload_card.contains("in property <string> label"));
        assert!(upload_card.contains("text: root.label"));
        assert!(upload_card.contains("border-radius: 10px"));
        assert!(composer.contains("width: 88px"));
        assert!(composer.contains("return 8;"));
        assert!(picker.contains("ReferenceCapture::new(&app,&context)"));
        assert!(picker.contains("start_reference_import(&app,context,capture,ReferenceSource::Paths(paths))"));
        assert!(composer.contains("reference-drop := DropArea"));
        let drop_layer_position = composer
            .find("reference-drop := DropArea")
            .expect("reference drop layer");
        let interactive_layer_position = composer
            .find("if AppState.quote-title")
            .expect("first interactive layer");
        assert!(drop_layer_position < interactive_layer_position);
        assert!(composer.contains("return DragAction.copy;"));
        assert!(composer.contains("AppState.add-reference-from-transfer(event.data)"));
        assert!(composer.contains("reference-drop.has-drag"));
        let drop_layer = composer
            .split("reference-drop := DropArea")
            .nth(1)
            .and_then(|value| value.split("if AppState.quote-title").next())
            .expect("reference drop block");
        assert!(drop_layer.contains("x: 0px;"));
        assert!(drop_layer.contains("y: 0px;"));
        assert!(drop_layer.contains("width: parent.width;"));
        assert!(drop_layer.contains("height: parent.height;"));
        assert!(composer
            .contains("AppState.reference-drop-x = reference-drop.absolute-position.x / 1px"));
        assert!(composer.contains("changed width => { root.sync-reference-drop-bounds(); }"));
        assert!(!composer.contains("interval: 50ms;\n        running: true;"));
        assert!(transfer.contains("transfer.plain_text()"));
        assert!(transfer.contains("external_image_url(data.as_str())"));
        assert!(transfer.contains("ReferenceSource::Url(url)"));
        assert!(transfer.contains("drag_data_to_paths(data.as_str())"));
        assert!(transfer.contains("ReferenceSource::Paths(paths)"));
        assert!(import.contains("spawn_reference_work(app,context,capture"));
        assert!(import.contains("download_captured_reference_bytes(&url,persistence)"));
        assert!(import.contains("decode_owned_reference_source(&authority,&path)"));
        assert!(import.contains("persist_reference_image_for_namespace(&authority,&image)"));
        assert!(external.contains("platform::take_external_image_drops()"));
        assert!(callbacks.contains("on_process_external_image_drops"));
        assert!(!callbacks.contains("poll_external_image_drops"));
        assert!(external.contains("ExternalImageDrop::Paths"));
        assert!(external.contains("ExternalImageDrop::Text"));
        assert!(external.contains("external_drop_inside_reference_input"));
        assert!(external.contains("start_reference_paths_for_context(app,context.clone(),paths)"));
        assert!(external.contains("start_reference_url_for_context(app,context.clone(),url)"));
        assert!(callbacks.contains("position.physical"));
        assert!(viewer.contains("pub(super) fn external_image_url"));
        assert!(viewer.contains("pub(super) fn drag_data_to_paths"));
        assert!(viewer.contains("let url = reqwest::Url::parse(raw).ok()?;"));
        assert!(viewer.contains("url.to_file_path()"));
        assert!(platform.contains("IDropTarget"));
        assert!(platform.contains("RegisterDragDrop"));
        assert!(platform.contains("CF_HDROP"));
        assert!(platform.contains("\"text/uri-list\""));
        assert!(platform.contains("\"text/html\""));
        assert!(platform.contains("mod macos_drop_target"));
        assert!(platform.contains("NSFilenamesPboardType"));
        assert!(platform.contains("class_replaceMethod"));
        assert!(platform.contains("sel!(performDragOperation:)"));
        assert!(platform.contains("ExternalImageDrop::Paths(paths, position)"));
        assert!(platform.contains("ScreenToClient"));
        assert!(platform.contains("draggingLocation"));
        assert!(!platform.contains("AnyObject::set_class"));
        assert!(app.contains("schedule_external_image_drop_install"));
    }

    #[test]
    fn regenerate_restores_and_reuploads_original_references() {
        let model = include_str!("model.rs").replace("\r\n", "\n");
        let storage = include_str!("storage/local_store.rs");
        let controller = include_str!("generation/controller.rs");
        let poll = include_str!("generation/poll.rs");
        let backend = include_str!("generation/backend.rs");
        let generation_callbacks = include_str!("callbacks/generation.rs");
        let viewer_callbacks = include_str!("callbacks/viewer.rs");

        assert!(model.contains("reference_paths: Vec<String>"));
        assert!(model.contains("#[serde(default)]\n    reference_paths: Vec<String>"));
        assert!(storage.contains("reference_paths: asset.reference_paths.clone()"));
        assert!(storage.contains("reference_paths: asset.reference_paths"));
        assert!(backend.contains("generation_reference_paths"));
        assert!(backend.contains("reference_file_ids: Some(uploaded.clone())"));
        assert!(poll.contains("&generation_reference_paths"));
        assert!(controller.contains("reference_paths: reference_paths.to_vec()"));
        assert!(controller.contains("restore_asset_regeneration_inputs"));
        assert!(controller.contains("references_for_category_mut"));
        assert!(controller.contains("load_preview_image(&path, PreviewPurpose::Reference)"));
        assert!(generation_callbacks.contains("start_asset_regeneration"));
        assert!(viewer_callbacks.contains("start_asset_regeneration"));
        assert!(viewer_callbacks.contains("persist_slint_reference"));
    }

    #[test]
    fn regenerate_keeps_an_existing_generation_visible() {
        let controller = include_str!("generation/controller.rs");
        let backend = include_str!("generation/backend.rs");
        let callbacks = include_str!("callbacks/generation.rs");

        assert!(controller.contains("ExistingGenerationPolicy::KeepExisting"));
        assert!(controller.contains("push_generations(app, &store)"));
        assert!(controller.contains("sync_generation_state_for_current_category(context, app)"));
        assert!(callbacks.contains("ExistingGenerationPolicy::StopExisting"));
        assert!(backend.contains("ExistingGenerationPolicy::KeepExisting"));
        assert!(backend.contains("已保留正在进行中的任务"));
        assert!(backend.contains("sync_generation_state_for_current_category(&context, app)"));
        assert!(backend.contains("navigate_to_with_store(app"));
    }

    #[test]
    fn external_image_drop_extracts_plain_and_html_urls() {
        assert_eq!(
            external_image_url("https://cdn.example.com/reference.png?size=large").as_deref(),
            Some("https://cdn.example.com/reference.png?size=large")
        );
        assert_eq!(
            external_image_url(
                "<img alt=\"reference\" src=\"https://cdn.example.com/reference.webp\">"
            )
            .as_deref(),
            Some("https://cdn.example.com/reference.webp")
        );
        assert_eq!(
            external_image_url(
                "Version:0.9\r\nSourceURL:https://cdn.example.com/reference.jpg\r\n<html></html>"
            )
            .as_deref(),
            Some("https://cdn.example.com/reference.jpg")
        );
        assert!(external_image_url("file:///C:/images/reference.png").is_none());
        assert!(external_image_url("C:\\images\\reference.png").is_none());
    }

    #[test]
    fn finder_and_file_manager_drops_preserve_absolute_paths_and_multiple_files() {
        let paths = drag_data_to_paths(
            "# Finder drag\r\nfile:///Users/demo/first%20image.png\r\nfile:///Users/demo/second.jpg\r\n",
        );

        assert_eq!(paths.len(), 2);
        assert_eq!(
            paths[0].file_name().and_then(|name| name.to_str()),
            Some("first image.png")
        );
        assert_eq!(
            paths[1].file_name().and_then(|name| name.to_str()),
            Some("second.jpg")
        );
        #[cfg(not(windows))]
        assert!(paths[0].is_absolute());
    }

    #[test]
    fn custom_prompt_page_uses_compact_bordered_list_rows() {
        let settings = include_str!("../../ui/components/custom-prompt-settings.slint");
        let types = include_str!("../../ui/types.slint");
        let sync = include_str!("presentation/sync.rs");

        assert!(settings.contains("for item in AppState.custom-prompt-items"));
        assert!(settings.contains("height: 68px;"));
        assert!(settings.contains("border-width: 1px;"));
        assert!(settings.contains("border-color: AppTheme.border;"));
        assert!(settings.contains("text: item.format == \"json\" ? \"JSON\" : \"TXT\""));
        assert!(settings.contains("root.category-label(item.category)"));
        assert!(settings.contains("background: transparent;"));
        assert!(types.contains("category: string"));
        assert!(types.contains("format: string"));
        assert!(sync.contains("category: normalized_custom_prompt_category"));
        assert!(sync.contains("format: normalized_custom_prompt_format"));
    }

    #[test]
    fn custom_prompt_editor_allows_ime_to_handle_composition_keys() {
        let page = include_str!("../../ui/pages/custom-prompt-editor-page.slint");
        let prompt_input = page
            .split("prompt-input := TextInput")
            .nth(1)
            .and_then(|value| value.split("if AppState.custom-prompt-input").next())
            .expect("custom prompt content input");

        assert!(page.contains("init => { prompt-name-input.focus(); }"));
        assert!(page.matches("input-type: text;").count() >= 3);
        assert!(!prompt_input.contains("key-pressed(event)"));
        assert!(!prompt_input.contains("root.save-prompt()"));
    }

    #[test]
    fn windows_file_drag_runs_on_the_pointer_thread_after_releasing_capture() {
        let drag = include_str!("../drag_preview.rs");
        let references = include_str!("callbacks/reference.rs");
        let viewer = include_str!("callbacks/viewer.rs");
        let handler = drag
            .split("pub(crate) fn start_thumbnail_file_drag_captured(drag: CapturedNativeFileDrag) -> bool")
            .nth(1)
            .and_then(|value| value.split("#[cfg(target_os = \"macos\")]").next())
            .expect("Windows file drag handler");
        let reset = references.split_once("fn reference_reset_pointer(").unwrap().1
            .split_once("fn start_reference_native_drag(").unwrap().0;
        let pointer = references.split_once("fn reference_pointer_exit(").unwrap().1
            .split_once("fn reference_path_in_store(").unwrap().0;
        let native = references.split_once("fn start_reference_native_drag(").unwrap().1
            .split_once("fn wire_reference_callbacks(").unwrap().0;
        let viewer_drag = viewer.split_once("state.on_start_viewer_file_drag(").unwrap().1
            .split_once("state.on_viewer_cutout_image(").unwrap().0;

        assert_eq!(handler.matches("ReleaseCapture()").count(), 2);
        assert!(handler.contains("drag.consume(|path|"));
        assert!(handler.contains("windows_file_drag::run(path.to_owned()).is_ok()"));
        assert!(!handler.contains("std::thread::spawn"));
        assert!(drag
            .contains("DoDragDrop(&data_object, &drop_source, DROPEFFECT_COPY, &mut effect).ok()"));
        // The shared captured completion, not either old unbound caller, owns reset.
        assert!(pointer.contains("WindowEvent::PointerExited"));
        assert!(reset.contains("slint::Timer::single_shot(Duration::ZERO"));
        assert!(reset.contains("ticket.current() && capture.current(&app,&context)"));
        assert!(reset.contains("reference_pointer_exit(&app)"));
        assert!(native.contains("reference_reset_pointer(app,context.clone(),capture.clone(),completion)"));
        assert!(viewer_drag.contains("state.invoke_start_thumbnail_file_drag("));
    }

    #[test]
    fn macos_file_drag_exposes_the_local_image_to_finder() {
        let drag = include_str!("../drag_preview.rs");
        let platform = include_str!("../platform.rs");
        let authority = include_str!("native_drag.rs");
        let queued = platform.split_once("fn queue_macos_file_drag(").unwrap().1
            .split_once("fn take_macos_file_drag(").unwrap().0;
        let mouse = platform.split_once("fn mouse_dragged(").unwrap().1
            .split_once("fn mouse_up(").unwrap().0;
        let native = platform.split_once("fn start_native_file_drag(").unwrap().1
            .split_once("fn extract_image_paths(").unwrap().0;
        let prepare = authority.split_once("fn prepare_native_file_drag_source(").unwrap().1
            .split_once("fn require_drag_binding(").unwrap().0;
        let consume = authority.split_once("fn consume<R>(").unwrap().1
            .split_once("fn cancel_native_file_drag_for_retirement(").unwrap().0;

        assert!(drag.contains("crate::platform::queue_macos_file_drag(drag)"));
        assert!(queued.contains("CapturedNativeFileDrag"));
        assert!(queued.contains("PENDING_MACOS_FILE_DRAG.try_with"));
        assert!(platform.contains("sel!(mouseDragged:)"));
        assert!(platform.contains("ORIGINAL_MOUSE_DRAGGED"));
        assert!(platform.contains("sel!(mouseUp:)"));
        assert!(platform.contains("ORIGINAL_MOUSE_UP"));
        assert!(mouse.contains("take_macos_file_drag()"));
        assert!(mouse.contains("drag.consume(|path| start_native_file_drag(view, event, path.to_owned()))"));
        assert!(native.contains("NSString::from_str(&path.to_string_lossy())"));
        assert!(native.contains("dragFile_fromRect_slideBack_event"));
        // The original regular file is held/inspected before AppKit consumes it;
        // canonicalizing an arbitrary late pathname is no longer the authority.
        assert!(prepare.contains("authority.open_existing_regular(&key)"));
        assert!(prepare.contains("authority.inspect_regular(&file)"));
        assert!(consume.contains("self.source.authority.inspect_regular(&self.source.file)? == self.source.metadata"));
        assert!(consume.contains("Ok(native(&self.source.path))"));
    }

    #[test]
    fn thumbnail_file_drag_is_not_intercepted_by_slint_internal_drag_area() {
        let thumbnail = include_str!("../../ui/components/thumbnail-card.slint");

        assert!(!thumbnail.contains("DragArea {"));
        assert!(thumbnail.contains("Math.abs(hover.mouse-x - hover.pressed-x) < 7px"));
        assert!(thumbnail.contains("AppState.start-thumbnail-file-drag(drag-data)"));
        let native_drag = thumbnail
            .find("AppState.start-thumbnail-file-drag(drag-data)")
            .expect("native drag call");
        let cleanup = thumbnail[native_drag..]
            .find("root.hide-drag-preview();")
            .expect("post-drag cleanup");
        assert!(cleanup > 0);
    }

    #[test]
    fn generation_loading_thumbnail_exposes_a_stop_button() {
        let card = include_str!("../../ui/components/generation-loading-card.slint");
        let state = include_str!("../../ui/app-state.slint");
        let callbacks = include_str!("callbacks/generation.rs");

        assert!(card.contains("stop-button := Rectangle"));
        assert!(card.contains("stop-touch := TouchArea"));
        assert!(card.contains("card-hover := TouchArea"));
        assert!(card.contains("visible: card-hover.has-hover || stop-touch.has-hover;"));
        assert!(card.contains("AppState.stop-generation()"));
        assert!(card.contains("AppTheme.danger"));
        assert!(state.contains("callback stop-generation();"));
        assert!(callbacks.contains("state.on_stop_generation"));
    }

    #[test]
    fn generation_loading_thumbnail_has_a_breathing_border() {
        let card = include_str!("../../ui/components/generation-loading-card.slint");

        assert!(card.contains("property <bool> pulse-bright: false;"));
        assert!(card.contains("interval: AppState.reduced-motion ? 1400ms : 900ms;"));
        assert!(card.contains("breathing-border := Rectangle"));
        assert!(card.contains(
            "animate opacity { duration: AppState.reduced-motion ? 0ms : 900ms; easing: ease-in-out; }"
        ));
    }

    #[test]
    fn generation_loading_cards_bounce_left_to_right_every_two_seconds() {
        let card = include_str!("../../ui/components/generation-loading-card.slint");
        let gallery = include_str!("../../ui/components/virtualized-gallery.slint");

        assert!(card.contains("in property <int> sequence-index: 0;"));
        assert!(card.contains("in property <int> bounce-step: 0;"));
        assert!(card.contains("root.bounce-step - root.sequence-index * 4 + 40"));
        assert!(card.contains("phase == 5 ? 0px - 7px"));
        assert!(card.contains("phase == 11 ? 1px"));
        assert!(card.contains(
            "animate y { duration: AppState.reduced-motion ? 0ms : 65ms; easing: ease-in-out; }"
        ));
        assert!(gallery.contains("interval: AppState.reduced-motion ? 200ms : 50ms;"));
        assert!(gallery.contains("running: root.loaders.length > 0;"));
        assert!(gallery.contains("Math.mod(root.loading-bounce-step + 1, 40)"));
        assert!(gallery.contains("sequence-index: loader.sequence-index;"));
        assert!(gallery.contains("bounce-step: root.loading-bounce-step;"));
    }

    #[test]
    fn generation_loading_and_completed_items_share_the_virtualized_template() {
        let panel = include_str!("../../ui/components/generation-result-panel.slint");
        let gallery = include_str!("../../ui/components/virtualized-gallery.slint");

        assert!(!panel.contains("GenerationWaterfall"));
        assert!(panel.contains("result-gallery := VirtualizedGallery"));
        assert!(
            panel.contains("visible: AppState.generations.length > 0 || root.active-generating();")
        );
        assert!(panel.contains("loaders: AppState.generation-layout-loaders;"));
        assert!(panel.contains("AppState.update-gallery-viewport("));
        assert!(gallery.contains("for loader in root.loaders: GenerationLoadingCard"));
        assert!(gallery.contains("for placement in root.placements: ThumbnailCard"));
    }

    #[test]
    fn generation_results_scroll_to_the_gallerys_measured_height() {
        let panel = include_str!("../../ui/components/generation-result-panel.slint");
        let gallery = include_str!("../../ui/components/virtualized-gallery.slint");

        assert!(panel.contains("viewport-height: max(self.height, result-gallery.height);"));
        assert!(!panel.contains("result-gallery.preferred-height"));
        assert!(panel.contains("y: 0px;"));
        assert!(panel.contains("content-height: AppState.generation-layout-height;"));
        assert!(gallery.contains("height: max(1px, root.content-height * 1px);"));
        assert!(!panel.contains("AppState.generation-groups.length * 66px"));
    }

    #[test]
    fn asset_gallery_scrolls_to_its_measured_grid_or_waterfall_height() {
        let assets = include_str!("../../ui/components/asset-gallery.slint");

        assert!(
            assets.contains("viewport-height: max(self.height, asset-gallery-content.height);")
        );
        assert!(!assets.contains("asset-gallery-content.preferred-height"));
        assert!(assets.contains("asset-gallery-content := VirtualizedGallery"));
        assert!(assets.contains("content-height: root.content-height;"));
        assert!(assets.contains("placements: root.placements;"));
        assert!(!assets.contains("root.groups.length * 66px"));
        assert!(!assets.contains("root.row-count() * root.row-height()"));
    }

    #[test]
    fn reference_images_are_capped_at_eight_for_every_workspace_category() {
        let model = include_str!("model.rs");
        let configuration = include_str!("configuration.rs");
        let composer = include_str!("../../ui/components/prompt-composer.slint");

        assert_eq!(max_reference_images_for_category("character"), 8);
        assert_eq!(max_reference_images_for_category("scene"), 8);
        assert_eq!(max_reference_images_for_category("ui"), 8);
        assert_eq!(max_reference_images_for_category("effect"), 8);
        assert_eq!(max_reference_images_for_category("unsupported-category"), 8);
        assert!(model.contains("const MAX_REFERENCE_IMAGES: usize = 8;"));
        assert!(configuration.contains("最多上传 8 张参考图"));
        assert!(composer.contains("return 8;"));
    }

    #[test]
    fn removed_workspace_feature_does_not_reappear_in_active_sources() {
        let removed_slug = ["action", "sequence"].join("-");
        let removed_type = ["Action", "Sequence"].join("");
        let sources = [
            include_str!("app.rs"),
            include_str!("configuration.rs"),
            include_str!("model.rs"),
            include_str!("prompt.rs"),
            include_str!("generation/backend.rs"),
            include_str!("storage/local_store.rs"),
            include_str!("../../ui/components/prompt-composer.slint"),
            include_str!("../../ui/components/creation-mode-chip.slint"),
            include_str!("../../ui/components/category-workspace-menu.slint"),
        ];

        for source in sources {
            assert!(!source.contains(&removed_slug));
            assert!(!source.contains(&removed_type));
        }
    }

    #[test]
    fn reference_thumbnails_use_a_responsive_four_column_grid() {
        let composer = include_str!("../../ui/components/prompt-composer.slint");
        let reference_thumb = include_str!("../../ui/components/reference-thumb.slint");

        assert!(composer.contains("return 4;"));
        assert!(composer.contains("function reference-card-size() -> length"));
        assert!(composer.contains("width: root.reference-card-size();"));
        assert!(composer.contains("height: root.reference-card-size();"));
        assert!(composer.contains("(root.width - 48px - 30px) / 4"));
        assert!(composer.contains("function reference-grid-x() -> length"));
        assert!(composer.contains("return 24px;"));
        assert!(composer.contains("- root.reference-card-size()"));
        assert!(composer.contains("(root.reference-row-count() - 1) * root.reference-row-height()"));
        assert!(composer.contains("ordinal: index + 1;"));
        assert!(reference_thumb.contains("in property <int> ordinal;"));
        assert!(reference_thumb.contains("text: root.ordinal;"));
        assert!(reference_thumb.contains("background: AppTheme.accent;"));
        assert!(!composer.contains("root.height - 80px"));
    }

    #[test]
    fn thumbnail_galleries_switch_between_grid_and_responsive_masonry_layouts() {
        let state = include_str!("../../ui/app-state.slint");
        let toggle = include_str!("../../ui/components/gallery-layout-toggle.slint");
        let panel = include_str!("../../ui/components/generation-result-panel.slint");
        let assets = include_str!("../../ui/pages/assets-page.slint");
        let inspiration = include_str!("../../ui/pages/inspiration-page.slint");
        let app = include_str!("app.rs");
        let profile = include_str!("storage/local_store.rs");
        let thumbnail = include_str!("../../ui/components/thumbnail-card.slint");
        let waterfall_column = include_str!("../../ui/components/waterfall-column.slint");
        let virtualized = include_str!("../../ui/components/virtualized-gallery.slint");

        for property in [
            "generation-gallery-layout",
            "asset-gallery-layout",
            "inspiration-gallery-layout",
        ] {
            assert!(
                state.contains(property),
                "missing gallery layout state {property}"
            );
        }
        assert!(toggle.contains("root.mode = root.mode == \"grid\" ? \"waterfall\" : \"grid\";"));
        assert!(toggle.contains("AppState.save-gallery-layout(root.preference-key, root.mode);"));
        assert!(toggle.contains("source: root.mode == \"waterfall\""));
        assert!(toggle.contains("text: root.mode == \"waterfall\""));
        assert!(panel.contains("mode <=> AppState.generation-gallery-layout;"));
        assert!(panel.contains("preference-key: \"generation\";"));
        assert!(assets.contains("mode <=> AppState.asset-gallery-layout;"));
        assert!(assets.contains("preference-key: \"assets\";"));
        assert!(inspiration.contains("mode <=> AppState.inspiration-gallery-layout;"));
        assert!(inspiration.contains("preference-key: \"inspiration\";"));
        assert!(state.contains("callback save-gallery-layout(string, string);"));
        assert!(app.contains("state.on_save_gallery_layout"));
        assert!(app.contains("save_device_settings(&app);"));
        assert!(profile.contains("fn device_settings_data"));
        assert!(profile.contains("state.set_generation_gallery_layout"));
        assert!(thumbnail.contains("in property <bool> masonry: false;"));
        assert!(thumbnail.contains("root.item.height / root.item.width"));
        assert!(waterfall_column.contains("masonry: true;"));
        assert!(waterfall_column.contains("in root.column-length(): ThumbnailCard"));
        assert!(waterfall_column.contains("root.items[root.source-index(index)]"));
        assert!(!waterfall_column.contains("for item[index] in root.items"));
        assert!(virtualized.contains("for placement in root.placements: ThumbnailCard"));
        assert!(virtualized.contains("for loader in root.loaders: GenerationLoadingCard"));
        assert!(panel.contains(
            "AppState.generation-gallery-layout == \"waterfall\" ? root.base-thumb-width() : root.item-width()"
        ));
    }

    #[test]
    fn asset_gallery_content_starts_below_the_filter_controls() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().expect("create app window");
        let state = app.global::<AppState>();

        state.set_logged_in(true);
        state.set_page("assets".into());
        state.set_assets(slint::ModelRc::new(slint::VecModel::from(vec![AssetItem {
            id: "asset-1".into(),
            title: "Asset 1".into(),
            category: "scene".into(),
            kind: "game".into(),
            width: 1024,
            height: 1024,
            source_path: "asset-1.png".into(),
            ..Default::default()
        }])));
        state.set_asset_layout_items(slint::ModelRc::new(slint::VecModel::from(vec![
            GalleryPlacement {
                item_index: 0,
                x: 0.0,
                y: 40.0,
                width: 200.0,
                gap: 18.0,
                masonry: false,
            },
        ])));
        state.set_asset_layout_headers(slint::ModelRc::new(slint::VecModel::from(vec![
            GalleryHeader {
                title: "8月20日".into(),
                y: 0.0,
            },
        ])));
        state.set_asset_layout_height(284.0);
        app.window().set_size(slint::LogicalSize::new(1440.0, 900.0));
        app.show().expect("show app window");

        let cards = i_slint_backend_testing::ElementHandle::find_by_element_type_name(
            &app,
            "ThumbnailCard",
        )
        .collect::<Vec<_>>();
        assert_eq!(cards.len(), 1, "expected the single asset thumbnail");
        let card_y = cards[0].absolute_position().y;
        assert!(
            card_y < 320.0,
            "asset thumbnail should stay near the filters, but started at y={card_y}"
        );
    }

    #[test]
    fn waterfall_column_mapping_instantiates_every_item_exactly_once() {
        for item_count in [0_usize, 1, 2, 7, 24, 49] {
            for column_count in 1_usize..=8 {
                let mut ordinary = Vec::new();
                for column in 0..column_count {
                    let length = if item_count <= column {
                        0
                    } else {
                        (item_count - 1 - column) / column_count + 1
                    };
                    ordinary.extend((0..length).map(|row| row * column_count + column));
                }
                ordinary.sort_unstable();
                assert_eq!(ordinary, (0..item_count).collect::<Vec<_>>());

                for loading_count in 0_usize..=4 {
                    let mut generation = Vec::new();
                    for column in 0..column_count {
                        let first = (column + column_count - loading_count % column_count)
                            % column_count;
                        let length = if item_count <= first {
                            0
                        } else {
                            (item_count - 1 - first) / column_count + 1
                        };
                        for row in 0..length {
                            let index = first + row * column_count;
                            assert_eq!((index + loading_count) % column_count, column);
                            generation.push(index);
                        }
                    }
                    generation.sort_unstable();
                    assert_eq!(generation, (0..item_count).collect::<Vec<_>>());
                }
            }
        }
    }

    #[test]
    fn gallery_pagination_is_edge_triggered_during_continuous_scrolling() {
        let assets = include_str!("../../ui/components/asset-gallery.slint");
        let generations = include_str!("../../ui/components/generation-result-panel.slint");
        let inspiration = include_str!("../../ui/pages/inspiration-page.slint");

        for source in [assets, generations, inspiration] {
            assert!(source.contains("property <bool> load-more-armed: true;"));
            assert!(source.contains("property <length> last-load-bottom: -10000px;"));
            assert!(source.contains("pagination-key"));
            assert!(source.contains("changed pagination-key =>"));
            assert!(source.contains("function reset-pagination-latch()"));
            assert!(source.contains("visible-bottom >= root.last-load-bottom + 240px"));
            assert!(source.contains("root.load-more-armed = false;"));
            assert!(source.contains("root.last-load-bottom = visible-bottom;"));
            assert!(source.contains("changed viewport-height =>"));
        }
        assert!(assets.contains("in property <string> pagination-key: \"\";"));
        assert!(generations.contains("property <string> pagination-key: AppState.asset-type;"));
        assert!(inspiration.contains(
            "property <string> pagination-key: AppState.inspiration-category-filter;"
        ));
        let assets_page = include_str!("../../ui/pages/assets-page.slint");
        assert!(assets_page.contains("pagination-key: AppState.asset-category-filter;"));
    }

    #[test]
    fn gallery_layout_preferences_are_backward_compatible_and_normalized() {
        let legacy: LegacyUserProfileData =
            serde_json::from_str("{}").expect("deserialize legacy user profile");
        assert_eq!(legacy.ui_preferences.generation_gallery_layout, "grid");
        assert_eq!(legacy.ui_preferences.asset_gallery_layout, "grid");
        assert_eq!(legacy.ui_preferences.inspiration_gallery_layout, "grid");

        let saved = DeviceSettings {
            generation_gallery_layout: "waterfall".to_string(),
            asset_gallery_layout: "waterfall".to_string(),
            inspiration_gallery_layout: "waterfall".to_string(),
            ..DeviceSettings::default()
        };
        let serialized = serde_json::to_string(&saved).expect("serialize user profile");
        let restored: DeviceSettings = serde_json::from_str(&serialized).expect("restore user profile");
        assert_eq!(restored.generation_gallery_layout, "waterfall");
        assert_eq!(normalize_gallery_layout(" WATERFALL "), "waterfall");
        assert_eq!(normalize_gallery_layout("unsupported"), "grid");
    }

    #[test]
    fn close_behavior_preferences_are_backward_compatible_and_normalized() {
        let legacy: LegacyUserProfileData =
            serde_json::from_str("{}").expect("deserialize legacy user profile");
        assert_eq!(normalize_close_behavior(&legacy.close_behavior), "ask");
        assert_eq!(normalize_close_behavior(" EXIT "), "exit");
        assert_eq!(normalize_close_behavior("tray"), "tray");
        assert_eq!(normalize_close_behavior("unsupported"), "ask");

        let saved = DeviceSettings {
            close_behavior: "tray".to_string(),
            ..DeviceSettings::default()
        };
        let serialized = serde_json::to_string(&saved).expect("serialize user profile");
        let restored: DeviceSettings = serde_json::from_str(&serialized).expect("restore user profile");
        assert_eq!(restored.close_behavior, "tray");
    }

    #[test]
    fn close_behavior_ui_and_system_tray_are_wired() {
        let app = include_str!("../../ui/app.slint");
        let state = include_str!("../../ui/app-state.slint");
        let settings = include_str!("../../ui/pages/settings-page.slint");
        let dialog = include_str!("../../ui/dialogs/close-behavior-dialog.slint");
        let option = include_str!("../../ui/components/close-behavior-option.slint");
        let runtime = include_str!("app.rs");
        let profile = include_str!("storage/local_store.rs");

        assert!(state.contains("in-out property <string> close-behavior: \"ask\""));
        assert!(state.contains("in-out property <bool> close-choice-open: false"));
        assert!(state.contains("callback set-close-behavior(string)"));
        assert!(state.contains("callback confirm-close-behavior(string)"));
        assert!(dialog.contains("退出程序"));
        assert!(dialog.contains("最小化到系统托盘"));
        assert!(settings.contains("CloseBehaviorOption"));
        assert!(settings.contains("AppState.en ? \"When closing the window\" : \"关闭窗口时\""));
        assert!(option.contains("AppState.set-close-behavior(root.id)"));

        assert!(app.contains("export component AppTray inherits SystemTrayIcon"));
        assert!(app.contains("clicked => { root.restore(); }"));
        assert!(app.contains("title: root.open-text"));
        assert!(app.contains("title: root.quit-text"));
        assert!(runtime.contains("wire_close_behavior(&app, &tray)"));
        assert!(runtime.contains("on_close_requested"));
        assert!(runtime.contains("CloseRequestResponse::KeepWindowShown"));
        assert!(runtime.contains("state.set_close_choice_open(true)"));
        assert!(runtime.contains("app.window().hide()"));
        assert!(runtime.contains("app.window().show()"));
        assert!(runtime.contains("slint::quit_event_loop()"));
        assert!(profile.contains("state.set_close_behavior"));
        assert!(profile.contains("close_behavior: normalize_close_behavior"));
    }

    #[test]
    fn task7_startup_and_shutdown_leave_private_services_unloaded() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().unwrap();
        let directory = tempfile::tempdir().unwrap();
        let private = directory.path().join("private-existing");
        fs::create_dir(&private).unwrap();
        fs::write(private.join("user-profile.json"), b"preserve-private").unwrap();
        let state = app.global::<AppState>();
        state.set_input_dir(private.display().to_string().into());
        state.set_prompt_dir(private.display().to_string().into());
        state.set_output_dir(private.display().to_string().into());
        let context = AppContext::default();
        super::app::apply_startup_device_state(
            &app,
            DeviceSettings {
                theme_id: "dark".into(),
                language: "en".into(),
                ..Default::default()
            },
            None,
        );
        assert_eq!(state.get_theme_id(), "dark");
        assert_eq!(state.get_language(), "en");
        assert!(state.get_input_dir().is_empty());
        assert!(state.get_prompt_dir().is_empty());
        assert!(state.get_output_dir().is_empty());
        assert!(context.store.borrow().assets.is_empty());
        assert!(context.store.borrow().generations.is_empty());
        assert!(context.store.borrow().inspiration.is_empty());
        assert_eq!(
            fs::read(private.join("user-profile.json")).unwrap(),
            b"preserve-private"
        );
        assert_eq!(fs::read_dir(&private).unwrap().count(), 1);
        // The whole executable cannot run in a fixture: it owns the process UI,
        // tray, global repository and session. Keep its wiring assertion beside
        // the actual isolated startup-presentation behavior above.
        let run = include_str!("app.rs")
            .split("pub(super) fn apply_startup_device_state")
            .next()
            .unwrap();
        for private_service in [
            "init_portable_dirs(",
            "initialize_storage_index(",
            "initialize_preview_cache(",
            "cleanup_stale_reference_imports(",
            "cleanup_stale_toolbox_files(",
            "load_showcase_images(",
            "seed_inspiration(",
            "load_user_profile(",
            "load_local_store(",
            "rebuild_storage_references(",
            "push_startup_state(",
            "save_local_store_checked(",
            "save_user_profile_checked(",
            "cleanup_orphaned_durable_copies_",
        ] {
            assert!(
                !run.contains(private_service),
                "private service still starts without a namespace: {private_service}"
            );
        }
        assert!(run.contains("save_device_settings_checked(&app)?"));
        assert!(run.contains("flush_device()?"));
    }

    #[test]
    fn application_brand_and_release_artifacts_are_elunvi_canvas() {
        let app = include_str!("../../ui/app.slint");
        let sidebar = include_str!("../../ui/components/sidebar.slint");
        let welcome = include_str!("../../ui/pages/welcome-page.slint");
        let settings = include_str!("../../ui/pages/settings-page.slint");
        let windows_resources = include_str!("../../build.rs");
        let installer = include_str!("../../../installer/ElunviCanvas.iss");
        let windows_package = include_str!("../../../scripts/package-native-client.ps1");
        let macos_package = include_str!("../../../scripts/package-macos.sh");

        assert!(app.contains("title: \"Elunvi Canvas\";"));
        assert!(sidebar.contains("text: \"Elunvi Canvas\";"));
        assert!(welcome.contains("利用 Elunvi Canvas"));
        assert!(settings.contains("text: \"Elunvi Canvas\";"));
        assert!(windows_resources.contains("res.set(\"ProductName\", \"Elunvi Canvas\")"));
        assert!(windows_resources.contains("res.set(\"FileDescription\", \"Elunvi Canvas\")"));
        assert!(installer.contains("#define AppName \"Elunvi Canvas\""));
        assert!(installer.contains("#define AppFileStem \"ElunviCanvas\""));
        assert!(installer.contains("#define AppExeName \"ElunviCanvas.exe\""));
        assert!(installer.contains("AppName={#AppName}"));
        assert!(windows_package.contains("<string>Elunvi Canvas</string>"));
        assert!(macos_package.contains("<string>Elunvi Canvas</string>"));
        assert!(windows_package.contains("$AppName = \"ElunviCanvas\""));
        assert!(macos_package.contains("APP_NAME=\"ElunviCanvas\""));
    }

    #[test]
    fn loading_dots_use_staggered_bouncing_motion() {
        let dots = include_str!("../../ui/components/loading-dots.slint");

        assert!(dots.contains("dot-one := Rectangle"));
        assert!(dots.contains("dot-two := Rectangle"));
        assert!(dots.contains("dot-three := Rectangle"));
        assert!(dots.contains("interval: AppState.reduced-motion ? 360ms : 120ms"));
        assert!(dots.matches("animate y").count() >= 3);
    }

    #[test]
    fn studio_work_panel_is_wider_and_results_fill_the_remainder() {
        let page = include_str!("../../ui/pages/studio-split-page.slint");

        assert!(page.contains("width: 540px;"));
        assert!(page.contains("Rectangle { x: 540px;"));
        assert!(page.contains("x: 541px;"));
        assert!(page.contains("width: parent.width - 541px;"));
    }

    #[test]
    fn sidebar_toolbox_opens_a_seven_tool_page() {
        let sidebar = include_str!("../../ui/components/sidebar.slint");
        let glyph = include_str!("../../ui/components/nav-glyph.slint");
        let app = include_str!("../../ui/app.slint");
        let state = include_str!("../../ui/app-state.slint");
        let page = include_str!("../../ui/pages/toolbox-page.slint");

        let canvas = sidebar
            .find("page: \"free-canvas\"")
            .expect("free canvas nav");
        let toolbox = sidebar.find("page: \"toolbox\"").expect("toolbox nav");
        let assets = sidebar.find("page: \"assets\"").expect("assets nav");
        assert!(canvas < toolbox && toolbox < assets);
        assert!(glyph.contains("root.kind == \"toolbox\""));
        assert!(app.contains("AppState.page == \"toolbox\""));
        assert!(state.contains("toolbox-selected-tool"));
        for title in [
            "去水印",
            "图片清晰",
            "老照片上色",
            "图片裁剪",
            "图片转格式",
            "图片压缩",
            "去黑",
        ] {
            assert!(page.contains(title), "missing toolbox card: {title}");
        }
        assert_eq!(page.matches("tool-id: ").count(), 7);
        assert_eq!(page.matches("target-page: \"toolbox-").count(), 7);
        assert!(page.contains("target-page: \"toolbox-watermark\""));
        assert!(page.contains("target-page: \"toolbox-enhance\""));
        assert!(page.contains("target-page: \"toolbox-colorize\""));
        assert!(page.contains("target-page: \"toolbox-crop\""));
        assert!(page.contains("target-page: \"toolbox-convert\""));
        assert!(page.contains("target-page: \"toolbox-compress\""));
        assert!(page.contains("target-page: \"toolbox-remove-black\""));
        assert!(page.contains("../../assets/icons/toolbox-watermark.svg"));
        assert!(page.contains("../../assets/icons/toolbox-enhance.svg"));
        assert!(page.contains("../../assets/icons/toolbox-convert.svg"));
        assert!(page.contains("../../assets/icons/toolbox-compress.svg"));
        assert!(page.contains("AppState.toolbox-selected-tool = root.tool-id"));
        assert!(state.contains("toolbox-coming-soon-open"));
        assert!(page.contains("AppState.toolbox-coming-soon-open = true"));
        assert!(page.contains("AppState.en ? \"Coming soon\" : \"即将开放\""));
        assert!(page.contains("AppState.en ? \"Got it\" : \"知道了\""));
        assert!(!page.contains("\"选择工具\""));
        assert!(!page.contains("\"已选择\""));
        assert!(sidebar.contains("active: AppState.page == \"toolbox\""));
        for subpage in [
            "toolbox-watermark",
            "toolbox-enhance",
            "toolbox-colorize",
            "toolbox-crop",
            "toolbox-convert",
            "toolbox-compress",
        ] {
            assert!(
                sidebar.contains(&format!("AppState.page == \"{subpage}\"")),
                "toolbox navigation should remain active on {subpage}"
            );
        }
        let nav_item = include_str!("../../ui/components/nav-item.slint");
        assert!(nav_item.contains("in property <bool> active: AppState.page == root.page;"));
        assert!(nav_item.contains("background: root.active ? AppTheme.panel-soft"));
        assert!(nav_item.contains("border-width: root.active ? 1px : 0px;"));
        // Settings now lives in the avatar menu; its route and popup behavior
        // are exercised by tests/sidebar_account_menu.rs.
    }

    #[test]
    fn ai_creation_entry_opens_an_eight_choice_launcher_before_the_infinite_canvas() {
        let app = include_str!("../../ui/app.slint");
        let sidebar = include_str!("../../ui/components/sidebar.slint");
        let nav_glyph = include_str!("../../ui/components/nav-glyph.slint");
        let page = include_str!("../../ui/pages/free-canvas-page.slint");
        let runtime = include_str!("app.rs");
        let viewer = include_str!("callbacks/viewer.rs");
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));

        assert!(sidebar.contains("label: AppState.en ? \"AI Creation\" : \"AI创作\""));
        assert!(sidebar.contains("icon: \"ai-creation\""));
        assert!(nav_glyph.contains("root.kind == \"ai-creation\""));
        assert!(nav_glyph.contains("../../assets/icons/ai-creation.svg"));
        let ai_icon = std::fs::read_to_string(manifest.join("assets/icons/ai-creation.svg"))
            .expect("AI creation navigation icon");
        assert!(ai_icon.contains("viewBox=\"0 0 1024 1024\""));
        assert!(ai_icon.contains("M442.688 163.84l-288 720"));
        assert!(sidebar.contains("page: \"free-canvas\""));
        assert!(sidebar.contains(
            "active: AppState.page == \"free-canvas\" || AppState.page == \"canvas\""
        ));
        assert!(app.contains("import { FreeCanvasPage }"));
        assert!(app.contains("AppState.page == \"free-canvas\": FreeCanvasPage"));
        assert!(app.contains("AppState.page == \"canvas\": InfiniteCanvasPage"));
        assert_eq!(page.matches("card-id: \"").count(), 8);
        for (card_id, title) in [
            ("plant-growth", "植物生成器"),
            ("character-outfit", "角色换装"),
            ("monster-generator", "怪物生成器"),
            ("upgrade-evolution", "升级进化"),
            ("character-age", "角色年龄变化"),
            ("character-body", "角色体型修改器"),
            ("building-derivation", "建筑衍生器"),
            ("infinite-canvas", "无限画布"),
        ] {
            assert!(page.contains(&format!("card-id: \"{card_id}\"")));
            assert!(page.contains(title));
        }
        assert_eq!(page.matches("prompt-zh:").count(), 8);
        assert_eq!(page.matches("prompt-en:").count(), 8);
        assert!(!page.contains("AppState.navigate(\"generation\")"));
        assert!(page.contains("AppState.navigate(\"canvas\")"));
        assert!(page.contains("opens-canvas: true"));
        let back = core_toolbox_contract_block(runtime, "state.on_back(", "state.on_set_theme(");
        let back: String = back.split_whitespace().collect();
        assert!(back.contains("ifpage==\"canvas\"{navigate_to_with_store(&app,&store.borrow(),\"free-canvas\");return;"));
        let shortcut = core_toolbox_contract_block(viewer,
            "state.on_viewer_open_creation_workflow(", "state.on_request_delete_asset(");
        assert!(shortcut.contains("start_captured_viewer_reference(&app, context.clone(), CapturedViewerReferenceIntent::Creation"));
        let saved = core_toolbox_contract_block(viewer,
            "fn poll_captured_reference_store_ack(", "\nfn retry_captured_reference_save(");
        let character = saved.split_once("CapturedViewerReferenceIntent::Creation { workflow_id, title, template, hint, .. } =>")
            .expect("actual saved character-workflow branch").1;
        assert!(saved.find("finish_delivery_preparation(&cancel)").unwrap()
            < saved.find("if !matches!(receiver.try_recv(), Ok(Ok(())))").unwrap());
        assert!(character.contains("viewer_reference_source_after_target("));
        assert!(character.contains("context.apply_user_completion(persistence.lease()"));
        assert!(character.contains("state.set_page(\"canvas\".into())"));
        assert!(character.contains("start_canvas_preview_effects(&app, persistence.clone(), canvas)"));

        for image in [
            "plant-growth.png",
            "character-outfit.png",
            "monster-generator.png",
            "upgrade-evolution.png",
            "character-age.png",
            "character-body.png",
            "building-derivation.png",
        ] {
            assert!(manifest.join("assets/free-canvas").join(image).is_file());
        }
    }

    #[test]
    fn canvas_submission_ignores_workbench_controls_and_keeps_cutout_rules_last() {
        let controls = PromptControls {
            category: "ui".into(), creation: "ui-hud".into(), style: "dark".into(),
            view: "first-person".into(), weather: "rainy".into(), time: "night".into(), light: "neon".into(),
        };
        let quote = QuoteContext {
            title: "unrelated workbench reference".into(), prompt: "close-up portrait".into(),
            ratio: "1:1".into(), quality: "1K".into(), width: 1024, height: 1024,
        };
        let requested = compose_canvas_workflow_prompt("Create {count} outfit variations.", "wide capes", 8, true);
        let submitted = build_generation_prompt_for_destination(
            &requested, "unrelated negative", &controls, &quote, "ui", "16:9", "4K",
            PromptLanguage::English, &GenerationDestination::Canvas { source_node_id: "test-node".into() },
        );
        assert!(submitted.contains("Create 8 outfit variations."));
        assert!(submitted.contains("Arrange all 8 subjects in two rows"));
        assert!(submitted.contains("full body from the top of the head to the soles"));
        assert!(submitted.contains("70%"));
        assert!(submitted.contains("16:9"));
        assert!(submitted.contains("4K"));
        assert!(!submitted.contains("unrelated"));
        assert!(!submitted.contains("close-up portrait"));
        assert!(!submitted.contains("UI component atlas"));
        assert!(!submitted.contains("Generation controls:"));
        assert!(submitted.ends_with("Completeness and separation take priority over large subjects, filling the frame or a fixed single row."));
        let gallery = build_generation_prompt_for_destination(
            "game interface", "unrelated negative", &controls, &quote, "ui", "1:1", "2K",
            PromptLanguage::English, &GenerationDestination::Gallery,
        );
        assert!(gallery.contains("UI component atlas"));
        assert!(gallery.contains("unrelated negative"));
        assert!(gallery.contains("unrelated workbench reference"));
        assert!(!gallery.contains("final cutout-safe composition rules"));
    }

    #[test]
    fn upgrade_evolution_card_opens_a_reference_driven_canvas_with_strict_output_rules() {
        use i_slint_backend_testing::ElementHandle;
        use slint::platform::PointerEventButton;

        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().expect("create app window");
        let state = app.global::<AppState>();
        state.set_logged_in(true);
        state.set_contact_popup_open(false);
        state.set_page("free-canvas".into());

        let opened_workspaces = Rc::new(RefCell::new(Vec::<String>::new()));
        let observed_workspaces = opened_workspaces.clone();
        state.on_open_canvas_workspace(move |id| {
            observed_workspaces.borrow_mut().push(id.to_string());
        });
        let routes = Rc::new(RefCell::new(Vec::<String>::new()));
        let observed_routes = routes.clone();
        state.on_navigate(move |page| {
            observed_routes.borrow_mut().push(page.to_string());
        });

        app.window().set_size(slint::LogicalSize::new(1440.0, 900.0));
        app.show().expect("show app window");

        let page = ElementHandle::find_by_element_type_name(&app, "FreeCanvasPage")
            .next()
            .expect("AI creation launcher");
        let card = ElementHandle::find_by_accessible_label(&app, "升级进化")
            .next()
            .expect("upgrade evolution card");
        assert!(card.absolute_position().y >= page.absolute_position().y);
        assert!(
            card.absolute_position().y + card.size().height
                <= page.absolute_position().y + page.size().height + 1.0,
            "upgrade evolution card must be visible without leaving the launcher viewport"
        );

        card.mock_single_click(PointerEventButton::Left);

        assert_eq!(
            opened_workspaces.borrow().last().map(String::as_str),
            Some("upgrade-evolution")
        );
        assert_eq!(routes.borrow().last().map(String::as_str), Some("canvas"));
        assert_eq!(state.get_canvas_workflow_id(), "upgrade-evolution");
        assert_eq!(state.get_canvas_workflow_title(), "升级进化");
        assert_eq!(state.get_asset_type(), "scene");

        let template = state.get_canvas_workflow_template().to_string();
        assert!(template.contains("{count}个连续升级进化阶段"));
        assert!(template.contains("从低级、基础形态逐步进化到顶级、终极形态"));
        assert!(template.contains("保持同一主体"));
        let submitted = compose_canvas_workflow_prompt(&template, "参考图中的主体", 5, false);
        assert!(submitted.contains("不得把所有主体统一处理为从小到大"));
        assert!(submitted.contains("若主体是人类或类人角色"));
        assert!(submitted.contains("保持年龄、身高、体型、身体比例、面部和身份特征稳定"));
        assert!(submitted.contains("主要通过服装等级、武器、装备、护甲"));
        assert!(submitted.contains("若主体是怪物、机械生物或其他生物"));
        assert!(submitted.contains("允许随等级逐步改变体型、身体比例、轮廓和形态"));
        assert!(submitted.contains("若主体是武器、道具、载具、植物或建筑"));
        assert!(submitted.contains("白、绿、蓝、紫、橙、红"));
        assert!(submitted.contains("等级色只能作为局部品质标识"));
        assert!(submitted.contains("不得给整个主体统一染色"));
        assert!(submitted.contains("绿光、蓝光、紫光、金光、红光"));
        assert!(submitted.contains("不得跨越主体之间的纯色背景间距"));
        assert!(submitted.contains("除工作流明确要求且严格限制在单个主体后方的局部柔和光晕外"));
        assert!(submitted.contains("必须使用单一纯色背景"));
        assert!(submitted.contains("不得出现编号、序号、文字标签、标题、说明文字或水印"));
    }

    #[test]
    fn building_derivation_uses_the_uploaded_style_with_optional_function_description() {
        use i_slint_backend_testing::ElementHandle;
        use slint::platform::PointerEventButton;

        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().expect("create app window");
        let state = app.global::<AppState>();
        state.set_logged_in(true);
        state.set_contact_popup_open(false);
        state.set_page("free-canvas".into());
        app.window().set_size(slint::LogicalSize::new(1440.0, 900.0));
        app.show().expect("show app window");
        ElementHandle::find_by_accessible_label(&app, "建筑衍生器")
            .next()
            .expect("building derivation launcher card")
            .mock_single_click(PointerEventButton::Left);
        assert_eq!(state.get_canvas_workflow_id(), "building-derivation");
        assert_eq!(state.get_canvas_workflow_title(), "建筑衍生器");
        assert_eq!(state.get_asset_type(), "scene");
        let template = state.get_canvas_workflow_template().to_string();
        assert!(template.contains("自动识别其世界观"));
        assert!(template.contains("不同功能的新建筑"));
        assert!(template.contains("保持原图的2D或3D表现方式和观察角度"));
        assert!(template.contains("不是同一建筑逐级升级"));
        let composed = compose_canvas_workflow_prompt(&template, "铁匠铺、酒馆、仓库", 12, false);
        assert!(composed.contains("12座同风格"));
        assert!(composed.contains("用户描述：铁匠铺、酒馆、仓库"));
        assert!(composed.contains("分成上下两行"));
        assert!(composed.contains("必须统一缩小所有主体"));
        assert!(composed.contains("必须使用单一纯色背景"));
        assert!(!composed.contains("白、绿、蓝、紫、橙、红"));
        assert!(!composed.contains("{count}"));

        state.set_page("canvas".into());
        state.set_canvas_workflow_prompt("".into());
        state.set_image_model("test-image-model".into());
        state.on_compose_canvas_workflow_prompt(|template, prompt, count, english| {
            compose_canvas_workflow_prompt(template.as_str(), prompt.as_str(), count, english).into()
        });
        state.on_create_canvas_generation_source(|_, _, _| "building-source".into());
        let submitted = Rc::new(RefCell::new(Vec::<String>::new()));
        let observed = submitted.clone();
        state.on_generate_canvas_node(move |_, prompt| observed.borrow_mut().push(prompt.to_string()));
        let generate = ElementHandle::find_by_element_id(&app, "InfiniteCanvasPage::workflow-generate-button")
            .next()
            .expect("workflow generate button");
        generate.mock_single_click(PointerEventButton::Left);
        assert!(submitted.borrow().is_empty());
        assert_eq!(state.get_generation_status(), "请先上传建筑参考图");
        state.set_references(ModelRc::new(VecModel::from(vec![ReferenceItem {
            id: "building-reference".into(),
            image: slint::Image::default(),
            source_path: "building.png".into(),
        }])));
        generate.mock_single_click(PointerEventButton::Left);
        assert_eq!(submitted.borrow().len(), 1);
        assert!(submitted.borrow()[0].contains("5座同风格"));
        assert!(submitted.borrow()[0].contains("不按升级等级排列"));
        assert!(state.get_canvas_workflow_prompt().is_empty());

        state.set_canvas_workflow_prompt("铁匠铺、酒馆、仓库".into());
        generate.mock_single_click(PointerEventButton::Left);
        assert_eq!(submitted.borrow().len(), 2);
        assert!(submitted.borrow()[1].contains("用户描述：铁匠铺、酒馆、仓库"));

        state.set_language("en".into());
        state.set_page("free-canvas".into());
        ElementHandle::find_by_accessible_label(&app, "Building Derivation")
            .next().expect("English building derivation card")
            .mock_single_click(PointerEventButton::Left);
        let english = compose_canvas_workflow_prompt(
            state.get_canvas_workflow_template().as_str(), "smithy, tavern", 6, true,
        );
        assert!(english.contains("exactly 6 NEW buildings"));
        assert!(english.contains("by function, not by upgrade level"));
        assert!(english.contains("User description: smithy, tavern"));
        assert!(!english.contains("{count}"));
    }

    #[test]
    fn workflow_thumbnail_opens_a_quick_switcher_and_switches_workspaces() {
        use i_slint_backend_testing::ElementHandle;
        use slint::platform::PointerEventButton;

        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().expect("create app window");
        let state = app.global::<AppState>();
        state.set_logged_in(true);
        state.set_contact_popup_open(false);
        state.set_page("canvas".into());
        state.set_canvas_workflow_id("upgrade-evolution".into());
        state.set_canvas_workflow_title("升级进化".into());

        let opened_workspaces = Rc::new(RefCell::new(Vec::<String>::new()));
        let observed_workspaces = opened_workspaces.clone();
        state.on_open_canvas_workspace(move |id| {
            observed_workspaces.borrow_mut().push(id.to_string());
        });

        app.window().set_size(slint::LogicalSize::new(1440.0, 900.0));
        app.show().expect("show app window");

        assert!(
            ElementHandle::find_by_accessible_label(&app, "切换到植物生成器")
                .next()
                .is_none(),
            "quick switch cards should stay hidden until the workflow thumbnail is clicked"
        );
        let switcher = ElementHandle::find_by_accessible_label(&app, "切换创作模板")
            .next()
            .expect("workflow thumbnail switcher");
        switcher.mock_single_click(PointerEventButton::Left);

        let plant = ElementHandle::find_by_accessible_label(&app, "切换到植物生成器")
            .next()
            .expect("plant generator quick switch card");
        plant.invoke_accessible_default_action();

        assert_eq!(
            opened_workspaces.borrow().last().map(String::as_str),
            Some("plant-growth")
        );
        assert_eq!(state.get_canvas_workflow_id(), "plant-growth");
        assert_eq!(state.get_canvas_workflow_title(), "植物生成器");
        assert_eq!(state.get_asset_type(), "scene");
        assert!(
            state
                .get_canvas_workflow_template()
                .to_string()
                .contains("完整生命周期")
        );

        let switcher = ElementHandle::find_by_accessible_label(&app, "切换创作模板")
            .next()
            .expect("workflow thumbnail switcher after first switch");
        switcher.mock_single_click(PointerEventButton::Left);
        let outfit = ElementHandle::find_by_accessible_label(&app, "切换到角色换装")
            .next()
            .expect("character outfit quick switch card");
        outfit.invoke_accessible_default_action();

        assert_eq!(
            opened_workspaces.borrow().last().map(String::as_str),
            Some("character-outfit")
        );
        assert_eq!(state.get_canvas_workflow_id(), "character-outfit");
        assert_eq!(state.get_canvas_workflow_title(), "角色换装");
        assert_eq!(state.get_asset_type(), "character");
        assert!(
            state
                .get_canvas_workflow_template()
                .to_string()
                .contains("只改变服装、鞋履和配饰")
        );

        let switcher = ElementHandle::find_by_accessible_label(&app, "切换创作模板")
            .next()
            .expect("workflow thumbnail switcher after second switch");
        switcher.mock_single_click(PointerEventButton::Left);
        let evolution = ElementHandle::find_by_accessible_label(&app, "切换到升级进化")
            .next()
            .expect("upgrade evolution quick switch card");
        evolution.invoke_accessible_default_action();

        assert_eq!(
            opened_workspaces.borrow().last().map(String::as_str),
            Some("upgrade-evolution")
        );
        assert_eq!(state.get_canvas_workflow_id(), "upgrade-evolution");
        assert_eq!(state.get_canvas_workflow_title(), "升级进化");
        assert_eq!(state.get_asset_type(), "scene");
        let template = state.get_canvas_workflow_template().to_string();
        assert!(template.contains("先自动识别参考图中的主体类型"));
        assert!(template.contains("若主体是人类或类人角色"));
        assert!(template.contains("若主体是怪物、机械生物或其他生物"));
        assert!(template.contains("若主体是武器、道具、载具、植物或建筑"));
        let submitted = compose_canvas_workflow_prompt(&template, "", 8, false);
        assert!(submitted.contains("白、绿、蓝、紫、橙、红"));
        assert!(submitted.contains("等级色只能作为局部品质标识"));
        assert!(submitted.contains("不得给整个主体统一染色"));
        assert!(submitted.contains("绿光、蓝光、紫光、金光、红光"));
        assert!(submitted.contains("不得跨越主体之间的纯色背景间距"));
        assert!(template.contains("使用单一纯色背景"));
        assert!(template.contains("不得出现任何文字、字母、数字"));

        ElementHandle::find_by_accessible_label(&app, "切换创作模板")
            .next().expect("workflow switcher")
            .mock_single_click(PointerEventButton::Left);
        ElementHandle::find_by_accessible_label(&app, "切换到建筑衍生器")
            .next().expect("building derivation quick switch card")
            .invoke_accessible_default_action();
        assert_eq!(opened_workspaces.borrow().last().map(String::as_str), Some("building-derivation"));
        assert_eq!(state.get_canvas_workflow_id(), "building-derivation");
        assert!(state.get_canvas_workflow_template().contains("建筑功能衍生："));
        assert_eq!(state.get_asset_type(), "scene");
    }

    #[test]
    fn upgrade_evolution_requires_a_reference_but_not_an_extra_description() {
        use i_slint_backend_testing::ElementHandle;
        use slint::platform::PointerEventButton;

        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().expect("create app window");
        let state = app.global::<AppState>();
        state.set_logged_in(true);
        state.set_contact_popup_open(false);
        state.set_page("canvas".into());
        state.set_canvas_workflow_id("upgrade-evolution".into());
        state.set_canvas_workflow_title("升级进化".into());
        state.set_canvas_workflow_template("生成{count}个连续升级进化阶段。".into());
        state.set_canvas_workflow_prompt("".into());
        state.set_image_model("test-image-model".into());
        state.on_compose_canvas_workflow_prompt(|template, prompt, step_count, english| {
            compose_canvas_workflow_prompt(
                template.as_str(),
                prompt.as_str(),
                step_count,
                english,
            )
            .into()
        });

        let source_prompts = Rc::new(RefCell::new(Vec::<String>::new()));
        let observed_source_prompts = source_prompts.clone();
        state.on_create_canvas_generation_source(move |prompt, _, _| {
            observed_source_prompts
                .borrow_mut()
                .push(prompt.to_string());
            "upgrade-source".into()
        });
        let submitted_prompts = Rc::new(RefCell::new(Vec::<String>::new()));
        let observed_submitted_prompts = submitted_prompts.clone();
        state.on_generate_canvas_node(move |_, prompt| {
            observed_submitted_prompts
                .borrow_mut()
                .push(prompt.to_string());
        });

        app.window().set_size(slint::LogicalSize::new(1440.0, 900.0));
        app.show().expect("show app window");
        let generate = ElementHandle::find_by_element_id(
            &app,
            "InfiniteCanvasPage::workflow-generate-button",
        )
        .next()
        .expect("workflow generate button");

        generate.mock_single_click(PointerEventButton::Left);
        assert!(source_prompts.borrow().is_empty());
        assert_eq!(state.get_generation_status(), "请先上传主体参考图");

        state.set_references(ModelRc::new(VecModel::from(vec![ReferenceItem {
            id: "subject-reference".into(),
            image: slint::Image::default(),
            source_path: "subject.png".into(),
        }])));
        generate.mock_single_click(PointerEventButton::Left);

        assert_eq!(source_prompts.borrow().as_slice(), ["升级进化"]);
        assert_eq!(submitted_prompts.borrow().len(), 1);
        assert!(submitted_prompts.borrow()[0].contains("生成5个连续升级进化阶段"));
        assert!(submitted_prompts.borrow()[0].contains("必须使用单一纯色背景"));
    }

    #[test]
    fn all_creation_presets_require_images_and_accept_optional_prompts() {
        use i_slint_backend_testing::ElementHandle;
        use slint::platform::PointerEventButton;
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().unwrap();
        let state = app.global::<AppState>();
        state.set_logged_in(true);
        state.set_contact_popup_open(false);
        state.set_image_model("test-image-model".into());
        state.on_compose_canvas_workflow_prompt(|template, prompt, count, english| {
            compose_canvas_workflow_prompt(template.as_str(), prompt.as_str(), count, english).into()
        });
        state.on_create_canvas_generation_source(|_, _, _| "preset-source".into());
        let submitted = Rc::new(RefCell::new(Vec::<String>::new()));
        let observed = submitted.clone();
        state.on_generate_canvas_node(move |_, prompt| observed.borrow_mut().push(prompt.to_string()));
        app.window().set_size(slint::LogicalSize::new(1440.0, 900.0));
        app.show().unwrap();

        for (label, id, expected) in [
            ("植物生成器", "plant-growth", "完整生命周期"),
            ("角色换装", "character-outfit", "只改变服装"),
            ("怪物生成器", "monster-generator", "怪物设计"),
            ("升级进化", "upgrade-evolution", "连续升级进化阶段"),
            ("角色年龄变化", "character-age", "婴儿到老年"),
            ("角色体型修改器", "character-body", "体型"),
            ("建筑衍生器", "building-derivation", "不同功能的新建筑"),
        ] {
            submitted.borrow_mut().clear();
            state.set_page("free-canvas".into());
            ElementHandle::find_by_accessible_label(&app, label).next().expect(label)
                .mock_single_click(PointerEventButton::Left);
            assert_eq!(state.get_canvas_workflow_id(), id);
            state.set_page("canvas".into());
            state.set_references(ModelRc::new(VecModel::from(Vec::<ReferenceItem>::new())));
            state.set_canvas_workflow_prompt("".into());
            let generate = ElementHandle::find_by_element_id(&app, "InfiniteCanvasPage::workflow-generate-button")
                .next().unwrap();
            generate.mock_single_click(PointerEventButton::Left);
            assert!(submitted.borrow().is_empty(), "{label} requires an image");
            state.set_canvas_workflow_prompt("水彩风格".into());
            generate.mock_single_click(PointerEventButton::Left);
            assert!(submitted.borrow().is_empty(), "text alone must not bypass {label}'s reference requirement");
            state.set_references(ModelRc::new(VecModel::from(vec![ReferenceItem {
                id: "reference".into(), image: Image::default(), source_path: "reference.png".into(),
            }])));
            state.set_canvas_workflow_prompt("".into());
            generate.mock_single_click(PointerEventButton::Left);
            assert_eq!(submitted.borrow().len(), 1, "{label} must accept reference-only generation");
            assert!(submitted.borrow()[0].contains(expected), "{label} must use its built-in template");
            assert!(!submitted.borrow()[0].contains("{count}"));
            state.set_canvas_workflow_prompt("水彩风格".into());
            generate.mock_single_click(PointerEventButton::Left);
            assert_eq!(submitted.borrow().len(), 2);
            assert!(submitted.borrow()[1].contains("用户描述：水彩风格"));
        }
        // The infinite canvas still needs a prompt even when an image is attached.
        submitted.borrow_mut().clear();
        state.set_canvas_workflow_id("".into());
        state.set_canvas_workflow_template("".into());
        state.set_canvas_workflow_prompt("".into());
        ElementHandle::find_by_element_id(&app, "InfiniteCanvasPage::workflow-generate-button")
            .next().unwrap().mock_single_click(PointerEventButton::Left);
        assert!(submitted.borrow().is_empty());
    }

    #[test]
    fn canvas_failure_remains_visible_and_retry_requires_a_click() {
        use i_slint_backend_testing::ElementHandle;
        use slint::platform::PointerEventButton;
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().unwrap();
        let state = app.global::<AppState>();
        state.on_is_generation_error(|message| is_generation_error_message(message.as_str()));
        state.set_logged_in(true);
        state.set_contact_popup_open(false);
        state.set_page("canvas".into());
        state.set_image_model("test-model".into());
        state.set_canvas_workflow_id("character-age".into());
        state.set_canvas_workflow_prompt("保留蓝色服饰".into());
        state.set_references(ModelRc::new(VecModel::from(vec![ReferenceItem {
            id: "reference".into(), image: Image::default(), source_path: "reference.png".into(),
        }])));
        state.on_compose_canvas_workflow_prompt(|_, prompt, _, _| prompt);
        state.on_create_canvas_generation_source(|_, _, _| "retry-source".into());
        let submissions = Rc::new(RefCell::new(Vec::<String>::new()));
        let observed = submissions.clone();
        state.on_generate_canvas_node(move |_, prompt| observed.borrow_mut().push(prompt.to_string()));
        app.window().set_size(slint::LogicalSize::new(1440.0, 900.0));
        app.show().unwrap();
        slint::platform::update_timers_and_animations();
        state.set_generating(false);
        state.set_generation_status("生成失败：服务暂时不可用".into());
        slint::platform::update_timers_and_animations();
        assert!(ElementHandle::find_by_accessible_label(&app, "生成失败：服务暂时不可用").next().is_some(),
            "Canvas must expose the failure after loading disappears");
        assert!(submissions.borrow().is_empty(), "Never automatically resubmit a paid task");
        assert_eq!(state.get_canvas_workflow_prompt(), "保留蓝色服饰");
        assert_eq!(state.get_references().row_count(), 1);
        ElementHandle::find_by_accessible_label(&app, "重试生成").next().unwrap()
            .mock_single_click(PointerEventButton::Left);
        assert_eq!(submissions.borrow().as_slice(), ["保留蓝色服饰"]);
        assert_eq!(state.get_references().row_count(), 1);
        state.set_generating(true);
        if let Some(retry) = ElementHandle::find_by_accessible_label(&app, "重试生成").next() {
            retry.mock_single_click(PointerEventButton::Left);
        }
        assert_eq!(submissions.borrow().len(), 1, "Retry cannot stop or duplicate an active task");
        state.set_generating(false);
        state.set_generation_status("服务响应异常，请稍后重试".into());
        slint::platform::update_timers_and_animations();
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(5100));
        slint::platform::update_timers_and_animations();
        assert!(ElementHandle::find_by_accessible_label(&app, "服务响应异常，请稍后重试").next().is_none(),
            "Finished-task feedback must disappear after five seconds");
        assert_eq!(state.get_generation_status(), "服务响应异常，请稍后重试");
        assert_eq!(state.get_canvas_workflow_prompt(), "保留蓝色服饰");
        assert_eq!(state.get_references().row_count(), 1);
        state.set_generation_status("另一条失败提示".into());
        slint::platform::update_timers_and_animations();
        assert!(ElementHandle::find_by_accessible_label(&app, "另一条失败提示").next().is_some());
        ElementHandle::find_by_accessible_label(&app, "关闭提示").next().unwrap()
            .mock_single_click(PointerEventButton::Left);
        assert!(ElementHandle::find_by_accessible_label(&app, "另一条失败提示").next().is_none());
        assert_eq!(submissions.borrow().len(), 1);
        state.set_page("free-canvas".into());
        slint::platform::update_timers_and_animations();
        assert!(ElementHandle::find_by_accessible_label(&app, "另一条失败提示").next().is_none());
        state.set_page("canvas".into());
        slint::platform::update_timers_and_animations();
        assert!(ElementHandle::find_by_accessible_label(&app, "另一条失败提示").next().is_none(),
            "Reopening the canvas must not resurrect dismissed feedback");
        state.set_generating(true);
        slint::platform::update_timers_and_animations();
        state.set_generating(false);
        slint::platform::update_timers_and_animations();
        assert!(ElementHandle::find_by_accessible_label(&app, "另一条失败提示").next().is_some(),
            "A new task may report the same error again");
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(5100));
        slint::platform::update_timers_and_animations();
        state.set_page("free-canvas".into());
        slint::platform::update_timers_and_animations();
        assert!(ElementHandle::find_by_accessible_label(&app, "另一条失败提示").next().is_none());
        state.set_page("canvas".into());
        slint::platform::update_timers_and_animations();
        assert!(ElementHandle::find_by_accessible_label(&app, "另一条失败提示").next().is_none(),
            "Auto-dismissal must also survive reopening the canvas");
        state.set_generation_status("尚未关闭的新错误".into());
        slint::platform::update_timers_and_animations();
        assert!(ElementHandle::find_by_accessible_label(&app, "尚未关闭的新错误").next().is_some());
        state.set_page("free-canvas".into());
        slint::platform::update_timers_and_animations();
        assert!(ElementHandle::find_by_accessible_label(&app, "尚未关闭的新错误").next().is_none());
        state.set_page("canvas".into());
        slint::platform::update_timers_and_animations();
        assert!(ElementHandle::find_by_accessible_label(&app, "尚未关闭的新错误").next().is_none(),
            "Navigation must not replay even an undismissed old notification");
        for status in ["已添加参考图", "任务已提交，正在排队...", "正在生成...", "生成成功", "已停止生成"] {
            state.set_generation_status(status.into());
            slint::platform::update_timers_and_animations();
            assert!(ElementHandle::find_by_accessible_label(&app, status).next().is_none(),
                "Normal status must not display the error panel: {status}");
            assert!(ElementHandle::find_by_accessible_label(&app, "重试生成").next().is_none());
        }
    }

    #[test]
    fn free_canvas_presets_open_the_shared_canvas_composer() {
        let state = include_str!("../../ui/app-state.slint");
        let launcher = include_str!("../../ui/pages/free-canvas-page.slint");
        let canvas = include_str!("../../ui/pages/infinite-canvas-page.slint");
        let callbacks = include_str!("callbacks/infinite_canvas.rs");
        let reference_callbacks = include_str!("callbacks/reference.rs");
        let viewer = include_str!("callbacks/viewer.rs");

        for property in [
            "canvas-workflow-id",
            "canvas-workflow-title",
            "canvas-workflow-prompt",
            "canvas-workflow-template",
            "canvas-workflow-hint",
            "canvas-workflow-artwork",
            "canvas-workspace-switch-request",
        ] {
            assert!(state.contains(property));
        }
        assert!(state.contains("callback open-canvas-workspace(string)"));
        assert!(launcher.contains("AppState.open-canvas-workspace(root.card-id)"));
        assert!(launcher.contains("AppState.canvas-workflow-id = root.card-id"));
        assert!(launcher.contains("AppState.canvas-workflow-title = root.title"));
        assert!(launcher.contains(
            "AppState.canvas-workflow-template = AppState.en ? root.prompt-en : root.prompt-zh"
        ));
        assert!(launcher.contains(
            "AppState.canvas-workflow-hint = AppState.en ? root.hint-en : root.hint-zh"
        ));
        assert!(launcher.contains("AppState.canvas-workflow-artwork = root.artwork"));
        assert!(!launcher.contains("AppState.canvas-workflow-prompt = \"\""));
        assert!(launcher.contains("AppState.canvas-grid-style = \"dot\""));
        assert!(launcher.contains("AppState.canvas-dark-background = true"));
        assert!(!launcher.contains("AppState.navigate(\"generation\")"));
        assert!(launcher.contains("AppState.navigate(\"canvas\")"));

        assert!(canvas.contains("canvas-workflow-dock := Rectangle"));
        assert!(canvas.contains("workflow-dock-surface := Rectangle"));
        assert!(!canvas.contains("canvas-workflow-mode-tabs := Rectangle"));
        assert!(canvas.contains("workflow-collapse-handle := Rectangle"));
        assert!(canvas.contains("changed workspace-switch-request"));
        assert!(canvas.contains("workflow-template-action := Rectangle"));
        assert!(canvas.contains("workflow-bottom-controls := Rectangle"));
        assert!(!canvas.contains("AppState.en ? \"Video generation\" : \"视频生成\""));
        assert!(!canvas.contains("AppState.en ? \"3D generation\" : \"3D生成\""));
        assert!(!canvas.contains("text: \"@\""));
        assert!(canvas.contains("source: AppState.canvas-workflow-artwork"));
        assert!(canvas.contains("workflow-model-control := Rectangle"));
        assert!(canvas.contains("workflow-model-popup := PopupWindow"));
        assert!(canvas.contains("workflow-settings-popup := PopupWindow"));
        assert!(canvas.contains("AppState.select-image-model(model.code)"));
        assert!(canvas.contains("workflow-model-popup.show()"));
        assert!(canvas.contains("workflow-settings-popup.show()"));
        assert!(canvas.contains("AppState.ratio + \" · \" + AppState.quality"));
        assert!(canvas.contains("AppState.quality = \"4K\""));
        assert!(canvas.contains("AppState.ratio = \"16:9\""));
        assert!(canvas.contains("text <=> AppState.canvas-workflow-prompt"));
        assert!(canvas.contains("for reference[index] in AppState.references"));
        assert!(canvas.contains("AppState.add-reference()"));
        assert!(canvas.contains("AppState.open-reference(reference.id)"));
        assert!(canvas.contains("AppState.remove-reference(reference.id)"));
        assert!(canvas.contains("AppState.clear-references()"));
        assert!(canvas.contains("AppState.create-canvas-generation-source"));
        assert!(canvas.contains("AppState.compose-canvas-workflow-prompt("));
        assert!(canvas.contains("AppState.canvas-workflow-step-count"));
        assert!(canvas.contains(
            "AppState.generate-canvas-node(AppState.canvas-generation-loading-node-id, submitted-prompt)"
        ));

        assert!(state.contains(
            "callback create-canvas-generation-source(string, float, float) -> string"
        ));
        assert!(state.contains(
            "callback compose-canvas-workflow-prompt(string, string, int, bool) -> string"
        ));
        assert!(callbacks.contains("state.on_create_canvas_generation_source"));
        assert!(callbacks.contains("state.on_compose_canvas_workflow_prompt"));
        assert!(callbacks.contains("state.on_open_canvas_workspace"));
        assert!(callbacks.contains("switch_canvas_workspace"));
        assert!(state.contains("callback clear-references()"));
        assert!(reference_callbacks.contains("state.on_clear_references"));
        assert!(viewer.contains("state.set_canvas_workflow_id(\"\".into())"));
        assert!(viewer.contains("DEFAULT_CANVAS_WORKSPACE_ID"));
    }

    #[test]
    fn canvas_reference_addition_asks_for_a_source_before_opening_a_picker() {
        use i_slint_backend_testing::{ElementHandle, TestingBackend, TestingBackendOptions};
        use slint::platform::PointerEventButton;

        slint::platform::set_platform(Box::new(TestingBackend::new(TestingBackendOptions {
            mock_time: true,
            renderer_name: Some("software".into()),
            ..Default::default()
        })))
        .unwrap();
        let app = AppWindow::new().expect("create app window");
        let state = app.global::<AppState>();
        state.set_logged_in(true);
        state.set_contact_popup_open(false);
        state.set_page("canvas".into());
        state.set_canvas_workflow_id("upgrade-evolution".into());
        state.set_assets(ModelRc::new(VecModel::from(vec![AssetItem {
            id: "asset-1".into(),
            title: "测试资产".into(),
            source_path: "asset-1.png".into(),
            ..Default::default()
        }])));
        state.on_refresh_assets(|| {});
        let local_picker_calls = Rc::new(Cell::new(0));
        let local_picker_calls_for_callback = local_picker_calls.clone();
        state.on_add_reference(move || {
            local_picker_calls_for_callback.set(local_picker_calls_for_callback.get() + 1);
        });
        app.window().set_size(slint::LogicalSize::new(1280.0, 820.0));
        app.show().expect("show app window");

        let add = ElementHandle::find_by_element_id(
            &app,
            "InfiniteCanvasPage::upload-reference-touch",
        )
        .next()
        .expect("reference add tile");
        add.mock_single_click(PointerEventButton::Left);

        assert_eq!(local_picker_calls.get(), 0, "opening the tile must not choose a source yet");
        let local = ElementHandle::find_by_accessible_label(&app, "本地上传")
            .next()
            .expect("local upload source option");
        assert!(ElementHandle::find_by_accessible_label(&app, "从我的资产选择")
            .next()
            .is_some());
        local.invoke_accessible_default_action();
        assert_eq!(local_picker_calls.get(), 1);

        add.mock_single_click(PointerEventButton::Left);
        ElementHandle::find_by_accessible_label(&app, "从我的资产选择")
            .next()
            .expect("my assets source option")
            .invoke_accessible_default_action();
        assert!(ElementHandle::find_by_accessible_label(&app, "选择我的资产")
            .next()
            .is_some());
        let selected_asset_id = Rc::new(RefCell::new(String::new()));
        let selected_asset_id_for_callback = selected_asset_id.clone();
        state.on_add_reference_from_asset(move |id| {
            *selected_asset_id_for_callback.borrow_mut() = id.to_string();
            true
        });
        ElementHandle::find_by_accessible_label(&app, "选择资产 测试资产")
            .next()
            .expect("asset choice")
            .invoke_accessible_default_action();
        assert_eq!(selected_asset_id.borrow().as_str(), "asset-1");
    }

    #[test]
    fn stopping_a_canvas_generation_preserves_its_prompt_and_reference_images() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().expect("create app window");
        let state = app.global::<AppState>();
        state.set_logged_in(true);
        state.set_page("canvas".into());
        state.set_asset_type("scene".into());
        state.set_canvas_workflow_prompt("石头巨兽，逐步增加水晶装甲".into());
        state.set_references(ModelRc::new(VecModel::from(vec![ReferenceItem {
            id: "canvas-subject".into(),
            image: slint::Image::default(),
            source_path: "".into(),
        }])));

        let context = AppContext::default();
        context.store.borrow_mut().canvas_references.push(ReferenceData {
            id: "canvas-subject".to_string(),
            source_path: String::new(),
        });
        insert_active_generation(
            &context,
            ActiveGeneration {
                task_id: "canvas-task".to_string(),
                category: "scene".to_string(),
                prompt: "submitted workflow prompt".to_string(),
                destination: GenerationDestination::Canvas {
                    source_node_id: "loading-node".to_string(),
                },
                ..ActiveGeneration::default()
            },
        );

        stop_generation(&app, &context);

        assert_eq!(
            state.get_canvas_workflow_prompt(),
            "石头巨兽，逐步增加水晶装甲"
        );
        assert_eq!(state.get_references().row_count(), 1);
        assert_eq!(
            state.get_references().row_data(0).expect("canvas reference").id,
            "canvas-subject"
        );
    }

    #[test]
    fn reference_picker_does_not_block_the_slint_event_loop() {
        let callbacks = include_str!("callbacks/reference.rs");
        let add_reference = callbacks
            .split("state.on_add_reference(")
            .nth(1)
            .and_then(|block| block.split("state.on_paste_reference(").next())
            .expect("add-reference callback implementation");

        let picker = callbacks.split_once("fn reference_pick_files(").unwrap().1
            .split_once("fn reference_clipboard_image(").unwrap().0;
        // Scheduling shape only: the callback passes a weak-window completion;
        // actual rfd/OS modal behavior is not established by this source test.
        assert!(add_reference.contains("let weak=app.as_weak()"));
        assert!(add_reference.contains("reference_pick_files(Box::new(move|paths|"));
        assert!(add_reference.contains("weak.upgrade()"));
        assert!(picker.contains("slint::spawn_local(async move"));
        assert!(picker.contains("rfd::AsyncFileDialog::new()"));
        assert!(picker.contains(".pick_files().await"));
        assert!(!picker.contains("rfd::FileDialog::new()"));
        assert!(!add_reference.contains("rfd::FileDialog::new()"));
    }

    #[test]
    fn canvas_workflow_matches_inline_references_and_loading_result_behavior() {
        let state = include_str!("../../ui/app-state.slint");
        let launcher = include_str!("../../ui/pages/free-canvas-page.slint");
        let canvas = include_str!("../../ui/pages/infinite-canvas-page.slint");
        let controller = include_str!("generation/controller.rs");

        assert!(canvas.contains("workflow-reference-mentions := Rectangle"));
        assert!(canvas.contains("text: (AppState.en ? \"Image\" : \"图片\") + (index + 1)"));
        assert!(canvas.contains("x: workflow-prompt-frame.reference-mention-width"));
        assert!(canvas.contains("workflow-prompt-input.set-selection-offsets(2147483647, 2147483647)"));
        assert!(canvas.contains("x: 14px;\n                    y: 117px"));
        assert!(canvas.contains("x: 62px + index * 48px"));

        assert!(state.contains("canvas-generation-loading-node-id"));
        assert!(canvas.contains("function generation-loading() -> bool"));
        assert!(canvas.contains("workflow-generation-loading := Rectangle"));
        assert!(controller.contains("replace_canvas_generation_placeholder"));
        assert!(controller.contains("state.set_canvas_generation_loading_node_id(\"\".into())"));

        assert!(canvas.contains(
            "transform-rotation: root.workflow-composer-collapsed ? 0deg : 180deg;"
        ));
        assert!(launcher.contains("默认直接生长在自然土壤中"));
        assert!(launcher.contains("Only use a pot or container when the user explicitly requests one"));
    }

    #[test]
    fn workflow_prompt_discards_hidden_line_breaks_before_rendering() {
        assert_eq!(normalize_canvas_workflow_prompt("番茄 "), "番茄 ");
        assert_eq!(normalize_canvas_workflow_prompt("\n番茄"), "番茄");
        assert_eq!(
            normalize_canvas_workflow_prompt("番茄\r\n五个生长阶段"),
            "番茄 五个生长阶段"
        );
        assert_eq!(
            normalize_canvas_workflow_prompt("  番茄\u{2028}\u{2029}自然土壤  "),
            "番茄 自然土壤"
        );
    }

    #[test]
    fn canvas_workflow_step_count_replaces_every_template_placeholder() {
        let prompt = compose_canvas_workflow_prompt(
            "Create exactly {count} stages with {count} separate subjects.", "tomato", 8, true);
        assert!(prompt.starts_with("Create exactly 8 stages with 8 separate subjects."));
        assert!(!prompt.contains("{count}"));
        assert!(prompt.ends_with("User description: tomato"));
    }

    #[test]
    fn canvas_workflow_step_count_is_clamped_between_four_and_twelve() {
        let minimum = compose_canvas_workflow_prompt("制作{count}个步骤。", "番茄", 1, false);
        assert!(minimum.starts_with("制作4个步骤。"));
        assert!(minimum.contains("总共恰好4个完整主体"));
        let maximum = compose_canvas_workflow_prompt("制作{count}个步骤。", "番茄", 99, false);
        assert!(maximum.starts_with("制作12个步骤。"));
        assert!(maximum.contains("上排恰好6个，下排恰好6个"));
        assert!(maximum.contains("总共恰好12个完整主体"));
    }

    #[test]
    fn canvas_workflow_prompt_requires_a_solid_background_and_no_labels() {
        let prompt = compose_canvas_workflow_prompt("制作{count}个步骤。", "番茄", 5, false);
        assert!(prompt.contains("必须使用单一纯色背景"));
        assert!(prompt.contains("不得出现编号、序号、文字标签、标题、说明文字或水印"));
        assert!(prompt.contains("将全部5个对象按演变顺序排列在同一行"));
        assert!(prompt.ends_with("用户描述：番茄"));
    }

    #[test]
    fn canvas_workflow_prompt_forces_cutout_safe_gaps_between_every_subject() {
        let chinese =
            compose_canvas_workflow_prompt("制作{count}个升级阶段。", "仙侠角色", 6, false);
        assert!(chinese.contains("任意两个主体之间必须保留清晰、连续的纯色背景间距"));
        assert!(chinese.contains("主体的轮廓、服装、武器、装备、特效和阴影均不得互相接触、重叠或连接"));
        assert!(chinese.contains("空间不足时必须统一缩小所有主体"));
        assert!(chinese.contains("保持所选画布比例及正常2K或4K输出尺寸不变"));
        assert!(chinese.contains("确保每个主体都能被单独完整抠图"));

        let english =
            compose_canvas_workflow_prompt("Create {count} upgrade stages.", "wuxia hero", 6, true);
        assert!(english.contains(
            "Keep a clear, continuous solid-background gap between every pair of subjects"
        ));
        assert!(english.contains(
            "No silhouettes, clothing, weapons, gear, effects, or shadows may touch, overlap, or connect"
        ));
        assert!(english.contains("uniformly scale down all subjects"));
        assert!(english.contains("keep the selected canvas ratio and normal 2K or 4K output dimensions unchanged"));
        assert!(english.contains("each subject can be cleanly extracted on its own"));
    }

    #[test]
    fn canvas_workflow_prompt_splits_eight_subjects_into_two_rows() {
        let english = compose_canvas_workflow_prompt("Create {count} stages.", "", 8, true);
        assert!(english.contains("exactly 4 subjects in the top row and 4 in the bottom row"));
        assert!(english.contains("exactly 8 complete subjects"));
        let chinese = compose_canvas_workflow_prompt("生成{count}个角色", "", 8, false);
        assert!(chinese.contains("上排恰好4个，下排恰好4个"));
        assert!(chinese.contains("总共恰好8个完整主体"));
    }

    #[test]
    fn canvas_workflow_prompt_splits_nine_subjects_into_two_rows() {
        let english = compose_canvas_workflow_prompt("Create {count} stages.", "", 9, true);
        assert!(english.contains("exactly 5 subjects in the top row and 4 in the bottom row"));
        let chinese = compose_canvas_workflow_prompt("生成{count}个角色", "", 9, false);
        assert!(chinese.contains("上排恰好5个，下排恰好4个"));
        assert!(chinese.contains("总共恰好9个完整主体"));
    }

    #[test]
    fn canvas_workflow_defaults_to_five_steps() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().expect("create app window");

        assert_eq!(app.global::<AppState>().get_canvas_workflow_step_count(), 5);
    }

    #[test]
    fn legacy_canvas_workspace_prompts_are_migrated_to_single_line_text() {
        let mut workspaces = BTreeMap::from([(
            "plant-growth".to_string(),
            CanvasWorkspaceData {
                prompt: "\n番茄".to_string(),
                ..CanvasWorkspaceData::default()
            },
        )]);

        assert!(normalize_canvas_workspace_prompts(&mut workspaces));
        assert_eq!(workspaces["plant-growth"].prompt, "番茄");
        assert!(!normalize_canvas_workspace_prompts(&mut workspaces));
    }

    #[test]
    fn plant_growth_template_keeps_each_stage_on_separate_soil() {
        let launcher = include_str!("../../ui/pages/free-canvas-page.slint");

        assert!(launcher.contains("每个阶段分别位于一块独立的小土堆上"));
        assert!(launcher.contains("土块之间必须保留清晰的背景空隙"));
        assert!(launcher.contains("禁止形成连续土带、共享地面或相互连接的土壤"));
        assert!(launcher.contains("Place each stage on its own separate mound of soil"));
        assert!(launcher.contains("visible background gaps between every mound"));
        assert!(launcher.contains("Never form a continuous soil strip, shared ground, or connected soil"));
    }

    // Structural wiring checks only. Runtime/SQLite behavior is covered by the
    // owned toolbox and delivery tests; a source token is not a runtime receipt.
    fn core_toolbox_contract_block<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
        source.split_once(start).expect("current producer start").1
            .split_once(end).expect("current producer end").0
    }

    fn core_toolbox_drop_dispatch(source: &str) -> String {
        core_toolbox_contract_block(source, "fn process_captured_external_image_drops(",
            "\nfn external_drop_inside_reference_input(").split_whitespace().collect()
    }

    fn core_toolbox_delivery_metadata() -> String {
        core_toolbox_contract_block(include_str!("generation/controller.rs"),
            "fn stage_namespace_delivery(", "\npub(super) struct CommittedNamespaceDelivery")
            .split_whitespace().collect()
    }

    #[test]
    fn toolbox_conversion_reuses_the_batch_upload_layout() {
        let toolbox = include_str!("../../ui/pages/toolbox-page.slint");
        let conversion = include_str!("../../ui/pages/toolbox-conversion-page.slint");
        let compression = include_str!("../../ui/pages/toolbox-compression-page.slint");
        let state = include_str!("../../ui/app-state.slint");
        let app_ui = include_str!("../../ui/app.slint");
        let callbacks = include_str!("callbacks/toolbox.rs");
        let reference = include_str!("callbacks/reference.rs");

        assert!(toolbox.contains("target-page: \"toolbox-convert\""));
        assert!(app_ui.contains("AppState.page == \"toolbox-convert\""));
        assert!(app_ui.contains("ToolboxConversionPage"));
        assert!(compression.contains("export component CompressionDropArea"));
        assert!(compression.contains("export component CompressionListRow"));
        assert!(conversion.contains(
        "import { CompressionDropArea, CompressionListRow } from \"toolbox-compression-page.slint\""
    ));
        assert!(conversion.contains("AppState.choose-conversion-images()"));
        assert!(conversion.contains("AppState.paste-conversion-images()"));
        assert!(conversion.contains("AppState.remove-conversion-image(item.id)"));
        assert!(conversion.contains("AppState.save-conversion-result(item.id)"));
        assert!(conversion.contains("AppState.clear-conversion-images()"));
        assert!(conversion.contains("value <=> AppState.conversion-target-format"));
        assert!(!conversion.contains("AppState.conversion-quality"));
        assert!(!conversion.contains("quality-track"));
        assert!(conversion.contains("AppState.conversion-source-format + \" → \""));
        for format in [
            "JPEG (.jpg)",
            "PNG (.png)",
            "WebP (.webp)",
            "BMP (.bmp)",
            "AVIF (.avif)",
        ] {
            assert!(
                conversion.contains(format),
                "missing conversion option: {format}"
            );
        }
        assert!(conversion.contains("AppState.conversion-images.length"));
        assert!(conversion.contains("AppState.start-conversion()"));
        assert!(state.contains("in-out property <[CompressionImageItem]> conversion-images"));
        assert!(state.contains("conversion-target-format: \"jpeg\""));
        assert!(state.contains("conversion-saving: false"));
        assert!(state.contains("conversion-has-results: false"));
        assert!(!state.contains("conversion-quality"));
        assert!(!state.contains("conversion-estimated-credits"));
        assert!(conversion.contains("\"转换仅在本地进行，不会上传图片\""));
        assert!(conversion.contains("result-action-text: AppState.en ? \"Save\" : \"保存\""));
        assert!(conversion.contains("result-action-visible: item.status == \"completed\""));
        assert!(callbacks.contains("const MAX_CONVERSION_IMAGES: usize = 50;"));
        assert!(callbacks.contains("state.on_choose_conversion_images"));
        assert!(callbacks.contains("state.on_add_conversion_images_from_drag"));
        assert!(callbacks.contains("state.on_paste_conversion_images"));
        assert!(callbacks.contains("state.on_save_conversion_result"));
        assert!(callbacks.contains("state.on_start_conversion"));
        assert!(callbacks.contains("run_local_conversion_worker"));
        assert!(callbacks.contains("convert_image_file"));
        assert!(callbacks.contains("rfd::AsyncFileDialog::new()"));
        assert!(callbacks.contains("copy_and_release_managed_toolbox_result("));
        assert!(callbacks.contains("conversion_source_format"));
        let drops = core_toolbox_drop_dispatch(reference);
        assert!(drops.contains("\"toolbox-convert\"=>toolbox_callbacks::add_conversion_paths_for_store("));
        let import = core_toolbox_contract_block(callbacks, "pub(super) fn add_conversion_paths_for_store(",
            "\npub(super) fn add_conversion_drag_for_store(");
        assert!(import.contains("capture_toolbox_effect(store)"));
        assert!(import.contains("add_conversion_paths_captured"));
    }

    #[test]
    fn toolbox_colorize_matches_the_watermark_original_and_result_layout() {
        let toolbox = include_str!("../../ui/pages/toolbox-page.slint");
        let page = include_str!("../../ui/pages/toolbox-colorize-page.slint");
        let state = include_str!("../../ui/app-state.slint");
        let app_ui = include_str!("../../ui/app.slint");
        let callbacks = include_str!("callbacks/toolbox.rs");
        let api = include_str!("api/generation.rs");
        let recovery = include_str!("generation/backend.rs");
        let viewer = include_str!("presentation/sync.rs");
        let reference_callbacks = include_str!("callbacks/reference.rs");

        assert!(toolbox.contains("target-page: \"toolbox-colorize\""));
        assert!(app_ui.contains("AppState.page == \"toolbox-colorize\""));
        assert!(app_ui.contains("ToolboxColorizePage"));
        assert_eq!(page.matches("ColorizePreviewPanel {").count(), 3);
        assert!(page.contains("AppState.choose-colorize-source()"));
        assert!(page.contains("AppState.add-colorize-source-from-drag(data)"));
        assert!(page.contains("AppState.start-colorize()"));
        assert!(page.contains("AppState.reveal-colorize-result()"));
        assert!(page.contains("title: AppState.en ? \"Original\" : \"原图\""));
        assert!(page.contains("result-panel: true"));
        assert!(state.contains("colorize-estimated-credits: \"20\""));
        assert!(state.contains("callback add-colorize-source-from-drag(data-transfer) -> bool;"));
        assert!(callbacks.contains("state.on_choose_colorize_source"));
        assert!(callbacks.contains("state.on_add_colorize_source_from_drag"));
        assert!(callbacks.contains("add_colorization_from_drag_data"));
        assert!(callbacks.contains("start_external_colorization_import"));
        let start = core_toolbox_contract_block(callbacks, "pub(super) fn start_image_colorization_with_billing_scope(",
            "\npub(super) fn resume_pending_image_colorization(");
        assert!(start.contains("set_colorization_source_for_authority"));
        assert!(start.contains("persist_reference_image_for_namespace(&authority, &image)"));
        assert!(start.contains("upsert_pending_generation_for_namespace"));
        assert!(callbacks.contains("state.on_start_colorize"));
        assert!(callbacks.contains("state.on_reveal_colorize_result"));
        assert!(callbacks.contains("start_image_colorization"));
        assert!(callbacks.contains("CreateImageColorization"));
        assert!(callbacks.contains("create_image_colorization"));
        assert!(callbacks.contains("model_code: \"aliyun_image_colorization\""));
        let metadata = core_toolbox_delivery_metadata();
        assert!(metadata.contains("\"image_colorization\"=>Some((\"image_colorization\",\"老照片上色\"))"));
        assert!(metadata.contains("category:iftoolbox.is_some(){\"other\".into()}"));
        let worker = core_toolbox_contract_block(callbacks, "fn run_image_colorization_worker(",
            "\nfn poll_image_colorization_outcomes(");
        assert!(worker.contains("create_image_colorization_billing"));
        assert!(worker.contains("SavedReplayRequest::generation"));
        assert!(callbacks.contains("show_credit_rejection"));
        assert!(!callbacks.contains("老照片上色能力等待后端配置"));
        assert!(api.contains("/v1/toolbox/image-colorizations"));
        assert!(recovery.contains("resume_pending_image_colorization"));
        assert!(viewer.contains("item.origin != \"image_colorization\""));
        let drops = core_toolbox_drop_dispatch(reference_callbacks);
        assert!(drops.contains("\"toolbox-colorize\"=>{toolbox_callbacks::add_colorization_paths_for_store("));
        assert!(page.contains("text: AppState.en ? \"Change image\" : \"更换图片\""));
        assert!(page.contains("drop-enabled: !AppState.colorize-processing"));
    }

    #[test]
    fn toolbox_crop_is_a_free_local_editor_that_saves_other_assets() {
        let toolbox = include_str!("../../ui/pages/toolbox-page.slint");
        let page = include_str!("../../ui/pages/toolbox-crop-page.slint");
        let state = include_str!("../../ui/app-state.slint");
        let app_ui = include_str!("../../ui/app.slint");
        let callbacks = include_str!("callbacks/toolbox.rs");
        let reference = include_str!("callbacks/reference.rs");

        assert!(toolbox.contains("target-page: \"toolbox-crop\""));
        assert!(app_ui.contains("AppState.page == \"toolbox-crop\""));
        assert!(app_ui.contains("ToolboxCropPage"));
        assert!(page.contains("AppState.choose-crop-source()"));
        assert!(page.contains("AppState.paste-crop-source()"));
        assert!(page.contains("AppState.add-crop-source-from-drag"));
        assert!(page.contains("AppState.update-crop-rect"));
        assert!(page.contains("AppState.transform-crop-source"));
        assert!(page.contains("AppState.save-crop-result()"));
        for ratio in ["original", "free", "1:1", "4:3", "3:4", "16:9", "9:16"] {
            assert!(page.contains(&format!("value: \"{ratio}\"")));
        }
        assert!(state.contains("in-out property <string> crop-source-path"));
        assert!(state.contains("in-out property <float> crop-x"));
        assert!(state.contains("in-out property <float> crop-width"));
        assert!(page.contains("保持原始像素，不放大"));
        assert!(page.contains("本地处理 · 0积分"));
        assert!(!page.contains("crop-width-px"));
        assert!(!page.contains("crop-estimated-credits"));
        assert!(callbacks.contains("state.on_choose_crop_source"));
        assert!(callbacks.contains("state.on_save_crop_result"));
        assert!(callbacks.contains("process_crop_result"));
        assert!(callbacks.contains("origin: \"image_crop\""));
        assert!(callbacks.contains("category: \"other\""));
        assert!(callbacks.contains("store.assets.insert(0, item)"));
        let drops = core_toolbox_drop_dispatch(reference);
        assert!(drops.contains("\"toolbox-crop\"=>{toolbox_callbacks::add_crop_paths_for_store("));
        let projection = core_toolbox_contract_block(callbacks, "fn enqueue_toolbox_asset_projection(",
            "\nfn poll_toolbox_asset_ack");
        assert!(projection.contains("store.assets.insert(0, item)"));
        assert!(projection.contains("prepare_ordered_save"));
        assert!(projection.contains(".enqueue(local_store_data(app, &store))"));
        assert!(!projection.contains("store.generations.insert"));
    }

    #[test]
    fn toolbox_compression_runs_locally_and_saves_results() {
        let toolbox = include_str!("../../ui/pages/toolbox-page.slint");
        let page = include_str!("../../ui/pages/toolbox-compression-page.slint");
        let state = include_str!("../../ui/app-state.slint");
        let types = include_str!("../../ui/types.slint");
        let app_ui = include_str!("../../ui/app.slint");
        let callbacks = include_str!("callbacks/toolbox.rs");
        let image_processing = include_str!("services/image_processing.rs");
        let reference = include_str!("callbacks/reference.rs");
        let formats = include_str!("../image_formats.rs");

        assert!(toolbox.contains("target-page: \"toolbox-compress\""));
        assert!(app_ui.contains("AppState.page == \"toolbox-compress\""));
        assert!(app_ui.contains("ToolboxCompressionPage"));
        assert!(types.contains("export struct CompressionImageItem"));
        assert!(types.contains("status: string"));
        assert!(types.contains("result-path: string"));
        assert!(state.contains("in-out property <[CompressionImageItem]> compression-images"));
        assert!(state.contains("compression-saving: false"));
        assert!(state.contains("compression-has-results: false"));
        assert!(!state.contains("compression-estimated-credits"));

        assert!(page.contains("CompressionDropArea"));
        assert!(page.contains("CompressionListRow"));
        assert!(page.contains("AppState.choose-compression-images()"));
        assert!(page.contains("AppState.paste-compression-images()"));
        assert!(page.contains("AppState.remove-compression-image(item.id)"));
        assert!(page.contains("AppState.save-compression-result(item.id)"));
        assert!(page.contains("root.item.status == \"completed\""));
        assert!(page.contains("AppState.en ? \"Completed\" : \"已完成\""));
        assert!(page.contains("result-action-text: AppState.en ? \"Save\" : \"保存\""));
        assert!(page.contains(
            "result-action-visible: item.status == \"completed\" && item.result-path != \"\""
        ));
        assert!(page.contains("result-action-disabled: AppState.compression-saving"));
        assert!(page.contains("remove-disabled: root.busy"));
        assert!(page.contains("AppState.clear-compression-images()"));
        assert!(page.contains("@image-url(\"../../assets/icons/trash.svg\")"));
        assert!(page.contains("AppState.compression-mode = \"quality\""));
        assert!(page.contains("AppState.compression-mode = \"size\""));
        assert!(page.contains("AppState.compression-target-kb"));
        assert!(page.contains("AppState.compression-target-mb + \" MB\""));
        assert!(page.contains(
            "property <bool> busy: AppState.compression-processing || AppState.compression-saving"
        ));
        assert!(page.contains("disabled: root.busy"));
        assert!(page.contains("enabled: !root.busy"));
        assert!(!page.contains("compression-estimated-credits"));
        assert!(!page.contains("Estimated cost"));
        assert!(!page.contains("本次压缩预计消耗"));
        assert!(page.contains("AppState.start-compression()"));

        assert!(callbacks.contains("const MAX_COMPRESSION_IMAGES: usize = 50;"));
        assert!(callbacks.contains(".pick_files()"));
        assert!(callbacks.contains("state.on_paste_compression_images"));
        assert!(callbacks.contains("state.on_remove_compression_image"));
        assert!(callbacks.contains("state.on_save_compression_result"));
        assert!(callbacks.contains("status: \"pending\".into()"));
        assert!(callbacks.contains("state.on_update_compression_target_preview"));
        assert!(callbacks.contains("kilobytes / 1024.0"));
        assert!(callbacks.contains("state.on_start_compression"));
        assert!(callbacks.contains("start_local_compression"));
        assert!(callbacks.contains("run_local_compression_worker"));
        assert!(callbacks.contains("compress_image_file"));
        assert!(callbacks.contains("start_compression_result_save"));
        assert!(callbacks.contains("normalize_compression_destination"));
        assert!(callbacks.contains("rfd::AsyncFileDialog::new()"));
        assert!(callbacks.contains("copy_and_release_managed_toolbox_result("));
        assert!(!callbacks.contains("state.on_reveal_compression_result"));
        assert!(!callbacks.contains("set_compression_estimated_credits"));
        assert!(!callbacks.contains("图片压缩能力等待后端配置"));
        assert!(callbacks.contains("crate::image_formats::picker_image_extensions()"));
        let compression_import = core_toolbox_contract_block(callbacks,
            "fn add_compression_paths_captured(", "\npub(super) fn add_compression_paths(");
        assert!(compression_import.contains("persistence.storage_authority()"));
        assert!(compression_import.contains("load_toolbox_preview_for_authority(&authority, &path, PreviewPurpose::Toolbox)"));
        let preview = core_toolbox_contract_block(callbacks, "fn load_toolbox_preview_for_authority(",
            "\nfn write_toolbox_owned_output(");
        assert!(preview.contains("authority.read_image_source"));
        assert!(preview.contains("decode_image_bytes(path, &bytes)"));
        assert!(!compression_import.contains("is_compression_image_path"));
        assert!(!compression_import.contains("image::open("));
        assert!(image_processing.contains("ImageCompressionMode::Quality"));
        assert!(image_processing.contains("ImageCompressionMode::TargetBytes"));
        assert!(image_processing.contains("resize_image_by_scale"));
        assert!(image_processing.contains("CompressionFormat::Jpeg"));
        assert!(image_processing.contains("CompressionFormat::Png"));
        assert!(image_processing.contains("CompressionFormat::WebP"));
        assert!(image_processing.contains("CompressionFormat::Bmp"));
        let drops = core_toolbox_drop_dispatch(reference);
        assert!(drops.contains("\"toolbox-compress\"=>toolbox_callbacks::add_compression_paths_for_store("));
        assert!(formats.contains("\"bmp\""));
        assert!(formats.contains("\"gif\""));
        assert!(formats.contains("\"tiff\""));
    }

    #[test]
    fn toolbox_enhance_submits_a_fixed_price_task_and_saves_an_other_asset() {
        let toolbox = include_str!("../../ui/pages/toolbox-page.slint");
        let page = include_str!("../../ui/pages/toolbox-enhance-page.slint");
        let state = include_str!("../../ui/app-state.slint");
        let app_ui = include_str!("../../ui/app.slint");
        let app = include_str!("app.rs");
        let callbacks = include_str!("callbacks/image_enhancement.rs");
        let api = include_str!("api/generation.rs");
        let reference_callbacks = include_str!("callbacks/reference.rs");
        let recovery = include_str!("generation/backend.rs");
        let viewer = include_str!("presentation/sync.rs");

        assert!(toolbox.contains("target-page: \"toolbox-enhance\""));
        assert!(toolbox.contains("title: AppState.en ? \"Image Enhance\" : \"图片清晰\""));
        assert!(app_ui.contains("AppState.page == \"toolbox-enhance\""));
        assert!(app_ui.contains("ToolboxEnhancePage"));
        assert!(app.contains("page.starts_with(\"toolbox-\")"));
        assert_eq!(page.matches("EnhancePreviewPanel {").count(), 3);
        assert!(page.contains("text: AppState.en ? \"Image Enhance\" : \"图片清晰\""));
        assert!(!page.contains("图片变清晰"));
        assert!(!page.contains("一键智能超分"));
        assert!(!page.contains("One-click smart super resolution"));
        assert!(page.contains("title: AppState.en ? \"Original\" : \"原图\""));
        assert!(page.contains("AppState.choose-enhance-source()"));
        assert_eq!(page.matches("AppState.choose-enhance-source()").count(), 2);
        assert!(page.contains("source-drop := DropArea"));
        assert!(page.contains("AppState.add-enhance-source-from-drag(data)"));
        assert!(page.contains("drop-enabled: !AppState.enhance-processing"));
        assert!(page.contains("text: AppState.en ? \"Change image\" : \"更换图片\""));
        assert_eq!(page.matches("EnhanceQualityButton {").count(), 3);
        assert!(page.contains("value: \"2K\""));
        assert!(page.contains("value: \"4K\""));
        assert!(page.contains("AppState.start-enhance(AppState.enhance-quality)"));
        assert!(page.contains("AppState.reveal-enhance-result()"));
        assert!(page.contains("disabled: AppState.enhance-result-path == \"\""));
        assert!(state.contains("in-out property <string> enhance-quality: \"2K\""));
        assert!(state.contains("enhance-estimated-credits: \"20\""));
        assert!(state.contains("in-out property <int> enhance-progress: 0"));
        assert!(page.contains("\"本次预计扣除 \" + AppState.enhance-estimated-credits + \" 积分\""));
        assert!(state.contains("enhance-result-path"));
        assert!(state.contains("enhance-result-image"));
        assert!(state.contains("callback choose-enhance-source()"));
        assert!(state.contains("callback add-enhance-source-from-drag(data-transfer) -> bool"));
        assert!(state.contains("callback start-enhance(string)"));
        assert!(state.contains("callback reveal-enhance-result()"));
        assert!(callbacks.contains("state.on_choose_enhance_source"));
        assert!(callbacks.contains("state.on_add_enhance_source_from_drag"));
        assert!(callbacks.contains("state.on_start_enhance"));
        assert!(callbacks.contains("state.on_reveal_enhance_result"));
        assert!(callbacks.contains("normalized_enhancement_quality"));
        let compact: String = callbacks.split_whitespace().collect();
        assert!(compact.contains("ENHANCEMENT_MAX_INPUT_BYTES:u64=20*1024*1024;"));
        assert!(compact.contains("ENHANCEMENT_MIN_EDGE:u32=64;"));
        assert!(compact.contains("ENHANCEMENT_MAX_LONG_EDGE:u32=5000;"));
        assert!(!callbacks.contains("ENHANCEMENT_MAX_SHORT_EDGE"));
        assert!(compact.contains("ENHANCEMENT_MAX_ASPECT_RATIO:u32=2;"));
        assert!(callbacks.contains("state.set_enhance_estimated_credits(\"20\""));
        assert!(!callbacks.contains("state.set_enhance_estimated_credits(\"10\""));
        let metadata = core_toolbox_delivery_metadata();
        assert!(metadata.contains("\"image_enhancement\"=>Some((\"image_enhancement\",\"图片清晰\"))"));
        assert!(callbacks.contains("target_quality:"));
        assert!(callbacks.contains("CreateImageEnhancement"));
        assert!(callbacks.contains("image_enhancement"));
        assert!(metadata.contains("category:iftoolbox.is_some(){\"other\".into()}"));
        assert!(metadata.contains("origin:toolbox.map(|(origin,_)|origin)"));
        assert!(metadata.contains("upscale_done:record.task_type==\"image_upscale\"||enhancement"));
        let finish = core_toolbox_contract_block(callbacks, "fn finish_enhancement_work(",
            "\nfn enhancement_worker_current(");
        assert!(finish.contains("start_image_delivery_commit"));
        assert!(finish.contains("original.binding_matches()"));
        assert!(finish.contains("original.presentation_matches(app)"));
        assert!(api.contains("/v1/toolbox/image-enhancements"));
        assert!(api.contains("pub(crate) target_quality: String"));
        let drops = core_toolbox_drop_dispatch(reference_callbacks);
        assert!(drops.contains("\"toolbox-enhance\"=>{image_enhancement_callbacks::add_enhancement_paths_for_store("));
        assert!(recovery.contains("resume_pending_image_enhancement"));
        // The fixed model is retained by the real new-request producer; local
        // recovery dispatches that original row, not today's catalog selection.
        let new_record = core_toolbox_contract_block(callbacks,
            "fn new_enhancement_record(", "\npub(super) fn resume_pending_image_enhancement(");
        assert!(new_record.contains("model_code:\"aliyun_super_resolution\".into()"));
        assert!(new_record.contains("task_type:\"image_enhancement\".into()"));
        let submit = core_toolbox_contract_block(callbacks,
            "pub(super) fn start_image_enhancement_with_billing_scope(", "\nfn new_enhancement_record(");
        assert!(submit.find("let record=new_enhancement_record(").unwrap()
            < submit.find("upsert_pending_generation_for_namespace(&authority,&billing,record.clone())?").unwrap());
        assert!(submit.contains("run_enhancement_record(&backend,&authority,Some(&billing),&billing.request.session,record,cancel,progress)"));
        let resume = core_toolbox_contract_block(callbacks,
            "pub(super) fn resume_pending_image_enhancement(", "\nfn finish_enhancement_work(");
        assert!(resume.contains("run_enhancement_record(&backend,&authority,None,&session,record,cancel,progress)"));
        let dispatch = core_toolbox_contract_block(recovery,
            "if record.task_type == \"image_enhancement\" {", "if record.task_type == \"image_cutout\" {");
        assert!(dispatch.contains("resume_pending_image_enhancement(&app, context.clone(), record)"));
        assert!(viewer.contains("item.origin != \"image_enhancement\""));
    }

    #[test]
    fn watermark_tool_submits_a_fixed_price_task_and_saves_an_other_asset() {
        let toolbox = include_str!("../../ui/pages/toolbox-page.slint");
        let page = include_str!("../../ui/pages/toolbox-watermark-page.slint");
        let state = include_str!("../../ui/app-state.slint");
        let app = include_str!("app.rs");
        let callbacks = include_str!("callbacks/toolbox.rs");
        let reference_callbacks = include_str!("callbacks/reference.rs");

        assert!(toolbox.contains("target-page: \"toolbox-watermark\""));
        assert!(page.contains("AppState.choose-watermark-source()"));
        assert!(page.contains("AppState.start-watermark-removal()"));
        assert!(page.contains("AppState.reveal-watermark-result()"));
        assert_eq!(
            page.matches("AppState.choose-watermark-source()").count(),
            2
        );
        assert!(page.contains("text: AppState.en ? \"Change image\" : \"更换图片\""));
        assert!(page.contains("disabled: AppState.watermark-processing"));
        assert!(page.contains("source-drop := DropArea"));
        assert!(page.contains("return root.drop-enabled ? DragAction.copy : DragAction.none;"));
        assert!(page.contains("AppState.add-watermark-source-from-drag(data)"));
        assert!(page.contains("松开即可上传图片"));
        assert!(page.contains("x: 32px + root.panel-width() - 166px;"));
        assert!(!page.contains("Upload an image. The processed file stays local"));
        assert!(!page.contains("结果仅保存在本地"));
        assert_eq!(page.matches("y: 76px;").count(), 2);
        assert!(page.contains("查看图片"));
        assert!(page.contains("去水印中("));
        assert!(state.contains("watermark-result-path"));
        assert!(state.contains("watermark-estimated-credits: \"20\""));
        assert!(state.contains("callback add-watermark-source-from-drag(data-transfer) -> bool"));
        assert!(
            page.contains("\"本次预计扣除 \" + AppState.watermark-estimated-credits + \" 积分\"")
        );
        assert!(callbacks.contains("rfd::FileDialog::new()"));
        assert!(callbacks.contains("state.on_add_watermark_source_from_drag"));
        assert!(callbacks.contains("add_watermark_from_drag_data"));
        assert!(callbacks.contains("set_watermark_source_from_path"));
        assert!(callbacks.contains("external_image_url(data)"));
        assert!(callbacks.contains("start_external_watermark_import"));
        let drops = core_toolbox_drop_dispatch(reference_callbacks);
        assert!(drops.contains("\"toolbox-watermark\"=>{toolbox_callbacks::add_watermark_paths_for_store("));
        assert!(callbacks.contains("reveal_path_in_file_manager(&path)"));
        assert!(callbacks.contains("CreateWatermarkRemoval"));
        assert!(callbacks.contains("image_watermark_removal"));
        assert!(callbacks.contains("category: \"other\".to_string()"));
        let metadata = core_toolbox_delivery_metadata();
        assert!(metadata.contains("\"image_watermark_removal\"=>Some((\"watermark_removal\",\"去水印\"))"));
        assert!(metadata.contains("category:iftoolbox.is_some(){\"other\".into()}"));
        assert!(metadata.contains("iftoolbox.is_none(){reveal_prompt_history_entry(store,&item.prompt);store.generations.insert(0,item.clone());}"));
        assert!(metadata.contains("store.assets.insert(0,item)"));
        let delivery = core_toolbox_contract_block(callbacks, "fn enqueue_toolbox_remote_delivery(",
            "\n#[derive");
        assert!(delivery.contains("prepared.ensure_current()"));
        assert!(delivery.contains("start_image_delivery_commit"));
        assert!(delivery.contains("ToolboxRemoteKind::Watermark"));
        assert!(state.contains("viewer-repeat-enabled"));
        assert!(include_str!("../../ui/dialogs/viewer-overlay.slint")
            .contains("AppState.viewer-repeat-enabled"));
        assert!(app.contains("if page.starts_with(\"toolbox-\")"));
        assert!(app.contains("navigate_to_with_store(&app, &store.borrow(), \"toolbox\")"));
    }

    #[test]
    fn idle_generation_area_rotates_slash_usage_tips() {
        let panel = include_str!("../../ui/components/studio-work-panel.slint");
        let tips = include_str!("../../ui/components/usage-tip-carousel.slint");

        assert!(panel.contains("UsageTipCarousel"));
        assert!(panel.contains("AppState.generation-status == \"\""));
        assert!(tips.contains("interval: 4200ms"));
        assert!(tips.contains("Math.mod(root.active-tip + 1, 2)"));
        assert!(tips.contains(": \"输入“/”可查看最近的提示词记录\""));
        assert!(tips.contains(": \"输入“//”可查看自定义提示词\""));
        assert!(!tips.contains("Tip 1"));
        assert!(!tips.contains("Tip 2"));
        assert!(!tips.contains("1、"));
        assert!(!tips.contains("2、"));
        assert_eq!(tips.matches("animate y").count(), 2);
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
    fn prompt_popups_preserve_full_prompt_values_and_custom_names() {
        assert_eq!(
            single_line_prompt_preview("first line\nsecond\tline  end"),
            "first line second line end"
        );

        let composer = include_str!("../../ui/components/prompt-composer.slint");
        assert!(composer.matches("min(10, AppState.").count() >= 2);
        assert!(composer.matches("wrap: no-wrap;").count() >= 3);
        assert!(composer.contains("root.apply-selected-prompt(AppState.prompt-history[index])"));
        assert!(composer.contains("root.queue-custom-prompt-selection(item.content)"));
        assert!(composer.contains("viewport-height: AppState.prompt-history.length * 32px"));
        assert!(composer.contains("for item[index] in AppState.custom-prompt-items"));
        assert!(composer.contains("text: item.name"));
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
            .find("AppState.normalize-prompt-editor-text(value, \"\")")
            .expect("prompt value assignment");
        let cursor_position = apply_prompt
            .find("prompt-input.set-selection-offsets(2147483647, 2147483647)")
            .expect("prompt cursor moves to the end");
        assert!(focus_position < write_position);
        assert!(write_position < cursor_position);
        assert!(composer.contains("暂无自定义提示词，点击创建"));
        assert!(composer.contains("AppState.settings-section = \"prompts\""));
        assert!(composer.contains("AppState.navigate(\"settings\")"));
        assert!(state.contains("in-out property <string> settings-section: \"basic\""));
        assert!(settings.contains("AppState.settings-section"));
    }

    #[test]
    fn enter_confirms_auth_inputs_and_alt_enter_keeps_prompt_line_breaks() {
        let field = include_str!("../../ui/components/field.slint");
        let auth = include_str!("../../ui/dialogs/auth-dialog.slint");
        let prompt = include_str!("../../ui/components/prompt-composer.slint");

        assert!(field.contains("callback accepted();"));
        assert!(field.contains("accepted => { root.accepted(); }"));

        assert!(auth.contains("function confirm-auth()"));
        assert_eq!(
            auth.matches("accepted => { root.confirm-auth(); }").count(),
            3
        );

        assert!(prompt.contains("event.text == Key.Return"));
        assert!(prompt.contains("event.modifiers.alt"));
        assert!(prompt.contains("return reject"));
        assert!(prompt.contains("AppState.generate()"));
        assert!(prompt.contains("return accept"));
    }

    #[test]
    fn auth_and_password_management_share_the_segmented_mode_switch() {
        let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let segmented = std::fs::read_to_string(
            manifest_dir.join("ui/components/segmented-control.slint"),
        )
        .unwrap_or_default();
        let auth = include_str!("../../ui/dialogs/auth-dialog.slint");
        let profile = include_str!("../../ui/dialogs/profile-dialog.slint");

        assert!(!segmented.is_empty(), "shared segmented control source");
        for contract in [
            "accessible-role: button;",
            "accessible-role: none;",
            "accessible-enabled: !root.disabled;",
            "accessible-checkable: true;",
            "accessible-checked: root.active;",
            "accessible-action-default =>",
            "forward-focus:",
            "FocusScope",
            "key-pressed(event)",
            "focus.has-focus ? 2px",
        ] {
            assert!(segmented.contains(contract), "{contract}");
        }
        assert_eq!(auth.matches("SegmentedControl {").count(), 1);
        assert_eq!(profile.matches("SegmentedControl {").count(), 1);
        for contract in [
            "AppState.auth-email-mode == \"code\"",
            "AppState.auth-email-mode == \"password\"",
        ] {
            assert!(auth.contains(contract), "{contract}");
        }
        for contract in [
            "AppState.password-verification-mode == \"current_password\"",
            "AppState.password-verification-mode == \"email_code\"",
        ] {
            assert!(profile.contains(contract), "{contract}");
        }
    }

    #[test]
    fn secret_fields_use_accessible_eye_icons_instead_of_text() {
        let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let field = include_str!("../../ui/components/field.slint");
        let auth = include_str!("../../ui/dialogs/auth-dialog.slint");
        let profile = include_str!("../../ui/dialogs/profile-dialog.slint");

        assert!(field.contains("input-type: root.secret && !root.revealed ? password : text;"));
        assert!(field.contains("root.revealed = !root.revealed;"));
        assert!(field.contains("assets/icons/eye.svg"));
        assert!(field.contains("assets/icons/eye-off.svg"));
        assert!(field.contains("accessible-role: button;"));
        assert!(field.contains("accessible-label: root.revealed"));
        for contract in [
            "accessible-role: none;",
            "accessible-enabled: root.enabled;",
            "accessible-action-default =>",
            "forward-focus:",
            "FocusScope",
            "key-pressed(event)",
        ] {
            assert!(field.contains(contract), "{contract}");
        }
        assert!(!field.contains("AppState.en ? \"Hide\" : \"隐藏\""));
        assert!(!field.contains("AppState.en ? \"Show\" : \"显示\""));
        assert!(manifest_dir.join("assets/icons/eye.svg").is_file());
        assert!(manifest_dir.join("assets/icons/eye-off.svg").is_file());
        assert_eq!(auth.matches("secret: true;").count(), 3);
        assert_eq!(profile.matches("secret: true;").count(), 3);
    }

    #[test]
    fn long_prompt_input_scrolls_inside_its_fixed_viewport() {
        let prompt = include_str!("../../ui/components/prompt-composer.slint");

        assert!(prompt.contains("prompt-scroll := ScrollView"));
        assert!(prompt
            .contains("viewport-height: max(self.visible-height, prompt-input.preferred-height);"));
        assert!(prompt.contains("page-height: prompt-scroll.visible-height;"));
        assert!(prompt.contains("cursor-position-changed(position)"));
        assert!(prompt.contains("prompt-scroll.viewport-y"));
    }

    #[test]
    fn deep_prompt_drawer_contains_long_text_inside_fixed_cards() {
        let drawer = include_str!("../../ui/dialogs/deep-prompt-optimization-drawer.slint");

        assert!(drawer.matches("read-only: true").count() >= 4);
        assert!(drawer.matches("clip: true").count() >= 9);
        assert!(drawer.contains("panel-hit-blocker := TouchArea"));
        assert!(drawer.contains("current-prompt-frame := Rectangle"));
        assert!(drawer.contains("current-prompt-scroll := ScrollView"));
        assert!(drawer.contains("current-prompt-text := Text"));
        assert!(drawer.contains("text: AppState.prompt;"));
        assert!(drawer.contains("color: AppTheme.muted;"));
        assert!(drawer.contains("wrap: word-wrap;"));
        assert!(drawer.contains("vertical-scrollbar-policy: ScrollBarPolicy.always-on;",));
        assert!(drawer.contains("horizontal-scrollbar-policy: ScrollBarPolicy.always-off;",));
        assert!(drawer.contains("mouse-drag-pan-enabled: true;"));
        assert!(drawer.contains("progress-fill := Rectangle"));
        assert!(drawer.contains("x: 0px;"));
        assert!(drawer.contains(
            "width: max(0px, min(parent.width, parent.width * AppState.deep-optimization-progress / 100));",
        ));
        assert!(drawer.contains("text: AppState.deep-optimization-change-summary"));
        assert!(drawer.contains(
            "\"本次最多消耗 \" + AppState.deep-optimization-maximum-credits + \" 积分\"",
        ));
    }

    #[test]
    fn studio_generation_settings_include_collapsible_negative_prompt_and_unified_image_settings() {
        let chooser = include_str!("../../ui/components/inline-card-chooser.slint");
        let panel = include_str!("../../ui/components/studio-work-panel.slint");
        let prompt = include_str!("../../ui/components/prompt-composer.slint");
        let negative = include_str!("../../ui/components/negative-prompt-editor.slint");

        assert_eq!(
            chooser.matches("settings-popup := PopupWindow {").count(),
            1
        );
        assert!(chooser.contains("text: root.settings-summary();"));
        assert!(chooser.contains("Image settings · "));
        assert!(chooser.contains("图片设置 · "));
        assert!(chooser.contains("text: AppState.en ? \"Image settings\" : \"图片设置\""));
        assert!(chooser.contains("text: AppState.en ? \"Quality\" : \"质量\""));
        assert!(chooser.contains("text: AppState.en ? \"Aspect ratio\" : \"宽高比\""));
        assert!(chooser.contains("text: AppState.en ? \"Generation count\" : \"生成张数\""));
        assert!(!chooser.contains("text: \"W\""));
        assert!(!chooser.contains("text: \"H\""));
        assert!(!chooser.contains("image-width-text"));
        assert!(!chooser.contains("image-height-text"));
        assert!(chooser.contains("source: @image-url(\"../../assets/icons/controls.svg\")"));
        assert!(chooser.contains("height: 42px;"));
        assert!(chooser.contains("y: 0px - self.height - 8px;"));
        assert!(chooser.contains("width: 378px;"));
        assert!(chooser.contains("x: root.width - self.width;"));
        assert!(!chooser.contains("min(378px, root.width)"));
        assert!(
            chooser.contains("border-color: root.selected ? AppTheme.accent : AppTheme.border;")
        );
        assert_eq!(chooser.matches("ImageRatioOption { value:").count(), 11);
        assert_eq!(chooser.matches("ImageSettingPill { text:").count(), 7);
        assert!(panel.contains("settings-row := HorizontalLayout"));
        assert!(panel.contains("y: negative-editor.y + negative-editor.height + 12px"));
        assert!(panel.contains("CreationModeChip { width: 100px; height: 42px; }"));
        assert!(panel.contains("StyleModeChip { width: 100px; height: 42px; }"));
        assert!(panel.contains("AdvancedControlChip { width: 100px; height: 42px; }"));
        assert!(panel.contains("InlineCardChooser { horizontal-stretch: 1; }"));
        assert!(!prompt.contains("InlineCardChooser"));
        assert!(panel.contains("work-scroll := ScrollView"));
        assert!(panel.contains("viewport-height: max(self.visible-height, work-content.height)"));
        assert!(panel.contains("generate-action := GenerateActionButton"));
        assert!(panel.contains("y: settings-row.y + settings-row.height + 52px"));
        assert!(panel.contains("y: generate-action.y + generate-action.height + 14px"));
        assert!(!panel.contains("parent.height - 266px - negative-editor.height"));
        assert!(panel.contains("negative-editor := NegativePromptEditor"));
        assert!(negative.contains("height: AppState.negative-prompt-expanded ? 132px : 46px"));
        assert!(negative.contains("text <=> AppState.negative-prompt"));
        assert!(negative.contains("填写不希望画面中出现的内容"));
        assert!(negative.contains("x: parent.width - 42px"));
        assert!(negative.contains("negative-prompt-dropdown.svg"));
        assert!(negative
            .contains("transform-rotation: AppState.negative-prompt-expanded ? 180deg : 0deg"));
        assert!(prompt.contains("? 650px : 600px"));
        assert!(prompt.contains("border-radius: 12px;"));
        assert!(prompt.contains("width: 64px;"));
        assert!(prompt.contains("x: 94px;"));
        assert!(prompt.contains("x: 212px;"));

        for chip in [
            include_str!("../../ui/components/creation-mode-chip.slint"),
            include_str!("../../ui/components/style-mode-chip.slint"),
            include_str!("../../ui/components/advanced-control-chip.slint"),
        ] {
            assert!(chip.contains("y: 0px - self.height - 8px;"));
        }
    }

    #[test]
    fn studio_content_stays_top_aligned_in_tall_windows_for_every_category() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().expect("create app window");
        let state = app.global::<AppState>();

        state.set_logged_in(true);
        state.set_page("generation".into());
        app.window()
            .set_size(slint::LogicalSize::new(1351.0, 1335.0));
        app.show().expect("show app window");

        for category in ["character", "scene", "ui", "effect"] {
            state.set_asset_type(category.into());
            let composers = i_slint_backend_testing::ElementHandle::find_by_element_type_name(
                &app,
                "PromptComposer",
            )
            .collect::<Vec<_>>();
            assert_eq!(composers.len(), 1, "expected one composer for {category}");
            let composer_y = composers[0].absolute_position().y;
            assert!(
                composer_y < 200.0,
                "{category} composer should stay near the workbench header, but started at y={composer_y}"
            );
        }
    }

    #[test]
    fn prompt_optimization_actions_are_compact_backgroundless_tags() {
        let composer = include_str!("../../ui/components/prompt-composer.slint");

        assert!(composer.contains("component PromptOptimizationAction"));
        assert_eq!(composer.matches("PromptOptimizationAction {").count(), 5);
        assert!(composer.contains("background: transparent;"));
        assert!(composer.contains("border-width: 1px;"));
        assert!(composer.contains("border-radius: 12px;"));
        assert!(composer.contains("? AppTheme.accent : AppTheme.muted;"));
        assert!(composer.contains("font-size: 12px;"));
        assert!(composer.contains("font-weight: 400;"));
        assert!(
            !composer.contains("primary: true;\n            disabled: AppState.reasoning-model")
        );
    }

    #[test]
    fn prompt_clear_tag_clears_prompt_content_without_touching_reference_or_negative_state() {
        let composer = include_str!("../../ui/components/prompt-composer.slint");
        let clear_action = composer
            .split("text: AppState.en ? \"Clear\" : \"清空\";")
            .nth(1)
            .and_then(|value| value.split("if AppState.optimizing-prompt").next())
            .expect("clear prompt action");
        let clear_function = composer
            .split("function clear-current-prompt()")
            .nth(1)
            .and_then(|value| value.split("Timer {").next())
            .expect("clear prompt function");

        assert!(clear_action.contains("clicked => { root.clear-current-prompt(); }"));
        assert!(clear_function.contains("AppState.prompt = \"\";"));
        assert!(clear_function.contains("root.prompt-editor-text = \"\";"));
        assert!(clear_function.contains("AppState.clear-custom-prompt-selections();"));
        assert!(clear_function.contains("AppState.invalidate-deep-prompt-binding();"));
        assert!(!clear_function.contains("AppState.references"));
        assert!(!clear_function.contains("AppState.negative-prompt"));
    }

    #[test]
    fn image_quality_is_not_limited_by_membership() {
        let chooser = include_str!("../../ui/components/inline-card-chooser.slint");
        let canvas = include_str!("../../ui/pages/infinite-canvas-page.slint");
        let membership = include_str!("../../ui/components/membership-plans.slint");
        let profile = include_str!("../../ui/dialogs/profile-dialog.slint");
        let app = include_str!("../../ui/app.slint");

        for source in [chooser, canvas, membership, profile, app] {
            assert!(!source.contains("membership-max-quality"));
            assert!(!source.contains("QualityRestrictedDialog"));
        }
        assert!(chooser.contains("text: \"1K\""));
        assert!(chooser.contains("text: \"2K\""));
        assert!(chooser.contains("text: \"4K\""));
        assert!(!membership.contains("最高画质"));
        assert!(!membership.contains("Max quality"));
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
        use i_slint_backend_testing::ElementHandle;
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().unwrap();
        let state = app.global::<AppState>();
        state.set_page("generation".into());
        state.set_logged_in(true);
        app.show().unwrap();
        for width in [1180.0, 1364.0, 1600.0] {
            for payment in [false, true] {
                state.set_payment_active(payment);
                app.window().set_size(slint::LogicalSize::new(width, 928.0));
                let bar = ElementHandle::find_by_element_type_name(&app, "TopBar").next().unwrap();
                let mut pickers = bar.query_descendants().match_inherits("ModelPicker").find_all();
                assert_eq!(pickers.len(), 2, "both model selectors must remain available");
                pickers.sort_by(|a, b| a.absolute_position().x.total_cmp(&b.absolute_position().x));
                assert!((pickers[0].absolute_position().x - bar.absolute_position().x - 18.0).abs() <= 1.0);
                assert!((pickers[1].absolute_position().x - pickers[0].absolute_position().x
                    - pickers[0].size().width - 18.0).abs() <= 1.0);
                assert!((pickers[0].absolute_position().y - pickers[1].absolute_position().y).abs() <= 1.0,
                    "selectors must move together when the toolbar wraps");
                for picker in pickers {
                    assert!(picker.size().width >= 220.0);
                    assert!(picker.absolute_position().x + picker.size().width
                        <= bar.absolute_position().x + bar.size().width + 1.0);
                    assert!(picker.absolute_position().y + picker.size().height
                        <= bar.absolute_position().y + bar.size().height + 1.0);
                }
            }
        }
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
        let actions = callbacks.split_once("impl NotificationAction {").unwrap().1
            .split_once("fn wire_notification_callbacks(").unwrap().0;
        assert!(actions.contains("Self::Delete(id) => api.delete_scoped(&id, scope)"));
        assert!(actions.contains("Self::Clear => api.delete_all_scoped(scope)"));
        assert!(actions.contains("Self::Delete(id) => store.notifications.retain(|item| &item.id != id)"));
        assert!(actions.contains("Self::Clear => store.notifications.clear()"));

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
        assert!(!prompts.contains("clicked => { AppState.remove-custom-prompt(item.content); }"));
        assert!(notifications.contains("AppState.pending-delete-kind = \"notification\""));
        assert!(notifications.contains("AppState.pending-delete-kind = \"notifications-all\""));
        assert!(!notifications.contains("clicked => { AppState.delete-notification(item.id); }"));
        assert!(!notifications.contains("clicked => { AppState.clear-all-notifications(); }"));
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

    // These retained checks describe the current wiring, not native UI or
    // SQLite acceptance; the actual canvas callback tests cover those boundaries.
    fn core_canvas_contract_block<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
        source.split_once(start).expect("current canvas producer start").1
            .split_once(end).expect("current canvas producer end").0
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
        let callbacks =
            std::fs::read_to_string(manifest.join("src/runtime/callbacks/infinite_canvas.rs"))
                .unwrap_or_default();
        let local_store = include_str!("storage/local_store.rs");
        let sync = include_str!("presentation/sync.rs");

        let workbench = sidebar
            .find("CategoryWorkspaceMenu {")
            .expect("workbench menu");
        let canvas = sidebar
            .find("page: \"free-canvas\"")
            .expect("free canvas nav item");
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
        assert!(
            state.contains("callback finish-canvas-link(string, float, float, float) -> string")
        );
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
        assert!(!page.contains("Create the first node"));
        assert!(!page.contains("创建第一个节点"));
        assert!(!page.contains("if AppState.canvas-notes.length == 0: Rectangle"));
        assert!(page.contains("AppState.group-canvas-selection"));
        assert!(page.contains("AppState.undo-canvas()"));
        assert!(page.contains("AppState.redo-canvas()"));
        assert!(page.contains("canvas-minimap-open"));
        assert!(page.contains("canvas-grid-style"));
        assert!(page.contains(
            "grid-column-count: max(1, Math.ceil(self.width / root.grid-spacing()) + 1)"
        ));
        assert!(page
            .contains("grid-row-count: max(1, Math.ceil(self.height / root.grid-spacing()) + 1)"));
        assert!(page.contains("for column in canvas.grid-column-count"));
        assert!(page.contains("for row in canvas.grid-row-count"));
        assert!(!page.contains("for column in 70"));
        assert!(!page.contains("for row in 44"));
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
        let prepare = core_canvas_contract_block(&callbacks, "fn prepare_canvas_edit(",
            "\nfn apply_canvas_edit<R>");
        let commit = core_canvas_contract_block(&callbacks, "fn apply_canvas_edit_checked<R>",
            "\nfn start_canvas_edit_preview(");
        assert!(prepare.contains("capture.persistence.prepare_ordered_save()"));
        assert!(prepare.contains("prepare_canvas_store_ack_worker"));
        assert!(commit.contains("capture.apply(store"));
        assert!(commit.contains("local_store_data(app, &store)"));
        assert!(commit.contains(".enqueue(data)"));
        assert!(commit.contains("prepare_canvas_projection"));
        assert!(local_store.contains("canvas_notes: store.canvas_notes.clone()"));
        assert!(local_store.contains("canvas_links: store.canvas_links.clone()"));
        assert!(local_store.contains("let mut canvas_workspaces = store.canvas_workspaces.clone()"));
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
    fn free_canvas_entries_keep_independent_nodes_links_and_prompts() {
        let mut store = Store {
            active_canvas_workspace_id: "plant-growth".to_string(),
            canvas_notes: vec![CanvasNoteData {
                id: "plant-note".to_string(),
                content: "番茄".to_string(),
                selected: true,
                ..CanvasNoteData::default()
            }],
            canvas_links: vec![CanvasLinkData {
                id: "plant-link".to_string(),
                source_id: "plant-note".to_string(),
                target_id: "plant-note".to_string(),
                ..CanvasLinkData::default()
            }],
            canvas_references: vec![ReferenceData {
                id: "plant-reference".to_string(),
                source_path: "plant.png".to_string(),
            }],
            ..Store::default()
        };

        let monster_prompt = switch_canvas_workspace(
            &mut store,
            "\n红色番茄，陶盆",
            "monster-generator",
        );
        assert!(monster_prompt.is_empty());
        assert!(store.canvas_notes.is_empty());
        assert!(store.canvas_links.is_empty());
        assert!(store.canvas_references.is_empty());

        store.canvas_notes.push(CanvasNoteData {
            id: "monster-note".to_string(),
            content: "岩石怪物".to_string(),
            ..CanvasNoteData::default()
        });
        store.canvas_references.push(ReferenceData {
            id: "monster-reference".to_string(),
            source_path: "monster.png".to_string(),
        });
        let plant_prompt = switch_canvas_workspace(
            &mut store,
            "\r\n紫水晶石像\r\n",
            "plant-growth",
        );

        assert_eq!(plant_prompt, "红色番茄，陶盆");
        assert_eq!(store.canvas_notes.len(), 1);
        assert_eq!(store.canvas_notes[0].id, "plant-note");
        assert_eq!(store.canvas_links.len(), 1);
        assert_eq!(store.canvas_references[0].id, "plant-reference");
        assert!(!store.canvas_notes[0].selected);
        assert_eq!(
            store.canvas_workspaces["monster-generator"].notes[0].id,
            "monster-note"
        );
        assert_eq!(
            store.canvas_workspaces["monster-generator"].prompt,
            "紫水晶石像"
        );
        assert_eq!(
            store.canvas_workspaces["monster-generator"].references[0].id,
            "monster-reference"
        );
        assert_eq!(
            canvas_workspace_id_for_source(&store, "monster-note").as_deref(),
            Some("monster-generator")
        );
        assert_eq!(
            canvas_workspace_id_for_source(&store, "plant-note").as_deref(),
            Some("plant-growth")
        );
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
            "callback paste-canvas-content(float, float)",
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
            "on_paste_canvas_content",
            "on_duplicate_canvas_selection",
            "on_remove_canvas_selection",
            "on_group_canvas_selection",
            "on_ungroup_canvas_selection",
        ] {
            assert!(callbacks.contains(registration), "missing {registration}");
        }
    }

    #[test]
    fn infinite_canvas_selection_and_pan_modes_use_desktop_shortcuts() {
        let page = include_str!("../../ui/pages/infinite-canvas-page.slint");
        let canvas_pointer = page
            .split("canvas-touch := TouchArea")
            .nth(1)
            .and_then(|value| value.split("scroll-event(event)").next())
            .expect("canvas pointer handler");
        let canvas_scroll = page
            .split("scroll-event(event)")
            .nth(1)
            .and_then(|value| value.split("canvas-keyboard := FocusScope").next())
            .expect("canvas scroll handler");

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
        assert!(page.contains("root.workflow-hand-mode()"));
        assert!(include_str!("../../ui/app-state.slint")
            .contains("in-out property <string> canvas-tool: \"pan\""));
        assert!(page.contains("label: AppState.en ? \"Select\" : \"选择\""));
        assert!(page.contains("label: AppState.en ? \"Hand\" : \"抓手\""));
        assert!(page.contains("AppState.select-canvas-node(root.note.id, event.modifiers.shift);"));
        assert!(page.contains("root.marquee-additive = event.modifiers.shift;"));
        assert!(canvas_pointer.contains(
            "if event.button == PointerEventButton.middle || root.space-pan-active"
        ));
        assert!(canvas_scroll.contains("event.modifiers.control"));
        assert!(canvas_scroll.contains("root.set-zoom"));
        assert!(canvas_scroll.contains("root.pan-y += event.delta-y"));
        assert!(canvas_scroll.contains("root.pan-x += event.delta-x"));
        assert!(page.contains("if event.text == Key.Space"));
        assert!(page.contains("root.space-pan-active = true"));
        assert!(page.contains("root.space-pan-active = false"));
        assert!(canvas_pointer.contains(
            "root.temporary-pan-active || root.space-pan-active || root.workflow-hand-mode() ? pointer : default"
        ));
        assert!(page.contains(
            "if root.space-pan-active || root.workflow-hand-mode(): hand-pan-overlay := TouchArea"
        ));
        assert!(page.contains("mouse-cursor: pointer"));
        assert!(canvas_pointer.contains("root.workflow-hand-mode()"));
        assert!(
            !page.contains("AppState.select-canvas-node(root.note.id, event.modifiers.control);")
        );
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
    fn infinite_canvas_grouping_is_explicit_and_uses_a_dedicated_title_row() {
        let page = include_str!("../../ui/pages/infinite-canvas-page.slint");
        let dialog = include_str!("../../ui/dialogs/canvas-group-name-dialog.slint");
        let callbacks = include_str!("callbacks/infinite_canvas.rs");
        let ops = include_str!("canvas_ops.rs");
        let move_handler = callbacks
            .split("state.on_move_canvas_selection")
            .nth(1)
            .and_then(|value| value.split("state.on_copy_canvas_selection").next())
            .expect("move canvas selection handler");
        let canvas_node = page
            .split("component CanvasNodeCard")
            .nth(1)
            .expect("canvas node card");
        let group_header = canvas_node
            .split("header := Rectangle")
            .nth(1)
            .and_then(|value| value.split("if root.note.kind == \"text\"").next())
            .expect("group header");

        assert!(!move_handler.contains("assign_deepest_group"));
        assert!(ops.contains("pub(super) const GROUP_TOP_PADDING: f32 = 72.0"));
        assert!(ops.contains("y: bounds.y - GROUP_TOP_PADDING"));
        assert!(callbacks.contains("next_group_name(&store_mut.canvas_notes"));
        assert!(group_header.contains("x: 0px;"));
        assert!(group_header.contains("y: 0px;"));
        assert!(group_header.contains("width: parent.width;"));
        assert!(group_header.contains("text: root.current-content"));
        assert!(group_header.contains("font-size: 18px * root.group-control-scale()"));
        assert!(dialog.contains("text <=> AppState.canvas-group-name-edit-value"));
        assert!(!group_header.contains("text: root.node-title()"));
    }

    #[test]
    fn infinite_canvas_group_header_has_large_actions_and_dedicated_dialogs() {
        let app = include_str!("../../ui/app.slint");
        let state = include_str!("../../ui/app-state.slint");
        let page = include_str!("../../ui/pages/infinite-canvas-page.slint");
        let dialog = include_str!("../../ui/dialogs/canvas-group-name-dialog.slint");
        let delete = include_str!("../../ui/dialogs/delete-confirm.slint");
        let callbacks = include_str!("callbacks/infinite_canvas.rs");

        assert!(app.contains("import { CanvasGroupNameDialog }"));
        assert!(app.contains("CanvasGroupNameDialog {"));
        assert!(state.contains("canvas-group-name-dialog-open"));
        assert!(state.contains("callback rename-canvas-group(string, string) -> bool;"));
        assert!(state.contains("callback ungroup-canvas-node(string);"));
        assert!(state.contains("callback remove-canvas-group-with-children(string);"));
        assert!(callbacks.contains("state.on_rename_canvas_group"));
        assert!(callbacks.contains("state.on_ungroup_canvas_node"));
        assert!(callbacks.contains("state.on_remove_canvas_group_with_children"));
        assert!(page.contains("return max(0.75, root.node-scale());"));
        assert!(page.contains("width: 34px * root.group-control-scale();"));
        assert!(page.contains("width: 20px * root.group-control-scale();"));
        assert!(page.contains("@image-url(\"../../assets/icons/edit.svg\")"));
        assert!(page.contains("@image-url(\"../../assets/icons/ungroup.svg\")"));
        assert!(page.contains("@image-url(\"../../assets/icons/trash.svg\")"));
        assert!(page.contains("AppState.canvas-group-name-dialog-open = true;"));
        assert!(page.contains("AppState.ungroup-canvas-node(root.note.id);"));
        assert!(page.contains("AppState.pending-delete-kind = \"canvas-group\";"));
        assert!(dialog.contains("init => { group-name-input.focus(); }"));
        assert!(dialog.contains("AppState.rename-canvas-group("));
        assert!(delete.contains("是否删除当前分组以及组内节点？"));
        assert!(delete
            .contains("AppState.remove-canvas-group-with-children(AppState.pending-delete-id)"));
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
        assert!(node.contains("text-editor := TextInput"));
        assert!(node.contains("&& AppState.canvas-selected-id == root.note.id;"));
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
        assert!(
            node.matches("AppState.show-canvas-node-info(root.note.id)")
                .count()
                >= 2
        );
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
        let import = core_canvas_contract_block(callbacks, "fn start_canvas_image_import(",
            "\nfn poll_canvas_image_import(");
        let persist = core_canvas_contract_block(callbacks, "fn persist_canvas_managed_image(",
            "\n#[derive(Clone, Debug)]");
        assert!(import.contains("spawn_canvas_worker"));
        assert!(import.contains("authority.read_image_source"));
        assert!(import.contains("persist_canvas_managed_image"));
        assert!(import.contains("ManagedUserArea::CanvasUploads"));
        assert!(persist.contains("authority.sync_regular"));
        assert!(persist.contains("NamespaceManagedPublication::Absent"));
        assert!(persist.contains("register_file_for_namespace"));
        assert!(persist.contains("authority.lease().namespace.path(key.area())"));
        assert!(callbacks.contains("image_path = image.path"));
        let previews = core_canvas_contract_block(sync, "pub(super) fn start_canvas_preview_effects(",
            "\n#[cfg(test)]");
        assert!(previews.contains("prepare_owned_preview(&captured, Path::new(&path), PreviewPurpose::Canvas)"));
        assert!(previews.contains("CANVAS_PREVIEW_EPOCH.load(Ordering::Acquire) != epoch"));
        assert!(previews.contains("row.id.as_str() == id && row.image_path.as_str() == path"));
        assert!(!sync.contains(
            "load_preview_image(Path::new(&note.image_path), PreviewPurpose::Canvas)"
        ));
        assert!(page.contains("root.note.kind == \"image\" || root.is-board-image()"));
        assert!(page.contains("root.note.image-path != \"\""));
        assert!(page.contains("source: root.note.preview-image"));
        assert!(page.contains("image-fit: contain"));
        assert!(page.contains("clip: true"));
        assert!(page.contains("AppState.choose-canvas-node-image(root.note.id)"));
    }

    #[test]
    fn infinite_canvas_uploaded_images_resize_proportionally_like_whiteboard_objects() {
        let page = include_str!("../../ui/pages/infinite-canvas-page.slint");
        let state = include_str!("../../ui/app-state.slint");
        let callbacks = include_str!("callbacks/infinite_canvas.rs");
        let ops = include_str!("canvas_ops.rs");

        assert!(state.contains("callback resize-canvas-image-node(string, float, float)"));
        assert!(page.contains("image-resize-handle := Rectangle"));
        assert!(page.contains("image-resize-touch := TouchArea"));
        assert!(page.contains("root.note.kind == \"image\" || root.is-board-image()"));
        assert!(page.contains("root.note.image-path != \"\""));
        assert!(page.contains("AppState.resize-canvas-image-node(root.note.id"));
        assert!(page.contains("root.resize-preview-width = self.start-width * scale"));
        assert!(page.contains("root.resize-preview-height = self.start-height * scale"));
        assert!(callbacks.contains("state.on_resize_canvas_image_node"));
        assert!(callbacks.contains("fit_image_node_to_intrinsic_aspect"));
        let imported = core_canvas_contract_block(callbacks, "fn poll_canvas_image_import(",
            "\n#[cfg(test)]");
        assert!(imported.contains("fit_image_node_to_intrinsic_aspect"));
        assert!(imported.contains("image.width, image.height"));
        let resize = core_canvas_contract_block(callbacks, "state.on_resize_canvas_image_node(",
            "state.on_prepare_canvas_focus(");
        assert!(resize.contains("apply_canvas_edit"));
        assert!(resize.contains("resize_image_node_proportionally"));
        assert!(!resize.contains("inspect_image_dimensions"));
        assert!(ops.contains("fn resize_image_node_proportionally"));
        assert!(ops.contains("fn fit_image_node_to_intrinsic_aspect"));
    }

    struct IntegratedViewerFixture {
        scoped: video_image_callbacks::tests::scoped_inputs::Fixture,
        source_path: PathBuf,
    }
    impl IntegratedViewerFixture {
        fn new(app: &AppWindow) -> Self {
            let scoped = video_image_callbacks::tests::scoped_inputs::Fixture::new();
            let transition = scoped.context.namespace_operations.try_begin_transition().unwrap();
            let recovery = transition.begin_prepublication_recovery(scoped.persistence.lease()).unwrap();
            recovery.verify_no_unsupported_imports(&scoped.authority).unwrap();
            let recovered = recovery.finish().unwrap();
            transition.prepare_publication(scoped.persistence.lease(), recovered).unwrap().publish();
            let source_path = persist_reference_image_for_namespace(&scoped.authority,
                &image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(32, 20, image::Rgba([22,44,66,255])))).unwrap();
            scoped.context.store.borrow_mut().assets.push(AssetData {
                id: "integration-original".into(), conversation_id: String::new(), title: "Original".into(),
                category: "other".into(), kind: "game".into(), time: "fixture".into(), prompt: String::new(),
                ratio: "1:1".into(), quality: "1K".into(), model: "fixture".into(), origin: "generation".into(),
                width: 32, height: 20, source_path: source_path.to_string_lossy().into_owned(), reference_paths: vec![],
                cutout_done: false, remove_black_done: false, upscale_done: false, is_new: false,
                delivery_recoverable: false, delivery_downloading: false,
            });
            wire_viewer_callbacks(app, scoped.context.clone());
            let state = app.global::<AppState>();
            state.set_session_state("online".into());
            state.set_viewer_id("integration-original".into());
            state.set_viewer_source("asset".into());
            Self { scoped, source_path }
        }
        fn assert_owned_copy(&self, path: &str) {
            assert!(self.scoped.persistence.owns_path(Path::new(path)));
            assert_ne!(Path::new(path), self.source_path);
            assert_eq!(decode_image_file(Path::new(path)).unwrap().0.to_rgba8(),
                decode_image_file(&self.source_path).unwrap().0.to_rgba8());
        }
    }
    impl Drop for IntegratedViewerFixture {
        fn drop(&mut self) {
            let lease = self.scoped.persistence.lease();
            let delivery = drain_delivery_commit_workers_for_lease_for_test(lease);
            let references = drain_activation_preview_workers_for_lease_for_test(lease);
            let previews = drain_canvas_preview_workers_for_lease_for_test(lease);
            let canvas = drain_canvas_workers_for_lease_for_test(lease);
            let retired = self.scoped.context.user_activity.begin_quiesce(lease).map(|guard| guard.retire());
            if !std::thread::panicking() {
                delivery.unwrap(); references.unwrap(); previews.unwrap(); canvas.unwrap(); retired.unwrap();
            }
        }
    }

    #[test]
    fn generated_canvas_image_reveals_a_hover_detail_action_that_opens_the_viewer() {
        use i_slint_backend_testing::ElementHandle;
        use slint::platform::{PointerEventButton, WindowEvent};

        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().expect("create app window");
        let fixture = IntegratedViewerFixture::new(&app);
        let context = fixture.scoped.context.clone();
        let source_path = fixture.source_path.clone();
        context.store.borrow_mut().canvas_notes.push(CanvasNoteData {
            id: "generated-image".into(),
            kind: "image".into(),
            content: "机械生物逐级进化".into(),
            x: 120.0,
            y: 100.0,
            width: 420.0,
            height: 280.0,
            image_path: source_path.display().to_string(),
            ..CanvasNoteData::default()
        });

        let state = app.global::<AppState>();
        state.set_logged_in(true);
        state.set_contact_popup_open(false);
        state.set_page("canvas".into());
        state.set_canvas_workflow_id("upgrade-evolution".into());
        state.set_canvas_workflow_title("升级进化".into());
        push_canvas_notes(&app, &context.store.borrow());
        app.window().set_size(slint::LogicalSize::new(1200.0, 800.0));
        app.show().expect("show app window");

        assert!(
            ElementHandle::find_by_accessible_label(&app, "查看详情")
                .next()
                .is_none(),
            "detail action must stay hidden until the generated image is hovered"
        );
        let node = ElementHandle::find_by_element_type_name(&app, "CanvasNodeCard")
            .next()
            .expect("generated canvas image node");
        let position = slint::LogicalPosition::new(
            node.absolute_position().x + node.size().width / 2.0,
            node.absolute_position().y + node.size().height / 2.0,
        );
        app.window()
            .dispatch_event(WindowEvent::PointerMoved { position });
        ElementHandle::find_by_accessible_label(&app, "查看详情")
            .next()
            .expect("hovered generated image detail action")
            .mock_single_click(PointerEventButton::Left);

        assert!(state.get_viewer_open());
        assert_eq!(state.get_viewer_source(), "canvas");
        assert_eq!(state.get_viewer_source_path(), source_path.display().to_string());
        assert_eq!(state.get_viewer_prompt(), "机械生物逐级进化");
        assert_eq!(state.get_viewer_title(), "升级进化");

    }

    #[test]
    fn infinite_canvas_image_tool_separates_plain_uploads_from_generation_nodes() {
        let page = include_str!("../../ui/pages/infinite-canvas-page.slint");
        let state = include_str!("../../ui/app-state.slint");
        let callbacks = include_str!("callbacks/infinite_canvas.rs");

        assert!(state.contains("callback add-canvas-uploaded-image(float, float)"));
        assert!(page.contains("image-insert-open"));
        assert!(page.contains("text: AppState.en ? \"Upload image\" : \"上传图片\""));
        assert!(page.contains("text: AppState.en ? \"Image node\" : \"图片节点\""));
        assert!(page.contains("AppState.add-canvas-uploaded-image"));
        assert!(page.contains("root.add-node(\"image\")"));
        assert!(page.contains("function is-board-image() -> bool"));
        assert!(page.contains("!root.is-board-image() && root.zoom-percent >= 30"));
        assert!(callbacks.contains("state.on_add_canvas_uploaded_image"));
        assert!(callbacks.contains("kind: \"board-image\".into()"));
        let upload = core_canvas_contract_block(callbacks, "state.on_add_canvas_uploaded_image(",
            "state.on_create_canvas_generation_source(");
        assert!(upload.contains("CanvasActionCapture::capture"));
        assert!(upload.contains("choose_canvas_image_for_capture"));
        assert!(upload.contains("start_canvas_image_import"));
        assert!(upload.contains("CanvasImageImportTarget::New { id, center_x, center_y }"));
    }

    #[test]
    fn infinite_canvas_pastes_external_images_and_text_at_the_viewport_center() {
        let page = include_str!("../../ui/pages/infinite-canvas-page.slint");
        let state = include_str!("../../ui/app-state.slint");
        let callbacks = include_str!("callbacks/infinite_canvas.rs");

        assert!(state.contains("callback paste-canvas-content(float, float)"));
        assert!(page.contains("AppState.paste-canvas-content(world-x / 1px, world-y / 1px)"));
        assert!(callbacks.contains("state.on_paste_canvas_content"));
        assert!(callbacks.contains("clipboard.get_image()"));
        assert!(callbacks.contains("clipboard.get_text()"));
        let paste = core_canvas_contract_block(callbacks, "state.on_paste_canvas_content(",
            "state.on_duplicate_canvas_selection(");
        assert!(paste.contains("capture.begin_effect(&store)"));
        assert!(paste.contains("read_canvas_system_clipboard()"));
        assert!(paste.contains("start_canvas_image_import"));
        assert!(paste.contains("CanvasImageImportSource::Clipboard { width, height, bytes }"));
        assert!(paste.contains("CanvasImageImportTarget::New { id, center_x, center_y }"));
        assert!(callbacks.contains("kind: \"board-image\".into()"));
        assert!(callbacks.contains("kind: \"text\".into()"));
        assert!(callbacks.contains("invoke_paste_canvas_selection(24.0, 24.0)"));
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
        assert!(page.contains(
            "AppState.generate-canvas-node(root.note.id, root.effective-prompt())"
        ));
        assert!(page.contains("已连接输入："));
        assert!(page.contains(
            "node-drag-touch.has-hover || input-connector-touch.has-hover || output-connector-touch.has-hover"
        ));
        assert!(page.contains("x: toolbar.x + toolbar.width + 10px"));
        assert!(state.contains("canvas-drag-preview-id"));
        assert!(dialog.contains("确认删除这条连接？"));
    }

    #[test]
    fn infinite_canvas_links_are_selectable_and_backspace_requests_confirmation() {
        let page = include_str!("../../ui/pages/infinite-canvas-page.slint");
        let types = include_str!("../../ui/types.slint");
        let callbacks = include_str!("callbacks/infinite_canvas.rs").replace("\r\n", "\n");
        let curve = page
            .split("component CanvasConnectionCurve")
            .nth(1)
            .and_then(|value| value.split("component CanvasNodeCard").next())
            .expect("canvas connection component");

        assert!(curve.contains("function estimated-curve-length()"));
        assert!(curve.contains("property <int> dash-count:"));
        assert!(curve.contains("property <int> hit-count:"));
        assert!(curve.contains("in property <float> flow-phase: 0;"));
        assert!(curve.contains("function flow-distance(t: float)"));
        assert!(curve.contains("root.link.flow-reversed ? 1 - root.flow-phase : root.flow-phase"));
        assert!(types.contains("flow-reversed: bool"));
        assert!(callbacks.contains("connect_nodes_with_flow("));
        assert!(callbacks
            .contains("state.on_finish_canvas_reconnect(move |target_id, x, y, tolerance|"));
        assert!(callbacks.contains("target_id.as_str(),\n                true,"));
        assert!(curve.contains("property <bool> in-sweep:"));
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
        assert!(page.contains("interval: 80ms;"));
        assert!(page.contains(
            "running: !AppState.reduced-motion && AppState.canvas-selected-link-id != \"\";"
        ));
        assert!(!page.contains("interval: 32ms;"));
        assert!(page.contains("root.link-flow-step = Math.mod(root.link-flow-step + 1, 100)"));
        assert!(page.contains("flow-phase: AppState.canvas-selected-link-id == link.id"));
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
            "if (root.is-visual-media() || root.is-board-image()) && AppState.canvas-selected-id == root.note.id && root.zoom-percent >= 30 && !root.image-processing(): media-action-bar"
        ));
        assert!(node.contains(
            "if root.is-visual-media() && (root.note.kind != \"image\" || root.note.image-path == \"\") && AppState.canvas-selected-id == root.note.id && root.zoom-percent >= 30 && !root.image-processing(): media-editor-panel"
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
        assert!(node.contains(
            "AppState.generate-canvas-node(root.note.id, root.effective-prompt())"
        ));
        assert!(!node.contains("AppState.generate()"));
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
            .split("if root.note.kind == \"video\" && !root.split-mode: HorizontalLayout")
            .nth(1)
            .and_then(|value| {
                value
                    .split("if root.note.kind != \"video\" && !root.split-mode: HorizontalLayout")
                    .next()
            })
            .expect("video action layout");
        let other_actions = media_bar
            .split("if root.note.kind != \"video\" && !root.split-mode: HorizontalLayout")
            .nth(1)
            .expect("image and audio action layout");

        assert_eq!(
            text_bar
                .matches("CanvasMediaAction { horizontal-stretch: 1;")
                .count(),
            7
        );
        for label in ["信息", "删除", "存素材", "编辑", "生图", "缩小", "放大"] {
            assert!(
                text_bar.contains(label),
                "missing text node action: {label}"
            );
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
            6
        );
        assert!(other_actions.contains("AppState.save-canvas-image(root.note.id)"));
        assert!(!text_bar.contains("CanvasMediaAction { scale-factor: root.node-scale(); width:"));
        assert!(!media_bar.contains("CanvasMediaAction { scale-factor: root.node-scale(); width:"));
    }

    #[test]
    fn infinite_canvas_media_actions_center_the_icon_and_label_as_one_group() {
        let page = include_str!("../../ui/pages/infinite-canvas-page.slint");
        let action = page
            .split("component CanvasMediaAction")
            .nth(1)
            .and_then(|value| value.split("component CanvasSplitConfirmButton").next())
            .expect("canvas media action");

        assert!(!action.contains("HorizontalLayout"));
        assert!(action.contains("property <length> action-content-width:"));
        assert!(action.contains("x: (parent.width - root.action-content-width) / 2;"));
        assert_eq!(
            action
                .matches("y: (parent.height - 16px * root.scale-factor) / 2;")
                .count(),
            1
        );
        assert!(action.contains("vertical-alignment: center;"));
        assert!(!action.contains("x: 8px * root.scale-factor;"));
        assert!(!action.contains("x: 30px * root.scale-factor;"));
    }

    #[test]
    fn infinite_canvas_split_previews_grid_and_reports_local_progress() {
        let page = include_str!("../../ui/pages/infinite-canvas-page.slint");
        let state = include_str!("../../ui/app-state.slint");
        let callbacks = include_str!("callbacks/infinite_canvas.rs");
        let node = page
            .split("component CanvasNodeCard")
            .nth(1)
            .and_then(|value| value.split("export component InfiniteCanvasPage").next())
            .expect("canvas node component");

        assert!(node.contains("AppState.canvas-split-loading-node-id == root.note.id"));
        assert!(node.contains("AppState.canvas-extraction-loading-node-id == root.note.id"));
        assert!(node.contains("root.image-processing()"));
        assert!(node.contains("LoadingDots"));
        assert!(node.contains("正在提取透明 PNG 元素"));
        assert!(page.contains("确定分割"));
        assert!(page.contains("component CanvasSplitConfirmButton"));
        assert!(node.contains("CanvasSplitConfirmButton"));
        assert!(!node.contains("平均分割"));
        assert!(state.contains("canvas-split-loading-node-id"));
        assert!(state.contains("canvas-extraction-loading-node-id"));
        assert!(state.contains("callback save-canvas-image(string)"));
        assert!(callbacks.contains("state.set_canvas_split_loading_node_id(source.id.clone().into())"));
        let compact_callbacks = callbacks.split_whitespace().collect::<String>();
        assert!(compact_callbacks.contains(
            "extract_canvas_elements_to_directory(&source_path,&output_dir,&data_root,&configured_output_root,)"
        ));
        assert!(callbacks.contains("kind: \"board-image\".to_string()"));
        assert!(callbacks.contains("正在从当前图片提取透明 PNG 元素"));
        assert!(!callbacks.contains("start_canvas_ui_extraction"));
        assert!(callbacks.contains("connect_nodes(&mut store_mut.canvas_links, &source.id, id)"));
        assert!(callbacks.contains("state.on_save_canvas_image"));
    }

    #[test]
    fn infinite_canvas_split_fields_have_steppers_and_confirm_is_text_only() {
        let page = include_str!("../../ui/pages/infinite-canvas-page.slint");
        let confirm = page
            .split("component CanvasSplitConfirmButton")
            .nth(1)
            .and_then(|value| value.split("component CanvasSplitNumberField").next())
            .expect("split confirm button");
        let number = page
            .split("component CanvasSplitNumberField")
            .nth(1)
            .and_then(|value| value.split("component CanvasMediaChip").next())
            .expect("split number field");

        assert!(number.contains("function step-value(delta: int)"));
        assert!(number.contains("step-controls := TouchArea"));
        assert!(number.contains(
            "clicked => { root.step-value(self.mouse-y < self.height / 2 ? 1 : -1); }"
        ));
        assert!(!number.contains("step-up := TouchArea"));
        assert!(!number.contains("step-down := TouchArea"));
        assert_eq!(number.matches("../../assets/icons/chevron-up-tight.svg").count(), 2);
        assert!(!number.contains("../../assets/icons/arrow-up.svg"));
        assert!(number.contains("transform-rotation: 180deg;"));
        let divider = number
            .find("split-step-horizontal-divider := Rectangle")
            .expect("horizontal step divider");
        let step_up = number
            .find("step-up-area := Rectangle")
            .expect("step up area");
        assert!(divider < step_up, "divider must render behind the arrow buttons");
        let step_up_area = number
            .split("step-up-area := Rectangle")
            .nth(1)
            .and_then(|value| value.split("step-down-area := Rectangle").next())
            .expect("step up area");
        assert!(step_up_area.contains("width: 10px * root.scale-factor;"));
        assert!(step_up_area.contains("height: 6px * root.scale-factor;"));
        assert!(step_up_area.contains("y: 2px * root.scale-factor;"));
        assert!(!confirm.contains("Image {"));
        assert!(!confirm.contains("../../assets/icons/split.svg"));
        assert!(confirm.contains("horizontal-alignment: center;"));
        assert!(confirm.contains("font-weight: 500;"));
    }

    #[test]
    fn infinite_canvas_split_action_bar_hugs_its_controls() {
        let page = include_str!("../../ui/pages/infinite-canvas-page.slint");
        let node = page
            .split("component CanvasNodeCard")
            .nth(1)
            .and_then(|value| value.split("export component InfiniteCanvasPage").next())
            .expect("canvas node component");
        let split_layout = node
            .split("if root.split-mode: HorizontalLayout")
            .nth(1)
            .and_then(|value| value.split("if root.note.kind == \"video\"").next())
            .expect("split action layout");

        assert!(node.contains("function split-action-bar-width() -> length"));
        assert!(node.contains("return 464px * root.node-scale();"));
        assert!(node.contains("root.split-mode ? root.split-action-bar-width()"));
        assert!(!split_layout.contains("horizontal-stretch: 1;"));
    }

    #[test]
    fn infinite_canvas_image_processing_feedback_stays_visible_when_zoomed_out() {
        let page = include_str!("../../ui/pages/infinite-canvas-page.slint");
        let node = page
            .split("component CanvasNodeCard")
            .nth(1)
            .and_then(|value| value.split("export component InfiniteCanvasPage").next())
            .expect("canvas node component");
        let feedback = page
            .split("processing-feedback := Rectangle")
            .nth(1)
            .and_then(|value| value.split("if root.marquee-active").next())
            .expect("canvas processing feedback");

        assert!(node.contains("processing-border-bright"));
        assert!(node.contains("root.image-processing()\n            ? 3px"));
        assert!(node.contains("max(2px, 2px * root.node-scale())"));
        assert!(node.contains("animate border-color"));
        assert!(feedback.contains("AppState.canvas-extraction-loading-node-id != \"\""));
        assert!(feedback.contains("AppState.canvas-split-loading-node-id != \"\""));
        assert!(feedback.contains("正在提取元素"));
        assert!(feedback.contains("正在分析当前图片并生成透明 PNG，请稍候"));
        assert!(feedback.contains("width: min(440px, parent.width - 32px)"));
        assert!(!feedback.contains("root.node-scale()"));
    }

    #[test]
    fn infinite_canvas_selection_outline_stays_above_image_content_at_any_zoom() {
        let page = include_str!("../../ui/pages/infinite-canvas-page.slint");
        let node = page
            .split("component CanvasNodeCard")
            .nth(1)
            .and_then(|value| value.split("export component InfiniteCanvasPage").next())
            .expect("canvas node component");
        let outline = node
            .split("selection-outline := Rectangle")
            .nth(1)
            .and_then(|value| value.split("if root.is-visual-media()").next())
            .expect("selection outline");

        assert!(node.contains(
            "return root.note.selected || AppState.canvas-selected-id == root.note.id;"
        ));
        assert!(node.contains(
            "if root.node-selected() && !root.generation-loading(): selection-outline := Rectangle"
        ));
        assert!(outline.contains("border-width: 3px"));
        assert!(outline.contains("border-color: AppTheme.accent"));
        assert!(outline.contains("z: 50"));
        assert!(!outline.contains("border-width: 3px * root.node-scale()"));
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
            "if (root.is-visual-media() || root.is-board-image()) && AppState.canvas-selected-id == root.note.id && root.zoom-percent >= 30 && !root.image-processing(): media-action-bar"
        ));
        assert!(node.contains(
            "if root.is-visual-media() && (root.note.kind != \"image\" || root.note.image-path == \"\") && AppState.canvas-selected-id == root.note.id && root.zoom-percent >= 30 && !root.image-processing(): media-editor-panel"
        ));
        assert!(node.contains("visible: root.note.kind == \"group\" && root.zoom-percent >= 30"));
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

        assert!(input_connector
            .contains("root.reconnect-started(root.note.id, root.x, root.y + root.height / 2)"));
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
        assert!(zoom_panel.contains("x: parent.width - self.width - 16px"));
        assert!(zoom_panel.contains("y: 16px"));
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
    fn pressing_space_uses_the_windows_hand_cursor_on_the_canvas() {
        use i_slint_backend_testing::{ElementHandle, TestingBackend, TestingBackendOptions};
        use slint::platform::{Key, PointerEventButton, WindowEvent};

        slint::platform::set_platform(Box::new(TestingBackend::new(TestingBackendOptions {
            mock_time: true,
            renderer_name: Some("software".into()),
            ..Default::default()
        })))
        .unwrap();
        let app = AppWindow::new().expect("create app window");
        let state = app.global::<AppState>();
        state.set_logged_in(true);
        state.set_page("canvas".into());
        state.set_contact_popup_open(false);
        app.window().set_size(slint::LogicalSize::new(1200.0, 800.0));
        app.show().expect("show app window");

        let canvas = ElementHandle::find_by_element_id(
            &app,
            "InfiniteCanvasPage::canvas-touch",
        )
        .next()
        .expect("canvas touch area");
        canvas.mock_single_click(PointerEventButton::Left);
        let position = slint::LogicalPosition::new(
            canvas.absolute_position().x + canvas.size().width / 2.0,
            canvas.absolute_position().y + canvas.size().height / 2.0,
        );
        app.window()
            .dispatch_event(WindowEvent::PointerMoved { position });
        app.window().dispatch_event(WindowEvent::KeyPressed {
            text: Key::Space.into(),
        });
        app.window()
            .dispatch_event(WindowEvent::PointerMoved { position });

        let adapter =
            slint::private_unstable_api::re_exports::WindowInner::from_pub(app.window())
                .window_adapter();
        let cursor = adapter
            .internal(i_slint_core::InternalToken)
            .and_then(|internal| {
                (internal as &dyn std::any::Any).downcast_ref::<
                    i_slint_backend_testing::testing_backend::TestingWindow,
                >()
            })
            .map(|window| format!("{:?}", window.mouse_cursor()))
            .expect("testing window adapter");
        assert_eq!(
            cursor, "Pointer",
            "Windows renders Slint's Pointer cursor as IDC_HAND; Grab maps to IDC_SIZEALL"
        );
    }

    #[test]
    fn infinite_canvas_uses_a_full_width_workspace_with_a_vertical_tool_strip() {
        let app = include_str!("../../ui/app.slint");
        let page = include_str!("../../ui/pages/infinite-canvas-page.slint");
        let toolbar = page
            .split("toolbar := Rectangle")
            .nth(1)
            .and_then(|value| value.split("if root.image-insert-open").next())
            .expect("canvas toolbar");

        assert!(app.contains(
            "AppState.page != \"video-generation\" && AppState.page != \"canvas\": Sidebar"
        ));
        assert!(toolbar.contains("x: 16px"));
        assert!(toolbar.contains("visible: !AppState.canvas-node-info-open && AppState.canvas-workflow-id == \"\""));
        assert!(toolbar.contains("y: max(16px, (parent.height - self.height) / 2)"));
        assert!(toolbar.contains("width: 44px"));
        assert!(toolbar.contains("height: 310px"));
        assert!(toolbar.contains("VerticalLayout"));
        assert!(!toolbar.contains("HorizontalLayout"));
        assert!(page.contains("x: toolbar.x + toolbar.width + 10px"));
        assert!(page.contains("toolbar.y + image-button.y"));
        assert!(page.contains("toolbar.y + appearance-button.y"));
    }

    #[test]
    fn preset_canvas_uses_two_circular_select_and_hand_modes() {
        let launcher = include_str!("../../ui/pages/free-canvas-page.slint");
        let page = include_str!("../../ui/pages/infinite-canvas-page.slint");
        let mode_button = page
            .split("component CanvasModeButton")
            .nth(1)
            .and_then(|value| value.split("component CanvasImageInsertChoice").next())
            .expect("preset canvas mode button");
        let preset_toolbar = page
            .split("preset-tool-strip := Rectangle")
            .nth(1)
            .and_then(|value| value.split("toolbar := Rectangle").next())
            .expect("preset canvas toolbar");
        let workflow_generation = page
            .split("function generate-workflow()")
            .nth(1)
            .and_then(|value| value.split("function begin-connection").next())
            .expect("workflow generation function");

        assert!(launcher.contains("AppState.canvas-tool = \"select\""));
        assert!(launcher.contains("AppState.canvas-tool = \"pan\""));
        assert!(mode_button.contains("border-radius: self.width / 2"));
        assert!(preset_toolbar.contains("visible: !AppState.canvas-node-info-open && AppState.canvas-workflow-id != \"\""));
        assert!(preset_toolbar.contains("height: 84px"));
        assert!(preset_toolbar.contains("select.svg"));
        assert!(preset_toolbar.contains("hand.svg"));
        assert!(preset_toolbar.contains("selected: AppState.canvas-tool == \"select\""));
        assert!(preset_toolbar.contains("selected: AppState.canvas-tool == \"hand\""));
        assert!(preset_toolbar.contains("AppState.canvas-tool = \"select\""));
        assert!(preset_toolbar.contains("AppState.canvas-tool = \"hand\""));
        assert!(page.contains(
            "if root.space-pan-active || root.workflow-hand-mode(): hand-pan-overlay := TouchArea"
        ));
        assert!(!workflow_generation.contains("AppState.canvas-tool = \"pan\""));
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
    fn payment_ui_uses_external_alipay_website_flow() {
        let credit_page = include_str!("../../ui/pages/credits-page.slint");
        let checkout = include_str!("../../ui/dialogs/alipay-payment-dialog.slint");
        let membership = include_str!("../../ui/components/membership-plans.slint");
        let purchase_agreements = include_str!("../../ui/components/purchase-agreements.slint");
        let callbacks = include_str!("callbacks/payment.rs");
        let payment_checkout = include_str!("payment_checkout.rs");
        let top_bar = include_str!("../../ui/components/top-bar.slint");

        assert!(credit_page.contains(
            "clicked => { AppState.recharge-credits(AppState.selected-credit-pack-code); }"
        ));
        assert!(!credit_page.contains("AppState.credit-pay-open = true"));
        assert!(membership.contains("clicked => { AppState.purchase-membership(plan.code); }"));

        assert!(checkout.contains("AppState.payment-dialog-open"));
        assert!(checkout.contains("请在浏览器中完成付款"));
        assert!(checkout.contains("每 3 秒自动检测支付结果"));
        assert!(checkout.contains("取消后可重新发起"));
        assert!(checkout.contains("AppState.retry-payment-browser()"));
        assert!(!checkout.contains("PurchaseAgreement"));
        assert!(!checkout.contains("二维码"));
        assert!(payment_checkout.contains("Command::new(\"open\")"));
        assert!(payment_checkout.contains("Command::new(\"rundll32.exe\")"));
        assert!(payment_checkout.contains("Command::new(\"xdg-open\")"));
        assert!(!payment_checkout.contains("WebViewBuilder"));
        let launch = callbacks.split_once("fn launch_payment_checkout(").unwrap().1
            .split_once("fn begin_payment_session(").unwrap().0;
        let poll = callbacks.split_once("fn poll_payment_order(").unwrap().1
            .split_once("fn continue_payment_order(").unwrap().0;
        let recovery = callbacks.split_once("fn recover_pending_orders(").unwrap().1
            .split_once("fn start_credit_order_with_billing_scope(").unwrap().0;
        let create = callbacks.split_once("fn create_payment_worker(").unwrap().1
            .split_once("fn create_upgrade_order_checked(").unwrap().0;
        assert!(launch.contains("find_saved_payment(capture,key)"));
        assert!(launch.contains("open_payment_checkout(&checkout,capture.backend.api.base_url(),&capture.persistence,&record.order_id)"));
        assert!(poll.contains("Duration::from_secs(3)"));
        assert!(recovery.contains("PaymentPoll::Initial{launch:false}"));
        assert!(callbacks.contains("暂时无法确认支付结果，请稍后查看订单状态"));
        assert!(membership.contains("PurchaseAgreements"));
        assert!(credit_page.contains("PurchaseAgreements"));
        assert!(purchase_agreements.contains("purchase-membership-accepted"));
        assert!(purchase_agreements.contains("purchase-credit-rules-accepted"));
        let agreements = create.find("accept_agreements_scoped(&acceptances,&worker.capture.session)?").unwrap();
        let credit_order = create.find("create_credit_order_billing(").unwrap();
        let membership_order = create.find("create_order_billing(").unwrap();
        assert!(agreements < credit_order);
        assert!(agreements < membership_order);
        assert!(callbacks.contains("apply_agreements_from_payment_error"));
        assert!(callbacks.contains("agreement_acceptance_required"));
        assert!(!callbacks.contains("cancel_active_payment"));
        assert!(!callbacks.contains("cancelled_payment_requests"));
        assert!(checkout.contains("clicked => { AppState.dismiss-payment(); }"));
        assert!(!callbacks.contains(
            "if started.kind == PaymentOrderKind::Membership {\n            state.set_membership_open(false);"
        ));
        assert!(top_bar.contains("查看支付状态"));

        let combined = format!("{checkout}\n{membership}\n{top_bar}");
        for removed in ["支付宝扫码支付", "关闭支付码", "加载支付二维码"] {
            assert!(
                !combined.contains(removed),
                "obsolete payment copy: {removed}"
            );
        }
    }

    #[test]
    fn credits_page_contains_recharge_redemption_and_subscription_tabs() {
        let credits = include_str!("../../ui/pages/credits-page.slint");
        let profile = include_str!("../../ui/dialogs/profile-dialog.slint");
        let app = include_str!("../../ui/app.slint");
        let state = include_str!("../../ui/app-state.slint");
        let membership = include_str!("../../ui/components/membership-plans.slint");
        let callbacks = include_str!("callbacks/credits.rs");
        let auth_callbacks = include_str!("callbacks/auth.rs");
        let account_api = include_str!("api/account.rs");

        assert!(state.contains("in-out property <string> credits-tab: \"recharge\";"));
        assert!(state.contains("credit-redemption-code"));
        assert!(state.contains("credit-redemption-message"));
        assert!(state.contains("callback redeem-credits(string);"));
        assert!(credits.contains("text: AppState.en ? \"Recharge\" : \"充值\";"));
        assert!(credits.contains("text: AppState.en ? \"Redeem\" : \"兑换码\";"));
        assert!(credits.contains("text: AppState.en ? \"Subscription\" : \"订阅\";"));
        assert!(credits.contains("active: AppState.credits-tab == \"recharge\";"));
        assert!(credits.contains("active: AppState.credits-tab == \"redeem\";"));
        assert!(credits.contains("active: AppState.credits-tab == \"membership\";"));
        assert!(credits.contains("输入兑换码"));
        assert!(credits.contains("立即兑换"));
        assert!(credits.contains("到账账号"));
        assert_eq!(
            credits
                .matches("AppState.redeem-credits(redemption-input.text)")
                .count(),
            2
        );
        assert!(!credits.contains("preview-redeem"));
        assert!(!credits.contains("INVALID"));
        assert!(!credits.contains("USED"));
        assert!(!credits.contains("成功状态预览"));
        // Structural wiring only: each assertion is bounded to its real producer.
        let wire = callbacks.split_once("fn wire_credit_callbacks(").unwrap().1
            .split_once("fn poll_credit_redemption(").unwrap().0;
        let prepare = callbacks.split_once("fn prepare_credit_redemption(").unwrap().1
            .split_once("enum CreditRedemptionSettlement").unwrap().0;
        let run = callbacks.split_once("impl PreparedCreditRedemption {").unwrap().1
            .split_once("fn prepare_credit_redemption(").unwrap().0;
        let ledger = callbacks.split_once("fn request_credit_ledger_page(").unwrap().1
            .split_once("fn poll_credit_ledger_page(").unwrap().0;
        let redeem_api = account_api.split_once("fn redeem_credit_code_billing(").unwrap().1
            .split_once("fn revoke_session(").unwrap().0;
        assert!(wire.contains("state.on_redeem_credits"));
        assert!(wire.contains("prepare_credit_redemption(&app, &context, &code)"));
        assert!(wire.contains("prepared.run(&worker_backend)"));
        assert!(prepare.contains("capture_billing_action(KnownCapability::Redeem)"));
        assert!(prepare.contains("SavedReplayRequest::redemption(persistence, &session, &existing.client_request_id)"));
        assert!(run.contains("redeem_credit_code_billing(&self.receipt.code, &self.receipt.key, scope)"));
        assert!(run.contains("backend.api.replay_saved("));
        assert!(ledger.contains("capture_billing_action(KnownCapability::ReadGroupFinance)"));
        assert!(ledger.contains("ledger_page_billing("));
        assert!(ledger.contains("&worker_scope"));
        assert!(callbacks.contains("fn reset_credit_ledger("));
        assert!(redeem_api.contains("/v1/credits/redemptions"));
        assert!(redeem_api.contains("Some(client_request_id)"));
        assert!(auth_callbacks.contains("clear_credit_redemption_state"));
        assert!(credits.contains("CreditLedgerSection"));
        assert!(credits.contains("兑换成功后，积分将自动到账你自己的主账号。"));
        assert!(credits.contains("if AppState.credits-tab == \"recharge\": CreditLedgerSection"));
        let redeem_section = credits
            .split("if AppState.credits-tab == \"redeem\": VerticalLayout {")
            .nth(1)
            .expect("redeem section")
            .split("if AppState.credits-tab == \"recharge\": VerticalLayout {")
            .next()
            .expect("redeem section end");
        assert!(redeem_section.contains("CreditRedemptionPanel"));
        assert!(redeem_section.contains("兑换说明"));
        assert!(!redeem_section.contains("CreditBalanceCard"));
        assert!(!redeem_section.contains("SectionTitle"));
        assert!(!redeem_section.contains("CreditLedgerSection"));
        assert!(credits.contains("MembershipPlans { horizontal-stretch: 1; }"));
        assert!(membership.contains("AppState.purchase-membership(plan.code)"));
        assert!(membership.contains("PurchaseAgreements"));
        assert!(profile.contains("AppState.navigate(\"credits\")"));
        assert!(profile.contains("AppState.credits-tab = \"membership\""));
        assert!(!app.contains("MembershipDialog"));
    }

    #[test]
    fn invoice_application_ui_is_hidden_until_the_server_workflow_exists() {
        let credits = include_str!("../../ui/pages/credits-page.slint");
        let app = include_str!("../../ui/app.slint");
        let state = include_str!("../../ui/app-state.slint");

        assert!(!credits.contains("申请开票"));
        assert!(!credits.contains("Apply for invoice"));
        assert!(!credits.contains("open-invoice-orders"));
        assert!(!app.contains("InvoiceOrderDialog"));
        assert!(!app.contains("InvoiceDialog"));
        assert!(!app.contains("invoice-orders-open"));
        assert!(!app.contains("invoice-open"));
        assert!(!state.contains("invoice-open"));
        assert!(!state.contains("submit-invoice-request"));
        // Keep the derived recharge-order model for backward-compatible local data while no
        // unfinished invoice dialog is reachable from the production application shell.
        assert!(state.contains("in-out property <[InvoiceOrderView]> invoice-orders: []"));
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
    fn membership_cards_keep_free_copy_and_paid_actions_aligned() {
        let membership = include_str!("../../ui/components/membership-plans.slint");

        assert!(membership.contains("text: AppState.en ? \"Free forever\" : \"永久免费\";"));
        assert!(membership.contains("height: 244px;"));
        assert!(membership.contains("if plan.code != \"free\": PillButton"));
    }

    #[test]
    fn membership_cards_disclose_expiring_grants_only_for_paid_plans() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().expect("create app window");
        let state = app.global::<AppState>();

        state.set_logged_in(true);
        state.set_page("credits".into());
        state.set_credits_tab("membership".into());
        state.set_membership_plans(slint::ModelRc::new(slint::VecModel::from(vec![
            MembershipPlanView {
                code: "free".into(),
                name: "免费版".into(),
                price: "¥0".into(),
                grant_credits: "0".into(),
                period_days: 0,
                tier_rank: 0,
            },
            MembershipPlanView {
                code: "basic".into(),
                name: "基础版".into(),
                price: "¥29".into(),
                grant_credits: "2000".into(),
                period_days: 30,
                tier_rank: 1,
            },
        ])));
        app.window().set_size(slint::LogicalSize::new(1440.0, 900.0));
        app.show().expect("show app window");

        assert_eq!(
            i_slint_backend_testing::ElementHandle::find_by_accessible_label(
                &app,
                "随本期会员有效期到期",
            )
            .count(),
            1,
        );

        state.set_language("en".into());
        assert_eq!(
            i_slint_backend_testing::ElementHandle::find_by_accessible_label(
                &app,
                "Expires with this membership period",
            )
            .count(),
            1,
        );
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

        // Verify the selected native shell in real geometry, not the dimensions
        // of the superseded design. Other pages retain their existing contracts.
        use i_slint_backend_testing::ElementHandle;
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().unwrap();
        let state = app.global::<AppState>();
        state.set_logged_in(true);
        state.set_profile_open(true);
        state.set_account_center_section("accounts-teams".into());
        app.show().unwrap();
        for (width, height) in [(1180.0, 760.0), (1364.0, 928.0), (1600.0, 1000.0)] {
            app.window().set_size(slint::LogicalSize::new(width, height));
            let root = ElementHandle::find_by_element_type_name(&app, "ProfileDialog").next().unwrap();
            let dialog = ElementHandle::find_by_element_id(&app, "ProfileDialog::dialog").next().unwrap();
            let panel = ElementHandle::find_by_element_type_name(&app, "AccountsTeamsPanel").next().unwrap();
            assert!((dialog.size().width - (root.size().width - 48.0).min(1120.0)).abs() <= 1.0);
            assert!((dialog.size().height - (root.size().height - 48.0).min(720.0)).abs() <= 1.0);
            assert!((dialog.absolute_position().x - root.absolute_position().x
                - (root.size().width - dialog.size().width) / 2.0).abs() <= 1.0);
            assert!((dialog.absolute_position().y - root.absolute_position().y
                - (root.size().height - dialog.size().height) / 2.0).abs() <= 1.0);
            assert!(panel.absolute_position().x >= dialog.absolute_position().x);
            assert!(panel.absolute_position().y >= dialog.absolute_position().y);
            assert!(panel.absolute_position().x + panel.size().width
                <= dialog.absolute_position().x + dialog.size().width + 1.0);
            assert!(panel.absolute_position().y + panel.size().height
                <= dialog.absolute_position().y + dialog.size().height + 1.0);
        }
        assert!(profile.contains(
            "viewport-height: max(self.height, AppState.account-sessions.length * 68px);"
        ));
        assert!(profile.contains("x: parent.width - 128px;"));
        assert!(profile.contains("x: parent.width - 158px;"));
        assert!(profile.contains("clip: true;"));

        assert!(auth.contains("height: min(700px, root.height - 40px);"));
        assert!(agreement_update.contains("height: min(380px, root.height - 40px);"));
        assert!(agreement_viewer.contains("width: min(860px, root.width - 32px);"));
        assert!(agreement_viewer.contains("height: parent.height - 120px;"));
        assert!(update_prompt.contains("? min(500px, root.width - 32px)"));
        assert!(update_prompt
            .contains("min(AppState.update-active ? 420px : 390px, root.height - 40px)"));
        assert!(
            settings.contains("visible: AppState.update-available || AppState.update-checking;")
        );
        assert!(!settings.contains("AppState.update-message != \"\""));

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
        assert!(card.contains("AppState.request-delete-thumbnail(root.item.id, root.source)"));
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
    fn viewer_metadata_is_four_colored_plain_text_values_in_the_top_row() {
        let viewer = include_str!("../../ui/dialogs/viewer-overlay.slint");
        let info_start = viewer
            .find("component ViewerInfoText")
            .expect("viewer info text");
        let info_end = viewer
            .find("component ViewerFooterActionButton")
            .expect("viewer footer action button");
        let info = &viewer[info_start..info_end];

        assert!(viewer.contains("component ViewerInfoText inherits Text"));
        assert!(viewer.contains("viewer-info := HorizontalLayout"));
        assert!(viewer.contains("y: 24px;"));
        assert!(viewer.contains("spacing: 8px;"));
        assert!(viewer.contains("alignment: center;"));
        assert!(viewer.contains("width: min(self.preferred-width, 180px);"));
        assert!(viewer.contains("root.detail-collapsed || root.image-fullscreen ? 0px : 460px"));
        assert_eq!(viewer.matches("ViewerInfoText {").count(), 4);
        assert!(viewer.contains("AppState.viewer-width + \"X\" + AppState.viewer-height"));
        for color in ["#24b8ff", "#42d79e", "#ffb454", "#bda4ff"] {
            assert!(viewer.contains(color), "missing viewer info color: {color}");
        }
        assert!(!info.contains("TouchArea"));
        assert!(!info.contains("background:"));
        assert!(!info.contains("border-radius:"));
        assert!(!viewer.contains("InfoCard"));
        assert!(!viewer.contains("图像信息"));
    }

    #[test]
    fn viewer_footer_exposes_the_primary_image_actions() {
        let viewer = include_str!("../../ui/dialogs/viewer-overlay.slint");
        let state = include_str!("../../ui/app-state.slint");
        let callbacks = include_str!("callbacks/viewer.rs");
        let feature = include_str!("features/viewer.rs");

        assert!(viewer.contains("component ViewerFooterActionButton"));
        assert!(viewer.contains("viewer-footer-actions := HorizontalLayout"));
        assert!(viewer.contains(
            "if AppState.viewer-source != \"reference\" && AppState.viewer-source != \"inspiration\" && !root.image-fullscreen: Rectangle"
        ));
        assert!(viewer.contains("AppState.viewer-source == \"inspiration\" ? parent.height - 96px"));
        assert_eq!(viewer.matches("ViewerFooterActionButton {").count(), 5);
        assert!(viewer.contains("AppState.viewer-download-image();"));
        assert!(viewer.contains("AppState.viewer-use-reference();"));
        assert!(viewer.contains("AppState.viewer-import-to-canvas();"));
        assert!(viewer.contains("AppState.viewer-open-image-editor();"));
        assert!(viewer.contains("AppState.viewer-generate-video();"));
        assert!(viewer.contains("AppState.request-delete-asset(AppState.viewer-id);"));
        assert!(viewer.contains("@image-url(\"../../assets/icons/download.svg\")"));
        assert!(viewer.contains("@image-url(\"../../assets/icons/edit.svg\")"));
        assert!(viewer.contains("@image-url(\"../../assets/icons/upload.svg\")"));
        assert!(viewer.contains("@image-url(\"../../assets/icons/canvas.svg\")"));
        assert!(viewer.contains("@image-url(\"../../assets/icons/trash.svg\")"));

        assert!(state.contains("callback viewer-open-image();"));
        assert!(state.contains("callback viewer-import-to-canvas();"));
        assert!(callbacks.contains("state.on_viewer_open_image"));
        assert!(callbacks.contains("state.on_viewer_import_to_canvas"));
        let import = callbacks.split_once("state.on_viewer_import_to_canvas(").unwrap().1
            .split_once("state.on_viewer_open_creation_workflow(").unwrap().0;
        let bridge = callbacks.split_once("fn start_or_retry_viewer_canvas_import(").unwrap().1
            .split_once("#[cfg(test)]").unwrap().0;
        assert!(import.contains("start_or_retry_viewer_canvas_import("));
        assert!(bridge.contains("start_captured_viewer_image_import_to_canvas_with_stage("));
        assert!(bridge.contains("retry_staged_viewer_canvas_import("));
        assert!(callbacks.contains("open_viewer_image(&app, &store.borrow())"));
        assert!(feature.contains("pub(super) fn open_viewer_image"));
        assert!(feature.contains("open_path_with_default_app(&source)"));
    }

    #[test]
    fn viewer_footer_actions_use_five_distinct_semantic_colors() {
        let viewer = include_str!("../../ui/dialogs/viewer-overlay.slint");
        let footer = viewer
            .split("viewer-footer-actions := HorizontalLayout")
            .nth(1)
            .and_then(|value| value.split("if AppState.viewer-message").next())
            .expect("viewer footer");

        for color in [
            "AppTheme.custom-prompt-name-3",
            "AppTheme.custom-prompt-name",
            "AppTheme.success",
            "AppTheme.custom-prompt-name-2",
            "AppTheme.danger",
        ] {
            assert_eq!(
                footer.matches(&format!("foreground: {color};")).count(),
                1,
                "each viewer footer action should own one distinct semantic color: {color}"
            );
        }
        assert!(viewer.contains(
            "background: action-touch.has-hover ? root.foreground.with-alpha(0.12) : root.foreground.with-alpha(0.045);"
        ));
    }

    #[test]
    fn viewer_keyboard_shortcuts_invoke_the_matching_image_actions() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().expect("create app window");
        let state = app.global::<AppState>();
        let actions = Rc::new(RefCell::new(Vec::<String>::new()));

        {
            let actions = actions.clone();
            state.on_viewer_open_image(move || actions.borrow_mut().push("open".to_string()));
        }
        {
            let actions = actions.clone();
            state.on_viewer_open_image_editor(move || {
                actions.borrow_mut().push("local-edit".to_string())
            });
        }
        {
            let actions = actions.clone();
            state.on_viewer_use_reference(move || {
                actions.borrow_mut().push("add-reference".to_string())
            });
        }
        {
            let actions = actions.clone();
            state.on_request_delete_asset(move |id| {
                actions.borrow_mut().push(format!("delete:{id}"))
            });
        }

        state.set_viewer_source("generation".into());
        state.set_viewer_repeat_enabled(true);
        state.set_viewer_id("image-42".into());
        state.set_viewer_open(true);
        app.show().expect("show app window");

        for key in ["o", "e", "a"] {
            app.window()
                .dispatch_event(slint::platform::WindowEvent::KeyPressed { text: key.into() });
            app.window()
                .dispatch_event(slint::platform::WindowEvent::KeyReleased { text: key.into() });
        }
        let delete: slint::SharedString = slint::platform::Key::Delete.into();
        app.window()
            .dispatch_event(slint::platform::WindowEvent::KeyPressed { text: delete.clone() });
        app.window()
            .dispatch_event(slint::platform::WindowEvent::KeyReleased { text: delete });

        assert_eq!(
            actions.borrow().as_slice(),
            ["open", "local-edit", "add-reference", "delete:image-42"]
        );
    }

    #[test]
    fn viewer_opens_the_image_to_video_workspace() {
        let app = include_str!("../../ui/app.slint");
        let state = include_str!("../../ui/app-state.slint");
        let viewer = include_str!("../../ui/dialogs/viewer-overlay.slint");
        let page = include_str!("../../ui/pages/video-generation-page.slint");

        assert!(app.contains("import { VideoGenerationPage }"));
        assert!(app.contains("if AppState.page == \"video-generation\": VideoGenerationPage"));
        assert!(state.contains("callback viewer-generate-video();"));
        assert!(state.contains("callback close-video-generation();"));
        assert!(state.contains("callback request-video-quote(string, string, int);"));
        assert!(state.contains("callback submit-video-generation();"));

        let footer = viewer
            .split("viewer-footer-actions := HorizontalLayout")
            .nth(1)
            .and_then(|value| value.split("if AppState.viewer-message").next())
            .expect("viewer footer");
        assert!(footer.contains("label: AppState.en ? \"Generate Video\" : \"生成视频\";"));
        assert!(footer.contains("clicked => { AppState.viewer-generate-video(); }"));

        let tools = viewer
            .split("cutout-tools-card := Rectangle")
            .nth(1)
            .and_then(|value| value.split("viewer-repeat-card := Rectangle").next())
            .expect("viewer right-side image tools");
        let repeat = viewer
            .split("viewer-repeat-card := Rectangle")
            .nth(1)
            .and_then(|value| value.split("if AppState.viewer-source == \"inspiration\"").next())
            .expect("viewer right-side repeat actions");
        assert!(tools.contains("label: AppState.en ? \"Local Edit\" : \"局部修改\";"));
        assert!(tools.contains("clicked => { AppState.viewer-open-image-editor(); }"));
        assert!(repeat.contains("label: AppState.en ? \"Use Prompt\" : \"使用提示词\";"));
        assert!(repeat.contains("clicked => { AppState.viewer-edit(); }"));

        for ratio in ["21:9", "16:9", "4:3", "1:1", "3:4", "9:16"] {
            assert!(page.contains(ratio), "missing video ratio {ratio}");
        }
        for resolution in ["480P", "720P", "1080P"] {
            assert!(page.contains(resolution), "missing video resolution {resolution}");
        }
        assert!(page.contains("max(4, min(15"));
        assert!(page.contains("AppState.video-duration-seconds - 1"));
        assert!(page.contains("AppState.video-duration-seconds + 1"));
        assert!(page.contains("text <=> AppState.video-prompt"));
        assert!(page.contains("AppState.video-credit-cost"));
        assert!(page.contains("AppState.submit-video-generation();"));
    }

    #[test]
    fn video_generation_callbacks_use_server_models_quotes_and_stable_requests() {
        let callbacks = concat!(include_str!("callbacks/video_generation.rs"), include_str!("callbacks/video_pricing.rs"));
        let runtime = include_str!("mod.rs");
        let app = include_str!("app.rs");
        let auth = include_str!("callbacks/auth.rs");
        let state = include_str!("../../ui/app-state.slint");

        assert!(runtime.contains("mod video_generation_callbacks;"));
        assert!(app.contains("wire_video_generation_callbacks(app, context.clone());"));
        assert!(auth.contains("purpose == \"video_generation\""));
        assert!(state.contains("property <string> video-model:"));
        assert!(state.contains("property <bool> video-service-available:"));

        assert!(callbacks.contains("state.on_viewer_generate_video"));
        assert!(callbacks.contains("state.set_video_source_image"));
        assert!(callbacks.contains("state.on_request_video_quote"));
        assert!(callbacks.contains("CreateVideoQuote"));
        assert!(callbacks.contains("quote_video_billing"));
        assert!(callbacks.contains("state.set_video_quote_ready(false)"));
        assert!(callbacks.contains("视频服务暂未开放"));
        assert!(callbacks.contains("state.on_submit_video_generation"));
        assert!(callbacks.contains("CreateVideoGenerationTask"));
        assert!(callbacks.contains("pending_client_request_id"));

        i_slint_backend_testing::init_no_event_loop();
        let window = AppWindow::new().unwrap();
        let context = AppContext::default();
        *context.current_user_id.lock().unwrap() = Some("user-a".into());
        wire_deferred_video_generation_callbacks(&window, context.clone());
        let state = window.global::<AppState>();
        state.set_logged_in(true);
        state.set_session_state("offline".into());
        state.set_viewer_id("image-a".into());
        state.set_viewer_prompt("original image prompt".into());
        state.set_prompt("unrelated workspace prompt".into());
        state.set_video_prompt("retained video draft".into());
        // This release deliberately does not offer video connection. A raw user
        // label without a published namespace must not mutate private UI either.
        let original_page=state.get_page();
        state.invoke_viewer_generate_video();
        assert_eq!(state.get_page(), original_page);
        assert_eq!(state.get_video_prompt(), "retained video draft");
        assert!(!state.get_video_service_available());

        store_video_prompt_draft(
            &mut context.store.borrow_mut().prompt_drafts,
            "user-a", "image-a", "edited video prompt",
        );
        state.invoke_viewer_generate_video();
        assert_eq!(state.get_video_prompt(), "retained video draft");
        assert_eq!(video_prompt_for_source(&context.store.borrow().prompt_drafts,"user-a","image-a","original image prompt"),"edited video prompt");
        assert_eq!(state.get_viewer_prompt(), "original image prompt");
        assert_eq!(state.get_prompt(), "unrelated workspace prompt");

        *context.current_user_id.lock().unwrap() = Some("user-b".into());
        state.invoke_viewer_generate_video();
        assert_eq!(state.get_video_prompt(), "retained video draft");
        assert_eq!(video_prompt_for_source(&context.store.borrow().prompt_drafts,"user-b","image-a","original image prompt"),"original image prompt");
    }

    #[test]
    fn video_player_is_local_restricted_and_uses_custom_controls() {
        let runtime = include_str!("mod.rs");
        let player = include_str!("video_player.rs");
        let html = include_str!("video_player/player.html");
        let state = include_str!("../../ui/app-state.slint");
        let page = include_str!("../../ui/pages/video-generation-page.slint");

        assert!(runtime.contains("mod video_player;"));
        assert!(player.contains("validated_local_video_url"));
        assert!(player.contains("parse_player_command"));
        assert!(player.contains("NewWindowResponse::Deny"));
        assert!(player.contains("with_navigation_handler"));
        assert!(player.contains("with_download_started_handler"));
        assert!(player.contains("build_as_child"));
        assert!(player.contains("set_bounds"));
        assert!(state.contains("callback sync-video-player(float, float, float, float);"));
        assert!(page.contains("AppState.sync-video-player("));

        for control in [
            "playButton",
            "seek",
            "timeLabel",
            "volume",
            "loopButton",
            "fullscreenButton",
            "downloadButton",
            "folderButton",
            "regenerateButton",
        ] {
            assert!(html.contains(control), "missing player control {control}");
        }
        assert!(!html.contains("controls autoplay"));
    }

    #[test]
    fn canvas_import_uses_board_image_nodes_and_focuses_selection() {
        let canvas = include_str!("callbacks/infinite_canvas.rs");
        let callbacks = include_str!("callbacks/viewer.rs");
        let page = include_str!("../../ui/pages/infinite-canvas-page.slint");
        let bridge = callbacks.split_once("fn start_or_retry_viewer_canvas_import(").unwrap().1
            .split_once("#[cfg(test)]").unwrap().0;
        let import = canvas.split_once("fn start_captured_viewer_image_import_to_canvas_with_stage(").unwrap().1
            .split_once("type ViewerCanvasImportCompletion").unwrap().0;
        let commit = canvas.split_once("fn poll_viewer_canvas_import(").unwrap().1
            .split_once("fn target_at_input(").unwrap().0;

    assert!(bridge.contains("source.path().to_owned()"));
        assert!(bridge.contains("DEFAULT_CANVAS_WORKSPACE_ID.into()"));
        assert!(import.contains("source_current(app, &context, false)"));
        assert!(import.contains("persist_canvas_managed_image("));
        assert!(import.contains("ManagedUserArea::CanvasUploads"));
        assert!(commit.contains("kind: \"board-image\".into()"));
        assert!(commit.contains("apply_canvas_edit_checked("));
        assert!(commit.contains("CanvasSaveCompletion::Viewer"));
        assert!(page.contains("running: AppState.canvas-focus-request > 0;"));
        assert!(page.contains("root.focus-selection();"));
    }

    #[test]
    fn viewer_right_click_exposes_the_relevant_creation_workflows() {
        use i_slint_backend_testing::ElementHandle;
        use slint::platform::PointerEventButton;

        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().expect("create app window");
        let state = app.global::<AppState>();
        state.set_logged_in(true);
        state.set_contact_popup_open(false);
        state.set_viewer_source("asset".into());
        state.set_viewer_category("character".into());
        state.set_viewer_width(1024);
        state.set_viewer_height(1024);
        state.set_viewer_open(true);
        let selected_workflows = Rc::new(RefCell::new(Vec::<String>::new()));
        let observed_workflows = selected_workflows.clone();
        state.on_viewer_open_creation_workflow(move |id, _, _, _| {
            observed_workflows.borrow_mut().push(id.to_string());
        });
        app.window().set_size(slint::LogicalSize::new(1200.0, 800.0));
        app.show().expect("show app window");

        let image_touch = ElementHandle::find_by_element_id(&app, "ViewerOverlay::image-touch")
            .next()
            .expect("viewer image touch area");

        for (label, expected_id) in [
            ("导入角色年龄变化", "character-age"),
            ("导入角色换装", "character-outfit"),
            ("导入角色体型修改", "character-body"),
            ("导入升级进化", "upgrade-evolution"),
            ("导入建筑衍生器", "building-derivation"),
        ] {
            image_touch.mock_single_click(PointerEventButton::Right);
            ElementHandle::find_by_accessible_label(&app, label)
                .next()
                .unwrap_or_else(|| panic!("character context menu should expose {label}"))
                .mock_single_click(PointerEventButton::Left);
            assert_eq!(
                selected_workflows.borrow().last().map(String::as_str),
                Some(expected_id)
            );
        }

        state.set_viewer_category("scene".into());
        image_touch.mock_single_click(PointerEventButton::Right);
        for label in ["导入角色年龄变化", "导入角色换装", "导入角色体型修改"] {
            assert!(
                ElementHandle::find_by_accessible_label(&app, label)
                    .next()
                    .is_none(),
                "non-character context menu must hide {label}"
            );
        }
        ElementHandle::find_by_accessible_label(&app, "导入升级进化")
            .next()
            .expect("upgrade evolution must be available for every image category")
            .mock_single_click(PointerEventButton::Left);
        assert_eq!(
            selected_workflows.borrow().last().map(String::as_str),
            Some("upgrade-evolution")
        );
    }

    #[test]
    fn character_viewer_workflow_opens_its_independent_canvas_with_the_image_as_reference() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().expect("create app window");
        let fixture = video_image_callbacks::tests::scoped_inputs::Fixture::new();
        struct Drain<'a>(&'a video_image_callbacks::tests::scoped_inputs::Fixture);
        impl Drop for Drain<'_> {
            fn drop(&mut self) {
                let lease = self.0.persistence.lease();
                let delivery = drain_delivery_commit_workers_for_lease_for_test(lease);
                let previews = drain_activation_preview_workers_for_lease_for_test(lease);
                let canvas = drain_canvas_workers_for_lease_for_test(lease);
                let retired = self.0.context.user_activity.begin_quiesce(lease).map(|guard| guard.retire());
                if !std::thread::panicking() {
                    delivery.unwrap(); previews.unwrap(); canvas.unwrap(); retired.unwrap();
                }
            }
        }
        let _drain = Drain(&fixture);
        let context = fixture.context.clone();
        let transition = context.namespace_operations.try_begin_transition().unwrap();
        let recovery = transition.begin_prepublication_recovery(fixture.persistence.lease()).unwrap();
        recovery.verify_no_unsupported_imports(&fixture.authority).unwrap();
        let recovered = recovery.finish().unwrap();
        transition.prepare_publication(fixture.persistence.lease(), recovered).unwrap().publish();
        wire_viewer_callbacks(&app, context.clone());
        let state = app.global::<AppState>();
        let authority = fixture.authority.clone();
        let source_path = std::thread::spawn(move || {
            persist_reference_image_for_namespace(&authority,
                &image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
                    32, 20, image::Rgba([22, 44, 66, 255]),
                ))).unwrap()
        }).join().unwrap();
        let original_bytes = fs::read(&source_path).unwrap();
        context.store.borrow_mut().assets.push(AssetData {
            id: "character-workflow-original".into(), conversation_id: "conversation".into(),
            title: "Character".into(), category: "character".into(), kind: "game".into(),
            time: "fixture".into(), prompt: "original character".into(), ratio: "1:1".into(),
            quality: "1K".into(), model: "fixture-model".into(), origin: "generation".into(),
            width: 32, height: 20, source_path: source_path.to_string_lossy().into_owned(),
            reference_paths: vec![], cutout_done: false, remove_black_done: false,
            upscale_done: false, is_new: false, delivery_recoverable: false, delivery_downloading: false,
        });

        state.set_logged_in(true);
        state.set_session_state("online".into());
        state.set_page("assets".into());
        state.set_viewer_id("character-workflow-original".into());
        state.set_viewer_source("asset".into());
        state.set_viewer_open(true);
        state.set_viewer_category("character".into());
        state.set_viewer_source_path(source_path.display().to_string().into());
        state.set_canvas_workflow_prompt("prompt in the previous workspace".into());
        fixture.persistence.save_store(local_store_data(&app, &context.store.borrow())).unwrap();

        state.invoke_viewer_open_creation_workflow(
            "character-age".into(),
            "角色年龄变化".into(),
            "age template {count}".into(),
            "describe the character".into(),
        );

        video_image_callbacks::tests::scoped_inputs::pump(|| state.get_page() == "canvas");
        assert_eq!(state.get_page(), "canvas");
        assert_eq!(state.get_canvas_workflow_id(), "character-age");
        assert_eq!(state.get_canvas_workflow_title(), "角色年龄变化");
        assert_eq!(state.get_canvas_workflow_template(), "age template {count}");
        assert_eq!(state.get_canvas_workflow_hint(), "describe the character");
        assert!(!state.get_viewer_open());
        let store = context.store.borrow();
        assert_eq!(store.active_canvas_workspace_id, "character-age");
        assert_eq!(store.canvas_references.len(), 1);
        let copied_path = PathBuf::from(&store.canvas_references[0].source_path);
        assert!(fixture.persistence.owns_path(&copied_path));
        assert_eq!(
            decode_reference_bytes(&fs::read(&copied_path).unwrap()).unwrap().to_rgba8(),
            decode_reference_bytes(&original_bytes).unwrap().to_rgba8(),
        );
        assert_eq!(
            store
                .canvas_workspaces
                .get(DEFAULT_CANVAS_WORKSPACE_ID)
                .expect("previous workspace saved")
                .prompt,
            "prompt in the previous workspace"
        );
        drop(store);
        let saved = fixture.writer.load_client_state_for_namespace(fixture.persistence.lease()).unwrap().unwrap();
        assert_eq!(saved.active_canvas_workspace_id, "character-age");
        assert_eq!(saved.canvas_workspaces["character-age"].references.len(), 1);
        context.store.borrow_mut().canvas_workspaces.insert(
            "character-body".to_string(),
            CanvasWorkspaceData {
                references: (0..MAX_REFERENCE_IMAGES)
                    .map(|index| ReferenceData {
                        id: format!("reference-{index}"),
                        source_path: format!("existing-{index}.png"),
                    })
                    .collect(),
                ..CanvasWorkspaceData::default()
            },
        );
        state.set_page("assets".into());
        state.set_viewer_open(true);
        state.set_viewer_category("character".into());
        state.set_viewer_source_path(source_path.display().to_string().into());
        state.invoke_viewer_open_creation_workflow(
            "character-body".into(),
            "角色体型修改器".into(),
            "body template {count}".into(),
            "describe the body".into(),
        );

        assert_eq!(state.get_page(), "assets");
        assert!(state.get_viewer_open());
        assert!(!state.get_viewer_message().is_empty());
        assert_eq!(
            context.store.borrow().active_canvas_workspace_id,
            "character-age"
        );
        assert_eq!(fs::read(&source_path).unwrap(), original_bytes);
        assert!(copied_path.is_file());
    }

    #[test]
    fn viewer_building_derivation_imports_the_current_image_into_its_workspace() {
        use i_slint_backend_testing::ElementHandle;
        use slint::platform::PointerEventButton;

        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().expect("create app window");
        let fixture = IntegratedViewerFixture::new(&app);
        let context = fixture.scoped.context.clone();
        let state = app.global::<AppState>();
        let source_path = fixture.source_path.clone();
        state.set_logged_in(true);
        state.set_contact_popup_open(false);
        state.set_page("assets".into());
        state.set_viewer_source("asset".into());
        state.set_viewer_open(true);
        state.set_viewer_category("scene".into());
        state.set_viewer_width(1024);
        state.set_viewer_height(1024);
        state.set_viewer_source_path(source_path.display().to_string().into());
        app.window().set_size(slint::LogicalSize::new(1200.0, 800.0));
        app.show().expect("show app window");
        ElementHandle::find_by_element_id(&app, "ViewerOverlay::image-touch")
            .next().expect("viewer image touch area")
            .mock_single_click(PointerEventButton::Right);
        ElementHandle::find_by_accessible_label(&app, "导入建筑衍生器")
            .next().expect("building derivation menu item")
            .mock_single_click(PointerEventButton::Left);
        video_image_callbacks::tests::scoped_inputs::pump(|| !state.get_viewer_open());
        assert_eq!(state.get_page(), "canvas");
        assert_eq!(state.get_canvas_workflow_id(), "building-derivation");
        assert_eq!(state.get_asset_type(), "scene");
        assert!(state.get_canvas_workflow_template().contains("建筑功能衍生："));
        assert!(!state.get_viewer_open());
        let store = context.store.borrow();
        assert_eq!(store.active_canvas_workspace_id, "building-derivation");
        assert_eq!(store.canvas_references.len(), 1);
        fixture.assert_owned_copy(&store.canvas_references[0].source_path);
        drop(store);

    }

    #[test]
    fn viewer_import_picker_accepts_every_workflow_for_unclassified_images() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().unwrap();
        let fixture = IntegratedViewerFixture::new(&app);
        let context = fixture.scoped.context.clone();
        let state = app.global::<AppState>();
        state.set_logged_in(true);
        let source_path = fixture.source_path.clone();
        for id in ["character-outfit", "character-age", "character-body", "plant-growth",
            "monster-generator", "upgrade-evolution", "building-derivation"] {
            state.set_viewer_id("integration-original".into());
            state.set_viewer_source("asset".into());
            state.set_page("assets".into());
            state.set_viewer_open(true);
            state.set_viewer_category("other".into());
            state.set_viewer_source_path(source_path.display().to_string().into());
            state.invoke_viewer_open_creation_workflow(id.into(), "title".into(), "template".into(), "hint".into());
            video_image_callbacks::tests::scoped_inputs::pump(|| !state.get_viewer_open());
            assert_eq!(state.get_page(), "canvas", "workflow {id}");
            assert!(!state.get_viewer_open());
            let store = context.store.borrow();
            assert_eq!(store.active_canvas_workspace_id, id);
            assert_eq!(store.canvas_references.len(), 1);
            fixture.assert_owned_copy(&store.canvas_references[0].source_path);
        }
        state.set_page("assets".into());
        state.set_viewer_open(true);
        state.set_viewer_source_path(source_path.display().to_string().into());
        state.invoke_viewer_open_creation_workflow("unknown".into(), "".into(), "".into(), "".into());
        assert_eq!(state.get_page(), "assets");
        assert!(state.get_viewer_open());
        assert_eq!(context.store.borrow().active_canvas_workspace_id, "building-derivation");

    }

    #[test]
    fn viewer_upgrade_evolution_opens_with_the_current_image_as_reference() {
        use i_slint_backend_testing::ElementHandle;
        use slint::platform::PointerEventButton;

        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().expect("create app window");
        let fixture = IntegratedViewerFixture::new(&app);
        let context = fixture.scoped.context.clone();
        let state = app.global::<AppState>();
        let source_path = fixture.source_path.clone();

        state.set_logged_in(true);
        state.set_page("assets".into());
        state.set_viewer_source("asset".into());
        state.set_viewer_open(true);
        state.set_viewer_category("scene".into());
        state.set_viewer_width(1024);
        state.set_viewer_height(1024);
        state.set_viewer_source_path(source_path.display().to_string().into());
        app.window().set_size(slint::LogicalSize::new(1200.0, 800.0));
        app.show().expect("show app window");

        ElementHandle::find_by_element_id(&app, "ViewerOverlay::image-touch")
            .next()
            .expect("viewer image touch area")
            .mock_single_click(PointerEventButton::Right);
        ElementHandle::find_by_accessible_label(&app, "导入升级进化")
            .next()
            .expect("upgrade evolution context menu item")
            .mock_single_click(PointerEventButton::Left);

        video_image_callbacks::tests::scoped_inputs::pump(|| !state.get_viewer_open());
        assert_eq!(state.get_page(), "canvas");
        assert_eq!(state.get_canvas_workflow_id(), "upgrade-evolution");
        assert_eq!(state.get_canvas_workflow_title(), "升级进化");
        assert_eq!(state.get_asset_type(), "scene");
        let template = state.get_canvas_workflow_template().to_string();
        assert!(template.contains("不得把所有主体统一处理为从小到大"));
        assert!(template.contains("若主体是人类或类人角色"));
        assert!(template.contains("主要通过服装等级、武器、装备、护甲"));
        assert!(template.contains("若主体是怪物、机械生物或其他生物"));
        assert!(template.contains("允许随等级逐步改变体型、身体比例、轮廓和形态"));
        assert!(template.contains("若主体是武器、道具、载具、植物或建筑"));
        let submitted = compose_canvas_workflow_prompt(&template, "", 8, false);
        assert!(submitted.contains("白、绿、蓝、紫、橙、红"));
        assert!(submitted.contains("等级色只能作为局部品质标识"));
        assert!(submitted.contains("不得给整个主体统一染色"));
        assert!(submitted.contains("绿光、蓝光、紫光、金光、红光"));
        assert!(submitted.contains("不得跨越主体之间的纯色背景间距"));
        assert!(!state.get_viewer_open());
        let store = context.store.borrow();
        assert_eq!(store.active_canvas_workspace_id, "upgrade-evolution");
        assert_eq!(store.canvas_references.len(), 1);
        fixture.assert_owned_copy(&store.canvas_references[0].source_path);
        drop(store);

    }

    #[test]
    fn viewer_edit_opens_the_brush_image_editor() {
        let app = include_str!("../../ui/app.slint");
        let state = include_str!("../../ui/app-state.slint");
        let viewer = include_str!("../../ui/dialogs/viewer-overlay.slint");
        let editor = include_str!("../../ui/pages/image-editor-page.slint");
        let callbacks = include_str!("callbacks/viewer.rs");

        assert!(viewer.contains("AppState.viewer-open-image-editor();"));
        assert!(state.contains("callback viewer-open-image-editor();"));
        assert!(state.contains("property <[BrushPoint]> image-editor-points"));
        assert!(state.contains("property <string> image-editor-brush-shape"));
        assert!(state.contains("property <color> image-editor-brush-color"));
        assert!(state.contains("callback submit-image-edit();"));
        assert!(app.contains("if AppState.page == \"image-editor\": ImageEditorPage"));
        assert!(editor.contains("for point in AppState.image-editor-points"));
        assert!(editor.contains("mouse-cursor: none;"));
        assert!(editor.contains("AppState.begin-image-editor-stroke("));
        assert!(editor.contains("AppState.continue-image-editor-stroke("));
        assert!(editor.contains("AppState.image-editor-brush-size = max(8, min(80"));
        assert!(editor.contains("property <[color]> brush-palette"));
        assert!(editor.contains("point.shape == \"circle\""));
        assert!(editor.contains("point.color.with-alpha"));
        assert!(editor.contains("AppState.image-editor-brush-shape = \"square\""));
        assert!(editor.contains("AppState.image-editor-brush-color = color"));
        assert!(editor.contains("text <=> AppState.image-editor-prompt"));
        assert!(editor.contains("AppState.submit-image-edit();"));
        assert!(editor.contains("text: AppState.en ? \"Close\" : \"关闭\";"));
        assert!(!editor.contains("text: AppState.en ? \"← Back\" : \"← 返回\";"));
        assert!(callbacks.contains("state.on_viewer_open_image_editor"));
        assert!(callbacks.contains("interpolated_brush_points"));
        assert!(callbacks.contains("state.on_submit_image_edit"));
        assert!(callbacks.contains("请先用笔刷标记需要修改的区域"));
        assert!(callbacks.contains("rasterize_image_edit_mask"));
        assert!(callbacks.contains("start_backend_image_edit"));
        assert!(editor.contains("AppState.image-editor-estimated-credit-cost"));
        assert!(!callbacks.contains("局部重绘服务接口待接入"));
    }

    #[test]
    fn viewer_supports_zoom_fullscreen_and_collapsing_the_detail_panel() {
        let viewer = include_str!("../../ui/dialogs/viewer-overlay.slint");

        assert!(viewer.contains("property <float> image-zoom: 1.0;"));
        assert!(viewer.contains("max(0.5, min(3.0, root.image-zoom"));
        assert!(viewer.contains("root.image-fullscreen = !root.image-fullscreen;"));
        assert!(viewer.contains("fullscreen-button := Rectangle"));
        assert!(!viewer.contains("if image-touch.has-hover: fullscreen-button"));
        assert!(viewer.contains(
            "image-touch.has-hover || fullscreen-touch.has-hover || root.image-fullscreen"
        ));
        assert!(viewer.contains("@image-url(\"../../assets/icons/fit.svg\")"));
        assert!(viewer.contains("@image-url(\"../../assets/icons/restore.svg\")"));
        assert!(viewer.contains("root.detail-collapsed = true;"));
        assert!(viewer.contains("root.detail-collapsed = false;"));
        assert!(!viewer.contains("if AppState.viewer-source != \"inspiration\": PillButton"));
        assert!(!viewer.contains("text: AppState.en ? \"Use Prompt\""));
    }

    #[test]
    fn viewer_zoomed_image_left_drag_pans_inside_the_visible_bounds() {
        let viewer = include_str!("../../ui/dialogs/viewer-overlay.slint");

        assert!(viewer.contains("property <bool> image-pan-active: false;"));
        assert!(viewer.contains("property <length> max-pan-x:"));
        assert!(viewer.contains("property <length> max-pan-y:"));
        assert!(viewer.contains("property <bool> can-pan:"));
        assert!(viewer.contains("AppState.viewer-width * 1.0 / AppState.viewer-height"));
        assert!(viewer.contains("mouse-cursor: viewer-image-stage.can-pan ? move : pointer;"));
        assert!(viewer.contains("if viewer-image-stage.can-pan"));
        assert!(viewer.contains("root.image-pan-active = true;"));
        assert!(viewer.contains("root.image-pan-start-x + self.mouse-x - root.image-pressed-x"));
        assert!(viewer.contains("root.image-pan-start-y + self.mouse-y - root.image-pressed-y"));
        assert!(viewer.contains("max(-viewer-image-stage.max-pan-x, min("));
        assert!(viewer.contains("max(-viewer-image-stage.max-pan-y, min("));
    }

    #[test]
    fn viewer_prompt_allows_read_only_partial_text_selection_and_copy() {
        let viewer = include_str!("../../ui/dialogs/viewer-overlay.slint");
        let prompt = viewer
            .split("prompt-selection-input := TextInput")
            .nth(1)
            .and_then(|value| value.split("Text { text: AppState.viewer-time").next())
            .expect("selectable viewer prompt input");

        assert!(prompt.contains("text: AppState.viewer-prompt;"));
        assert!(prompt.contains("single-line: false;"));
        assert!(prompt.contains("wrap: word-wrap;"));
        assert!(prompt.contains("read-only: true;"));
    }

    #[test]
    fn new_generation_badge_can_be_dismissed() {
        let state = include_str!("../../ui/app-state.slint");
        let card = include_str!("../../ui/components/thumbnail-card.slint");
        let callbacks = include_str!("callbacks/generation.rs");

        assert!(state.contains("callback dismiss-new-generation(string);"));
        assert!(card.contains("return root.item.is-new && root.source == \"generation\""));
        assert!(card.contains("text: \"NEW\";"));
        assert!(card.contains("AppState.dismiss-new-generation(root.item.id);"));
        assert!(callbacks.contains("state.on_dismiss_new_generation"));
    }

    #[test]
    fn custom_prompt_editor_exposes_ai_optimization() {
        let state = include_str!("../../ui/app-state.slint");
        let editor = include_str!("../../ui/pages/custom-prompt-editor-page.slint");
        let callbacks = include_str!("callbacks/generation.rs");

        assert!(state.contains("callback optimize-custom-prompt-content();"));
        assert!(editor.contains("AppState.optimize-custom-prompt-content();"));
        assert!(callbacks.contains("state.on_optimize_custom_prompt_content"));
        assert!(callbacks.contains("PromptResultTarget::CustomPrompt"));
    }

    #[test]
    fn remove_black_is_a_local_toolbox_page() {
        let app = include_str!("../../ui/app.slint");
        let state = include_str!("../../ui/app-state.slint");
        let page = include_str!("../../ui/pages/toolbox-watermark-page.slint");
        let callbacks = include_str!("callbacks/toolbox.rs");

        assert!(app.contains("AppState.page == \"toolbox-remove-black\""));
        assert!(state.contains("callback start-remove-black-tool();"));
        assert!(page.contains("in property <bool> remove-black-mode: false;"));
        assert!(page.contains("AppState.start-remove-black-tool();"));
        assert!(callbacks.contains("state.on_start_remove_black_tool"));
        assert!(callbacks.contains("remove_black_pixels"));
    }

    #[test]
    fn viewer_cutout_submits_a_recoverable_task_and_saves_an_other_asset() {
        let app = include_str!("../../ui/app.slint");
        let state = include_str!("../../ui/app-state.slint");
        let viewer = include_str!("../../ui/dialogs/viewer-overlay.slint");
        let cutout = include_str!("../../ui/pages/cutout-page.slint");
        let viewer_callbacks = include_str!("callbacks/viewer.rs");
        let callbacks = include_str!("callbacks/image_cutout.rs");
        let api = include_str!("api/generation.rs");
        let recovery = include_str!("generation/backend.rs");
        let delivery = include_str!("generation/controller.rs");
        let record = callbacks.split_once("fn new_cutout_record(").unwrap().1
            .split_once("fn resume_pending_image_cutout(").unwrap().0;
        let worker = callbacks.split_once("fn run_cutout_record(").unwrap().1
            .split_once("fn decode_cutout_result_bytes(").unwrap().0;
        let complete = callbacks.split_once("fn finish_cutout_work(").unwrap().1
            .split_once("fn cutout_worker_current(").unwrap().0;
        let stage = delivery.split_once("fn stage_namespace_delivery(").unwrap().1
            .split_once("pub(super) struct CommittedNamespaceDelivery").unwrap().0;
        let enqueue = delivery.split_once("fn enqueue_delivery(").unwrap().1
            .split_once("fn stage_namespace_delivery(").unwrap().0;

        assert!(app.contains("import { CutoutPage }"));
        assert!(app.contains("CutoutPage {"));
        assert!(state.contains("in-out property <bool> cutout-open: false;"));
        assert!(state.contains("in-out property <string> cutout-type: \"general\";"));
        assert!(state.contains("in-out property <bool> cutout-processing: false;"));
        assert!(state.contains("in-out property <int> cutout-progress: 0;"));
        assert!(state.contains("in-out property <string> cutout-result-path: \"\";"));
        assert!(state.contains("in-out property <image> cutout-result-image;"));
        assert!(state.contains("cutout-estimated-credits: \"20\""));
        assert!(state.contains("callback close-cutout();"));
        assert!(state.contains("callback submit-cutout(string);"));
        assert!(state.contains("callback reveal-cutout-result();"));
        assert!(viewer.contains("AppState.viewer-cutout-image();"));
        assert!(viewer_callbacks.contains("state.on_viewer_cutout_image"));
        assert!(viewer_callbacks.contains("state.set_viewer_open(false);"));
        assert!(viewer_callbacks.contains("state.set_cutout_open(true);"));
        assert!(viewer_callbacks.contains("state.on_close_cutout"));
        assert!(viewer_callbacks.contains("state.set_viewer_open(true);"));
        assert!(callbacks.contains("state.on_submit_cutout"));
        assert!(worker.contains("create_image_cutout_billing(&CreateImageCutout"));
        assert!(worker.contains("subject_type:record.quality.clone()"));
        assert!(worker.contains("SavedReplayRequest::generation("));
        assert!(worker.contains("prepare_namespace_cutout_delivery("));
        assert!(record.contains("task_type:\"image_cutout\".into()"));
        assert!(record.contains("model_code:\"aliyun_image_segmentation\".into()"));
        assert!(record.contains("category:\"other\".into()"));
        assert!(complete.contains("start_image_delivery_commit("));
        assert!(enqueue.contains("stage_namespace_delivery(store,&prepared,time)"));
        assert!(enqueue.contains("self.enqueue(local_store_data(app,store))"));
        assert!(stage.contains("\"image_cutout\"=>Some((\"image_cutout\",\"智能抠图\"))"));
        assert!(stage.contains("cutout_done:cutout"));
        assert!(stage.contains("store.assets.insert(0,item)"));
        assert!(stage.contains("if toolbox.is_none() {\n            reveal_prompt_history_entry(store,&item.prompt);\n            store.generations.insert(0,item.clone());"));
        assert!(!callbacks.contains("store.generations.insert"));
        assert!(api.contains("/v1/toolbox/image-cutouts"));
        assert!(api.contains("pub(crate) subject_type: String"));
        assert!(recovery.contains("resume_pending_image_cutout"));
        assert!(recovery.contains("\"image_cutout\""));

        assert!(cutout.contains("if AppState.cutout-open: Rectangle"));
        assert!(cutout.contains("page-hit-blocker := TouchArea"));
        assert!(cutout.contains("pointer-event(event) => { }"));
        assert!(cutout.contains("scroll-event(event) => { return accept; }"));
        assert!(cutout.contains("title: AppState.en ? \"Original\" : \"原图\";"));
        assert!(cutout.contains("preview: AppState.viewer-image;"));
        for (value, label) in [
            ("general", "通用"),
            ("portrait", "人像"),
            ("avatar", "头像"),
            ("skin", "皮肤"),
            ("product", "商品"),
            ("clothing", "服饰"),
            ("sky", "天空"),
        ] {
            assert!(cutout.contains(&format!("value: \"{value}\"")));
            assert!(cutout.contains(label));
        }
        assert!(cutout.contains("AppState.submit-cutout(AppState.cutout-type)"));
        assert!(cutout.contains(
            "if AppState.cutout-message != \"\" && AppState.cutout-result-path == \"\": Text",
        ));
        assert!(!callbacks.contains("当前仅完成前端界面"));
    }

    #[test]
    fn viewer_groups_local_edit_with_image_tools_and_keeps_use_prompt() {
        let viewer = include_str!("../../ui/dialogs/viewer-overlay.slint");
        let tools = viewer
            .split("cutout-tools-card := Rectangle")
            .nth(1)
            .and_then(|value| value.split("viewer-repeat-card := Rectangle").next())
            .expect("viewer processing tools card");
        let repeat = viewer
            .split("viewer-repeat-card := Rectangle")
            .nth(1)
            .and_then(|value| {
                value
                    .split("if AppState.viewer-source == \"inspiration\"")
                    .next()
            })
            .expect("viewer repeat card");
        let prompt_scroll_index = viewer
            .find("prompt-scroll := ScrollView")
            .expect("viewer prompt scroll");
        let tools_index = viewer
            .find("cutout-tools-card := Rectangle")
            .expect("viewer processing tools");
        let repeat_index = viewer
            .find("viewer-repeat-card := Rectangle")
            .expect("viewer repeat tools");

        assert!(viewer.contains("component ViewerToolActionButton inherits Rectangle"));
        assert!(viewer.contains("prompt-scroll := ScrollView {"));
        assert!(viewer.contains("vertical-stretch: 1;"));
        assert!(!viewer.contains(
            "height: min(360px, max(24px, (AppState.viewer-prompt-lines > 20 ? 20 : AppState.viewer-prompt-lines) * 18px));"
        ));
        assert!(!viewer.contains("Rectangle { vertical-stretch: 1; }"));
        assert!(prompt_scroll_index < tools_index);
        assert!(tools_index < repeat_index);
        assert!(tools.contains("HorizontalLayout"));
        assert!(!tools.contains("GridLayout"));
        assert!(!tools.contains("Row {"));
        assert_eq!(tools.matches("ViewerToolActionButton {").count(), 3);
        assert_eq!(tools.matches("horizontal-stretch: 1;").count(), 3);
        assert!(tools.contains("label: AppState.en ? \"Cutout\" : \"抠图\""));
        assert!(!tools.contains("label: AppState.en ? \"Remove Black\" : \"去黑\""));
        assert!(tools.contains("label: AppState.en ? \"Clear Upscale\" : \"清晰放大\""));
        assert!(tools.contains("@image-url(\"../../assets/icons/fit.svg\")"));
        assert!(!tools.contains("@image-url(\"../../assets/icons/palette.svg\")"));
        assert!(tools.contains("@image-url(\"../../assets/icons/focus.svg\")"));
        assert!(tools.contains("label: AppState.en ? \"Local Edit\" : \"局部修改\""));
        assert!(tools.contains("clicked => { AppState.viewer-open-image-editor(); }"));
        assert!(tools.contains("@image-url(\"../../assets/icons/edit.svg\")"));
        assert!(repeat.contains("HorizontalLayout"));
        assert_eq!(repeat.matches("ViewerToolActionButton {").count(), 2);
        assert!(repeat.contains("label: AppState.en ? \"Use Prompt\" : \"使用提示词\""));
        assert!(repeat.contains("clicked => { AppState.viewer-edit(); }"));
        assert!(repeat.contains("label: AppState.en ? \"Generate Again\" : \"再次生成\""));
        assert!(repeat.contains("@image-url(\"../../assets/icons/edit.svg\")"));
        assert!(repeat.contains("@image-url(\"../../assets/icons/redo.svg\")"));
    }

    #[test]
    fn viewer_image_can_start_a_native_file_drag() {
        let viewer = include_str!("../../ui/dialogs/viewer-overlay.slint");
        let state = include_str!("../../ui/app-state.slint");
        let callbacks = include_str!("callbacks/viewer.rs");
        let references = include_str!("callbacks/reference.rs");
        let viewer_drag = callbacks.split_once("state.on_start_viewer_file_drag(").unwrap().1
            .split_once("state.on_viewer_cutout_image(").unwrap().0;
        let preparation = references.split_once("fn start_reference_native_drag(").unwrap().1
            .split_once("fn wire_reference_callbacks(").unwrap().0;
        let dispatch = references.split_once("fn reference_native_file_drag(").unwrap().1
            .split_once("fn reference_pointer_exit(").unwrap().0;

        assert!(state.contains("callback start-viewer-file-drag() -> bool;"));
        assert!(viewer.contains("property <bool> image-drag-armed: false;"));
        assert!(viewer.contains("AppState.start-viewer-file-drag();"));
        assert!(viewer.contains("if viewer-image-stage.can-pan"));
        assert!(viewer.contains("root.image-drag-armed = false;"));
        assert!(viewer.contains("root.image-drag-armed = true;"));
        let native_drag = viewer
            .find("AppState.start-viewer-file-drag();")
            .expect("viewer native drag call");
        let cleanup = &viewer[native_drag..];
        assert!(cleanup.contains("root.image-drag-armed = false;"));
        assert!(cleanup.contains("root.image-system-drag-started = false;"));
        assert!(callbacks.contains("state.on_start_viewer_file_drag"));
        assert!(viewer_drag.contains("viewer_item(&store.borrow(), &id, &source)"));
        assert!(viewer_drag.contains("state.invoke_start_thumbnail_file_drag("));
        assert!(preparation.contains("ReferenceCapture::native(app,&context)"));
        assert!(preparation.contains("prepare_native_file_drag_source(persistence,&path)"));
        assert!(preparation.contains("bind_native_file_drag(context,source)"));
        assert!(preparation.contains("drag.with_presentation_check("));
        assert!(preparation.contains("reference_native_file_drag(drag)"));
        assert!(dispatch.contains("drag_preview::start_thumbnail_file_drag_captured(drag)"));
    }

    #[test]
    fn viewer_remove_black_matches_the_unmult_reference_algorithm() {
        let mut pixels = vec![
            0, 0, 0, 255, 64, 32, 16, 255, 128, 128, 128, 128, 255, 255, 255, 255,
        ];

        remove_black_pixels(&mut pixels);

        assert_eq!(&pixels[0..4], &[0, 0, 0, 0]);
        assert_eq!(&pixels[4..8], &[255, 128, 64, 63]);
        assert_eq!(&pixels[8..12], &[255, 255, 255, 64]);
        assert_eq!(&pixels[12..16], &[255, 255, 255, 254]);
    }

    #[test]
    fn viewer_remove_black_preserves_hue_ratios_and_existing_transparency() {
        let mut pixel = vec![50, 100, 200, 64];

        remove_black_pixels(&mut pixel);

        assert_eq!(pixel, vec![64, 128, 255, 50]);
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
    fn collapsed_workspace_button_opens_a_category_popup_to_the_right() {
        let workspace = include_str!("../../ui/components/category-workspace-menu.slint");

        assert!(workspace.contains("collapsed-popup := PopupWindow"));
        assert!(workspace.contains("x: root.width + 8px;"));
        assert!(workspace.contains("close-policy: close-on-click-outside;"));
        assert!(workspace.contains("if root.collapsed"));
        assert!(workspace.contains("collapsed-popup.show();"));
        assert_eq!(
            workspace
                .matches("picked => { collapsed-popup.close(); }")
                .count(),
            4
        );
        for category in ["character", "scene", "ui", "effect"] {
            assert!(workspace.contains(&format!("category: \"{category}\";")));
        }
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
        assert!(card.contains("border-radius: AppState.card-style == \"rounded\" ? 10px : 0px;"));
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
        assert!(card.contains("visible: failed-hover.has-hover || failed-delete-touch.has-hover"));
        assert!(card.contains("AppState.request-delete-thumbnail(root.item.id, \"generation\")"));
        assert!(card.contains("visible: root.item.source-path != \"failed\";"));
        assert!(callbacks.contains("take_pending_store_record"));
        assert!(callbacks.contains("asset_collection_mut"));
    }

    #[test]
    fn renderer_prefers_gpu_and_keeps_software_fallback() {
        let app = include_str!("app.rs");
        let manifest = include_str!("../../Cargo.toml");
        let app_state = include_str!("../../ui/app-state.slint");
        assert!(!app.contains("set_var(\"SLINT_BACKEND\""));
        assert!(!app.contains("set_rendering_notifier"));
        assert!(app.contains("backend.contains(\"software\")"));
        assert!(app.contains("set_reduced_motion(using_software_renderer)"));
        assert!(app_state.contains("in-out property <bool> reduced-motion: false"));
        assert!(manifest.contains("\"renderer-femtovg\""));
        assert!(manifest.contains("\"renderer-software\""));
    }

    #[test]
    fn recovered_pending_payment_does_not_launch_the_browser_automatically() {
        let callbacks = include_str!("callbacks/payment.rs");
        let recovery = callbacks.split_once("fn recover_pending_orders(").unwrap().1
            .split_once("fn start_credit_order_with_billing_scope(").unwrap().0;
        let result = callbacks.split_once("fn poll_payment_result(").unwrap().1
            .split_once("fn poll_payment_order(").unwrap().0;
        let continuation = callbacks.split_once("fn continue_payment_order(").unwrap().1
            .split_once("fn wire_payment_callbacks(").unwrap().0;
        assert!(recovery.contains("recover_pending_order_worker(worker,record,kind,worker_presentation)"));
        assert!(recovery.contains("PaymentPoll::Initial{launch:false},receiver)"));
        assert!(result.contains("continue_payment_order(&app,context,capture,started,poll)"));
        assert!(!recovery.contains("open_payment_checkout("));
        assert!(!recovery.contains("schedule_payment_checkout("));
        assert!(continuation.contains("if matches!(poll,PaymentPoll::Initial{launch:true}) && checkout.is_some(){"));
        assert!(continuation.contains("schedule_payment_checkout("));
        assert!(continuation.contains("已恢复未完成订单，可重新打开支付宝继续支付"));
    }

    #[test]
    fn prompt_model_fallback_uses_gpt_5_6_sol_as_the_default() {
        let auth = include_str!("callbacks/auth.rs");
        // Both publication and refresh retain the same saved -> GPT-5.6 -> first order.
        for (start, end) in [
            ("fn prepare_activation_catalog_projection(", "fn prepare_activation_image_model("),
            ("fn apply_model_catalog_projection(", "fn model_group("),
        ] {
            let producer = auth.split_once(start).unwrap().1.split_once(end).unwrap().0;
            let selection = producer.split_once("let selected_prompt = available_models").unwrap().1
                .split_once("let selected_video_code").unwrap().0;
            let saved_selection = selection.find("item.code == selected_prompt_code").unwrap();
            let preferred_fallback = selection.find("item.code == \"gpt_5_6_sol\"").unwrap();
            let generic_fallback = selection.rfind("item.purpose == \"prompt_processing\"").unwrap();
            assert!(saved_selection < preferred_fallback, "{start}");
            assert!(preferred_fallback < generic_fallback, "{start}");
        }
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
    fn billing_rejection_generation_opens_contextual_dialog_without_failed_record() {
        let backend = include_str!("generation/backend.rs");
        let poll = include_str!("generation/poll.rs");
        let model = include_str!("model.rs");
        let dialog = include_str!("../../ui/dialogs/credit-insufficient-dialog.slint");
        let api_error = include_str!("api/error.rs");
        // Deliberately inspect the ordinary starter, not a D/E or retained-worker branch.
        let submission = backend.split_once("pub(super) fn start_backend_generation_with_billing_scope(").unwrap().1
            .split_once("pub(super) fn start_backend_image_edit(").unwrap().0;
        let rejected = submission.split_once("if error.is_billing_rejection() {").unwrap().1
            .split_once("if !error.should_preserve_generation_recovery()").unwrap().0;

        assert!(api_error.contains("is_insufficient_credits"));
        assert!(model.contains("CreditInsufficient"));
        assert!(rejected.contains("remove_pending_generation_for_namespace(&authority, &recovery_identity)"));
        assert!(rejected.contains("GenerationOutcome::CreditInsufficient"));
        assert!(rejected.contains("return;"));
        assert!(!rejected.contains("GenerationOutcome::Failure"));
        let credit_branch = poll
            .split_once("GenerationOutcome::CreditInsufficient { message } => {").unwrap().1
            .split_once("GenerationOutcome::Failure { reason, time } => {").unwrap().0;
        assert!(credit_branch.contains("remove_active_generation("));
        assert!(!credit_branch.contains("add_stream_failure_item("));
        assert!(credit_branch.contains("show_credit_rejection(&state, &message)"));
        assert!(credit_branch.contains("restore_stream_inputs("));
        assert!(credit_branch.contains("remove_conversation_placeholder(&state, &conversation_id)"));
        assert!(!credit_branch.contains("finish_conversation_placeholder(&state, &conversation_id"));
        assert!(dialog.contains("积分不足"));
        assert!(dialog.contains("前往充值"));
        assert!(dialog.contains("AppState.navigate(\"credits\")"));
        // Structure only: this does not prove that a failed recovery-row removal is acknowledged.
    }

    #[test]
    fn generation_terminal_scope_guard_clears_every_busy_surface() {
        let generation_state = include_str!("generation/state.rs");
        let generation_poll = include_str!("generation/poll.rs");
        let cutout = include_str!("callbacks/image_cutout.rs");
        let enhancement = include_str!("callbacks/image_enhancement.rs");
        let toolbox = include_str!("callbacks/toolbox.rs");
        let auth = include_str!("callbacks/auth.rs");
        let transition = include_str!("account_transition.rs");
        let guard = generation_state.split_once("pub(super) fn generation_scope_allows_polling(").unwrap().1
            .split_once("pub(super) fn observe_detached_generation_scope(").unwrap().0;
        let reset = generation_state.split_once("pub(super) fn clear_generation_account_state(").unwrap().1
            .split_once("pub(super) fn insert_active_generation(").unwrap().0;
        assert!(guard.contains("GenerationScopeDisposition::CapturedTerminal"));
        assert!(guard.contains("terminal_auth_scope_matches_context(context, session_scope)"));
        assert!(guard.contains("sign_out_locally(&app, context, true, Some(session_scope.auth_epoch))"));
        for setter in [
            "state.set_generating(false)",
            "state.set_generation_loading_count(0)",
            "state.set_image_editor_generating(false)",
            "state.set_viewer_processing(false)",
            "state.set_cutout_processing(false)",
            "state.set_cutout_progress(0)",
            "state.set_enhance_processing(false)",
            "state.set_enhance_progress(0)",
            "state.set_watermark_processing(false)",
            "state.set_watermark_progress(0)",
            "state.set_colorize_processing(false)",
            "state.set_colorize_progress(0)",
        ] {
            assert!(reset.contains(setter), "missing reset {setter}");
        }
        let logout = auth.split_once("pub(super) fn sign_out_locally(").unwrap().1
            .split_once("pub(super) fn require_online_operation(").unwrap().0;
        let retired = transition.split_once("fn clear_retired_private_state(").unwrap().1
            .split_once("/// Captured at model preparation").unwrap().0;
        assert!(logout.contains("coordinator.logout(app, context.clone(), scope, false)"));
        assert!(logout.contains("clear_generation_account_state(app, context, generation_teardown_scope.as_ref())"));
        assert!(retired.contains("clear_generation_account_state(app, context, None)"));
        let poll = generation_poll.split_once("pub(super) fn poll_generation_stream(").unwrap().1
            .split_once("pub(super) fn acknowledge_delivery_after_local_save(").unwrap().0;
        assert!(poll.contains("generation_scope_allows_polling(&app_weak, &context, &session_scope)"));

        // Migrated tool workers join before consuming terminal errors; they do not
        // use the old session-only poll guard to dispatch their private completion.
        for (source, poll_start, error_start) in [
            (cutout, "fn poll_cutout_work<R:Send+'static>(", "fn cutout_error("),
            (enhancement, "fn poll_enhancement_work<R:Send+'static>(", "fn enhancement_error("),
        ] {
            let poll = source.split_once(poll_start).unwrap().1.split_once(error_start).unwrap().0;
            let error = source.split_once(error_start).unwrap().1.split_once("capture.apply(app,").unwrap().0;
            assert!(poll.contains("if !capture.current()"));
            assert!(poll.contains("is_terminal_session_error()"));
            assert!(poll.contains("if capture.current(){complete(&app,&capture,result);}"));
            assert!(poll.find("_worker_pending(id)").unwrap() < poll.find("receiver.try_recv()").unwrap());
            assert!(error.contains("capture.binding_matches() && terminal_auth_scope_matches_context(&capture.context,&capture.session)"));
            assert!(error.contains("sign_out_locally(app,&capture.context,true,Some(capture.session.auth_epoch))"));
        }
        // The two captured remote-tool consumers now finish the terminal message,
        // then check session and original binding before any AppState projection.
        for start in ["fn poll_watermark_outcomes(", "fn poll_image_colorization_outcomes("] {
            let poll = toolbox.split_once(start).unwrap().1
                .split_once("let state = app.global::<AppState>();").unwrap().0;
            assert!(poll.contains("rx.finish_message(outcome)"));
            assert!(poll.contains("generation_scope_allows_polling(&app_weak, &context, &session_scope)"));
            assert!(poll.contains("toolbox_binding_is_current(&context.store, &persistence)"));
            assert!(poll.find("rx.finish_message(outcome)").unwrap()
                < poll.find("generation_scope_allows_polling(").unwrap());
        }
    }

    #[test]
    fn detached_generation_ack_and_cancel_observe_terminal_scope() {
        let generation_state = include_str!("generation/state.rs");
        let generation_poll = include_str!("generation/poll.rs");
        let generation_controller = include_str!("generation/controller.rs");
        let backend = include_str!("generation/backend.rs");
        let observer = generation_state.split_once("pub(super) fn observe_detached_generation_scope(").unwrap().1
            .split_once("pub(super) fn clear_generation_account_state(").unwrap().0;
        assert!(observer.contains("generation_scope_allows_polling(&app_weak, &context, &session_scope)"));
        assert!(observer.find("generation_scope_allows_polling(").unwrap()
            < observer.find("rx.try_recv()").unwrap());
        let ack = generation_poll.split_once("pub(super) fn acknowledge_delivery_after_local_save(").unwrap().1
            .split_once("#[cfg(test)]").unwrap().0;
        assert!(ack.contains("acknowledge_delivery_scoped("));
        assert!(ack.contains("&worker_scope"));
        assert!(ack.contains("observe_detached_generation_scope("));
        assert!(ack.contains("pending_delivery_acknowledged("));
        let cancel = generation_controller.split_once("pub(super) fn stop_generation(").unwrap().1
            .split_once("pub(super) fn add_stream_success_item(").unwrap().0;
        assert!(cancel.contains("GenerationRecoveryPatch::RequestCancellation"));
        assert!(cancel.contains("context.namespace_for(&task.session_scope)"));
        assert!(cancel.contains("cleanup_cancelled_generation(&backend, &authority, &api, &worker_scope, &key, &[], Some(&server_task_id), &cancellations)"));
        assert!(cancel.contains("observe_detached_generation_scope("));
        let cleanup = backend.split_once("pub(super) fn cleanup_cancelled_generation(").unwrap().1
            .split_once("fn cleanup_image_edit_input_path(").unwrap().0;
        assert!(cleanup.contains("api.cancel_scoped(task_id, session_scope)?"));
        assert!(cleanup.contains("require_saved_group(&row.billing_account_group_id, &before.billing_account_group_id)?"));
        assert!(cleanup.contains("require_saved_group(&row.billing_account_group_id, &after.billing_account_group_id)?"));
        // This preserves the legacy detached observer/delegation contract only.
        // It does not certify joined lifecycle or typed cancellation-error propagation.
    }

    #[test]
    fn generation_recovery_discovery_observes_terminal_scope() {
        let backend = include_str!("generation/backend.rs");
        // Historical test name retained. Server-only tasks absent from this device
        // are explicitly deferred; the current contract discovers existing local
        // namespace rows, preserves blocked records, and observes terminal scope.
        let discovery = backend.split_once("pub(super) fn recover_pending_generations(").unwrap().1
            .split_once("fn reconcile_recoverable_delivery_cards(").unwrap().0;
        assert!(discovery.contains("context.current_account_session_scope()"));
        assert!(discovery.contains("context.namespace_for(&scope)"));
        assert!(discovery.contains("context.storage_authority_for(&lease)"));
        assert!(discovery.contains("load_pending_generations_for_namespace(&authority)"));
        assert!(discovery.contains("| \"image_to_video\""));
        assert!(discovery.contains("activity.is_quiescing() || !backend.api.user_work_is_current(&worker_scope)"));
        assert!(discovery.contains("bind_generation_recovery_candidate(&backend, &authority, &worker_scope, record)"));
        assert!(discovery.contains("blocked += 1"));
        assert!(discovery.contains("sender.send(result)"));
        assert!(discovery.contains("poll_server_generation_recovery(app.as_weak(), context, scope,"));
        assert!(!discovery.contains("recover_server_generation_tasks("));
        assert!(!discovery.contains("list_tasks_scoped("));
        assert!(!discovery.contains("remove_pending_generation"));
        let poll = backend.split_once("fn poll_server_generation_recovery(").unwrap().1
            .split_once("fn resume_pending_generation(").unwrap().0;
        let read = poll.find("rx.try_recv()").unwrap();
        let first_guard = poll.find("generation_scope_allows_polling(&app_weak, &context, &session_scope)").unwrap();
        let last_guard = poll.rfind("generation_scope_allows_polling(&app_weak, &context, &session_scope)").unwrap();
        assert!(first_guard < read && read < last_guard);
        assert!(poll.contains("原记录已保留"));
        assert!(poll.contains("原付款账号和记录已保留；不会切换付款账号重试"));
        assert!(poll.contains("resume_pending_generation(&app, context.clone(), record)"));
    }

    #[test]
    fn delivery_failure_state_is_exposed_to_asset_cards() {
        let types = include_str!("../../ui/types.slint");
        let sync = include_str!("presentation/sync.rs");

        assert!(types.contains("delivery-recoverable: bool"));
        assert!(types.contains("delivery-downloading: bool"));
        assert!(sync.contains("delivery_recoverable: asset.delivery_recoverable"));
        assert!(sync.contains("delivery_downloading: asset.delivery_downloading"));
    }

    #[test]
    fn recoverable_failure_card_has_independent_download_action() {
        let card = include_str!("../../ui/components/thumbnail-card.slint");
        let state = include_str!("../../ui/app-state.slint");

        assert!(state.contains("callback retry-generation-delivery(string);"));
        assert!(card.contains("root.item.delivery-recoverable"));
        assert!(card.contains("../../assets/icons/download.svg"));
        assert!(card.contains("AppState.retry-generation-delivery(root.item.id)"));
        assert!(card.contains("root.item.delivery-downloading"));
        assert!(card.contains("图片已生成，下载失败"));
        assert!(card.contains("x: root.item.delivery-recoverable ? parent.width - 76px : parent.width - 38px;"));
    }

    #[test]
    fn repeated_recoverable_delivery_failures_replace_the_existing_card() {
        let failed_asset_id = "failed-delivery-asset";
        let mut cards = Vec::new();

        upsert_stream_failure_card(
            &mut cards,
            delivery_failure_card(failed_asset_id, "First delivery failure"),
        );
        upsert_stream_failure_card(
            &mut cards,
            delivery_failure_card(failed_asset_id, "Repeated delivery failure"),
        );

        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].id, failed_asset_id);
        assert!(cards[0].delivery_recoverable);
        assert_eq!(cards[0].title, "Repeated delivery failure");
    }

    #[test]
    fn automatic_replacement_failure_retains_original_recoverable_card_and_notification() {
        let failed_asset_id = "failed-delivery-asset";
        let mut store = Store::default();
        let mut original_card = delivery_failure_card(failed_asset_id, "Original failure");
        original_card.delivery_downloading = true;
        store.generations.push(original_card);
        store.notifications.push(NotificationData {
            id: "original-notification".to_string(),
            title: "Generation failed".to_string(),
            model: "model".to_string(),
            time: "2026-08-27 00:00:00".to_string(),
            reason: "initial delivery failure".to_string(),
            success: false,
            read: false,
        });

        assert!(retain_failed_delivery_after_replacement_failure(
            &mut store,
            failed_asset_id,
        ));

        assert_eq!(store.generations.len(), 1);
        assert_eq!(store.generations[0].id, failed_asset_id);
        assert_eq!(store.generations[0].title, "Original failure");
        assert_eq!(store.generations[0].source_path, "failed");
        assert!(store.generations[0].delivery_recoverable);
        assert!(!store.generations[0].delivery_downloading);
        assert_eq!(store.notifications.len(), 1);
        assert_eq!(store.notifications[0].id, "original-notification");
    }

    fn delivery_failure_card(id: &str, title: &str) -> AssetData {
        AssetData {
            id: id.to_string(),
            conversation_id: "conversation".to_string(),
            title: title.to_string(),
            category: "scene".to_string(),
            kind: "generate".to_string(),
            time: "2026-08-27 00:00:00".to_string(),
            prompt: "A recoverable delivery failure".to_string(),
            ratio: "1:1".to_string(),
            quality: "1K".to_string(),
            model: "model".to_string(),
            origin: "backend".to_string(),
            width: 0,
            height: 0,
            source_path: "failed".to_string(),
            reference_paths: Vec::new(),
            cutout_done: false,
            remove_black_done: false,
            upscale_done: false,
            is_new: false,
            delivery_recoverable: true,
            delivery_downloading: false,
        }
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

    #[test]
    fn generation_keeps_reference_thumbnails_after_submission() {
        let backend = include_str!("generation/backend.rs");
        let submission = backend
            .split_once("pub(super) fn start_backend_generation_with_billing_scope(").unwrap().1
            .split_once("pub(super) fn start_backend_image_edit(").unwrap().0;
        let captured = submission.split_once("let original_references = {").unwrap().1
            .split_once("let reference_paths = original_references").unwrap().0;

        assert!(captured.contains("GenerationDestination::Canvas { .. } => &store.canvas_references"));
        assert!(captured.contains("references_for_category(&store.references, &category)"));
        assert!(captured.contains(".cloned()"));
        assert!(!submission.contains("references_for_category_mut"));
        assert!(!submission.contains("push_references(app"));
        assert!(!submission.contains("canvas_references.clear()"));
        // No actual thumbnail submission fixture is implied by these source checks.
    }

    #[test]
    fn contact_details_are_available_on_first_launch_and_in_settings() {
        let state = include_str!("../../ui/app-state.slint");
        let app = include_str!("../../ui/app.slint");
        let popup = include_str!("../../ui/dialogs/contact-popup.slint");
        let settings = include_str!("../../ui/pages/settings-page.slint");
        let callbacks = include_str!("callbacks/contact.rs");

        assert!(state.contains("contact-popup-open: true"));
        assert!(state.contains("callback dismiss-contact-popup();"));
        assert!(state.contains("callback open-contact-settings();"));
        assert!(state.contains("callback copy-contact-detail(string);"));
        assert!(state.contains("contact-copy-toast-visible"));
        assert!(state.contains("contact-copy-sequence"));
        assert!(app.contains("if AppState.contact-popup-open: ContactPopup"));
        for detail in ["1090665775", "dyx346", "business@honeykid.cn"] {
            assert!(popup.contains(detail));
            assert!(settings.contains(detail));
        }
        assert!(settings.contains("AppState.settings-section = \"contact\""));
        assert!(callbacks.contains("store_mut.contact_popup_dismissed = true"));
        assert!(callbacks.contains("state.set_settings_section(\"contact\".into())"));
        assert!(callbacks.contains("state.on_copy_contact_detail"));
        assert!(callbacks.contains("clipboard.set_text(value.to_owned())"));
        assert!(callbacks.contains("state.set_contact_copy_toast_visible(true)"));
        assert!(callbacks.contains("Duration::from_millis(1400)"));
        assert!(callbacks.contains("state.get_contact_copy_sequence() == sequence"));
        assert!(popup.contains("AppState.copy-contact-detail(root.value)"));
        assert_eq!(settings.matches("AppState.copy-contact-detail(").count(), 3);
        assert!(app.contains("if AppState.contact-copy-toast-visible: Rectangle"));
        assert!(app.contains("AppState.en ? \"Copied\" : \"已复制\""));
        assert!(!popup.contains("AppState.contact-copied-value"));
        assert!(!settings.contains("AppState.contact-copied-value"));
        assert!(callbacks.contains("save_local_store(app, &store_mut)"));
    }

    #[test]
    fn about_page_recommends_related_products_with_trusted_external_links() {
        let state = include_str!("../../ui/app-state.slint");
        let settings = include_str!("../../ui/pages/settings-page.slint");
        let app = include_str!("app.rs");

        assert!(state.contains("callback open-external-link(string);"));
        assert!(settings.contains("你可能喜欢"));
        assert!(settings.contains("声音分离"));
        assert!(settings.contains("从视频完整分离音频"));
        assert!(settings.contains("https://www.shineway.tech/biyi/feature/audio"));
        assert!(settings.contains("言外之意"));
        assert!(settings.contains("读懂ta的弦外之音"));
        assert!(settings.contains("https://www.shineway.tech/biyi/feature/chat"));
        assert!(settings.contains("营销大师"));
        assert!(settings.contains("AI 营销内容创作工具"));
        assert!(settings.contains("https://www.shineway.tech/product/marketing-master/"));
        assert!(settings.contains("width: root.card-size"));
        assert!(settings.contains("height: root.card-size"));
        assert!(settings.contains("root.recommendations-wrap"));
        assert!(settings.contains("in property <image> artwork;"));
        assert!(settings.contains("../../assets/recommendations/audio-separation.png"));
        assert!(settings.contains("../../assets/recommendations/conversation-insight.png"));
        assert!(settings.contains("../../assets/recommendations/marketing-master.png"));
        assert!(settings.contains("card-touch := TouchArea"));
        assert!(settings.contains("card-touch.has-hover ? -5px : 0px"));
        assert!(settings.contains("animate y"));
        assert!(!settings.contains("launch-button := Rectangle"));
        assert!(!settings.contains("launch-touch := TouchArea"));
        assert!(!settings.contains("启动方式"));
        assert_eq!(settings.matches("AppState.open-external-link(").count(), 1);
        assert!(app.contains("wire_external_link_callbacks(app, context.clone());"));
    }

    #[test]
    fn available_update_shows_red_dots_on_settings_and_about_entries() {
        let sidebar = include_str!("../../ui/components/sidebar.slint");
        let settings = include_str!("../../ui/pages/settings-page.slint");

        assert!(sidebar.contains("show-dot: AppState.update-available"));
        assert!(settings.contains("update-indicator := Rectangle"));
        assert!(settings.contains("visible: AppState.update-available"));
        assert!(settings.contains("background: AppTheme.danger"));
    }

    #[test]
    fn invitation_code_ui_is_reachable_and_uses_the_reserved_backend_contract() {
        let state = include_str!("../../ui/app-state.slint");
        let profile = include_str!("../../ui/dialogs/profile-dialog.slint");
        let top_bar = include_str!("../../ui/components/top-bar.slint");
        let app = include_str!("../../ui/app.slint");
        let account_api = include_str!("api/account.rs");
        let callback = include_str!("callbacks/invitation_code.rs");

        assert!(state.contains("callback submit-invitation-code();"));
        assert!(profile.contains("AppState.account-center-section == \"invitation\""));
        assert!(profile.contains("请填写邀请码"));
        assert!(profile.contains("填写邀请码，确认后将由服务端验证"));
        assert!(state.contains("invitation-code-submitted: false"));
        assert!(profile.contains("每个账号只能填写一次"));
        assert!(profile.contains(
            "disabled: AppState.invitation-code-busy || AppState.invitation-code-submitted"
        ));
        assert!(top_bar.contains("AppState.navigate(\"invitation-gift\")"));
        // The invitation pill's layout and navigation are exercised by
        // tests/top_bar_layout.rs; its appearance is not a backend contract.
        assert!(app.contains("AppState.page == \"invitation-gift\""));
        assert!(account_api.contains("/v1/account/invitation-code"));
        assert!(callback.contains("api.submit_invitation_code_scoped(&code, &worker_scope)"));
        assert!(callback.contains("api.invitation_dashboard_scoped(&worker_scope)"));
        assert!(callback.contains("state.set_invitation_code_submitted(true)"));
        assert!(callback.contains("error.is_invitation_code_already_submitted()"));
        assert!(account_api.contains("invitation_code_submitted: bool"));
        assert!(!callback.contains("ELUNVI-2026"));
    }

    #[test]
    fn invitation_rewards_page_uses_server_authoritative_rules_and_invited_users() {
        let state = include_str!("../../ui/app-state.slint");
        let page = include_str!("../../ui/pages/invitation-gift-page.slint");
        let types = include_str!("../../ui/types.slint");
        let api = include_str!("api/account.rs");
        let callbacks = include_str!("callbacks/invitation_code.rs");

        assert!(state.contains("invitation-reward-rate: \"\""));
        assert!(!state.contains("invitation-reward-rate: \"10\""));
        assert!(state.contains("invitation-count"));
        assert!(state.contains("invitation-history-reward"));
        assert!(state.contains("invitation-own-code"));
        assert!(state.contains("invitation-rule-description"));
        assert!(state.contains("property <[InvitedUserView]> invitation-users: []"));
        assert!(state.contains("callback load-more-invitation-users()"));
        assert!(state.contains("invitation-users-has-more"));
        assert!(state.contains("invitation-users-loading"));
        assert!(types.contains("export struct InvitedUserView"));
        assert!(types.contains("id: string"));
        assert!(types.contains("reward-detail: string"));
        assert!(types.contains("registered-at: string"));
        assert!(page.contains("我的返利比例"));
        assert!(page.contains("服务端当前规则"));
        assert!(!page.contains("当前暂定返利比例"));
        assert!(!page.contains("返利比例暂定为 10%"));
        assert!(page.contains("邀请人数"));
        assert!(page.contains("历史返利额度"));
        let summary = page
            .split("summary := Rectangle")
            .nth(1)
            .and_then(|value| value.split("invitation-card := Rectangle").next())
            .expect("invitation reward summary");
        assert!(summary.contains("HorizontalLayout"));
        assert_eq!(summary.matches("RewardSummaryCard").count(), 3);
        assert_eq!(summary.matches("horizontal-stretch: 1").count(), 3);
        assert!(page.contains("我的邀请码"));
        assert!(page.contains("复制邀请信息"));
        assert!(page.contains("强烈安利一个 AI 美术生产工具——Elunvi Canvas！"));
        assert!(page.contains("符合活动规则的新用户可获得 200 积分体验额度"));
        assert!(page.contains("https://www.shineway.tech/product/elunvi-canvas/"));
        assert!(page.contains("已邀请用户"));
        assert!(page.contains("返利明细"));
        assert!(page.contains("注册时间"));
        assert!(page.contains("for user in AppState.invitation-users"));
        assert!(page.contains("AppState.load-more-invitation-users()"));
        assert!(api.contains("/v1/account/invitations?limit=50&cursor={cursor}"));
        assert!(callbacks.contains("api.invitation_users_scoped(&cursor, &worker_scope)"));
        assert!(page.contains("AppState.invitation-rule-description"));
        assert!(!page.contains("当前返利比例暂定为 10%"));
        assert!(!page.contains("邀请链接"));
        assert!(!page.contains("复制链接"));
        assert!(!state.contains("invitation-share-link"));
        assert!(page.contains("AppState.copy-contact-detail(root.copy-value)"));
        assert!(!page.contains("可转返利"));
    }

    #[test]
    fn notifications_keep_the_server_cursor_and_append_more_rows() {
        let state = include_str!("../../ui/app-state.slint");
        let page = include_str!("../../ui/pages/notifications-page.slint");
        let api = include_str!("api/notifications.rs");
        let callbacks = include_str!("callbacks/notification.rs");

        assert!(state.contains("callback load-more-notifications()"));
        assert!(state.contains("notification-page-has-more"));
        assert!(state.contains("notification-page-loading"));
        assert!(page.contains("AppState.load-more-notifications()"));
        assert!(api.contains("next_cursor: Option<String>"));
        assert!(api.contains("/v1/notifications?limit=50&cursor={cursor}"));
        let start = callbacks.split_once("fn start_notification_page(").unwrap().1
            .split_once("fn notification_session_ended(").unwrap().0;
        let poll = callbacks.split_once("fn poll_server_notifications(").unwrap().1
            .split_once("fn notification_is_success(").unwrap().0;
        let update = callbacks.split_once("fn update_notification_store(").unwrap().1
            .split_once("fn poll_notification_save(").unwrap().0;
        assert!(start.contains("notification_page_epoch.checked_add(1)"));
        assert!(start.contains("api.list_page_scoped(cursor.as_deref(), scope)"));
        assert!(start.contains("capture, epoch, append, receiver"));
        assert!(poll.contains("update_notification_store(&app, &context, &capture, Some(epoch)"));
        assert!(poll.contains("if !append { store.notifications.clear(); }"));
        assert!(poll.contains("existing.id == item.id"));
        assert!(poll.contains("store.notifications.push(item)"));
        assert!(poll.contains("let cursor = page.next_cursor.unwrap_or_default()"));
        assert!(poll.contains("set_notification_next_cursor(cursor.clone().into())"));
        assert!(update.contains("store.notification_page_epoch != epoch"));
    }

    #[test]
    fn invitation_gift_asset_is_packaged_as_a_compact_transparent_icon() {
        let icon = image::load_from_memory(include_bytes!("../../assets/invitation-gift.png"))
            .expect("decode invitation gift icon")
            .to_rgba8();

        assert_eq!(icon.dimensions(), (256, 256));
        assert_eq!(icon.get_pixel(0, 0).0[3], 0);
    }

    #[test]
    fn legacy_local_store_shows_the_first_launch_contact_popup() {
        let data: LocalStoreData =
            serde_json::from_str("{}").expect("deserialize legacy local store");
        assert!(!data.contact_popup_dismissed);

        let saved = LocalStoreData {
            contact_popup_dismissed: true,
            ..LocalStoreData::default()
        };
        let serialized = serde_json::to_string(&saved).expect("serialize local store");
        let restored: LocalStoreData =
            serde_json::from_str(&serialized).expect("restore local store");
        assert!(restored.contact_popup_dismissed);
    }

    #[test]
    fn active_brand_assets_use_the_new_elunvi_logo() {
        let logo = image::load_from_memory(include_bytes!("../../assets/logo.png"))
            .expect("decode active logo")
            .to_rgba8();
        assert_eq!(logo.dimensions(), (460, 460));
        assert_eq!(logo.get_pixel(0, 0).0[3], 0);
        assert!(include_bytes!("../../assets/app.ico").len() > 20_000);
        assert!(include_bytes!("../../assets/app.icns").len() > 100_000);
    }

    #[test]
    fn macos_dock_icon_keeps_platform_safe_area() {
        let platform = include_str!("../platform.rs");
        let icon = image::load_from_memory(include_bytes!("../../assets/app-icon-macos.png"))
            .expect("decode macOS app icon")
            .to_rgba8();
        assert!(platform.contains("include_bytes!(\"../assets/app-icon-macos.png\")"));
        assert_eq!(icon.dimensions(), (1024, 1024));

        let mut min_x = 1024;
        let mut min_y = 1024;
        let mut max_x = 0;
        let mut max_y = 0;
        for (x, y, pixel) in icon.enumerate_pixels() {
            if pixel.0[3] > 0 {
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }

        assert_eq!((min_x, min_y, max_x, max_y), (100, 100, 923, 923));
    }
}
