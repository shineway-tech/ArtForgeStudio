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
        assert!(windows_update_installer_args().contains(&"/VERYSILENT"));
        assert!(windows_update_installer_args().contains(&"/CLOSEAPPLICATIONS"));
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
        assert!(updater.contains("\"/VERYSILENT\""));
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
        assert!(composer.contains(
            "y: root.prompt-input-y() + (AppState.selected-custom-prompt-items.length > 0 ? 66px : 32px);"
        ));
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
        assert!(composer.contains("property <int> custom-prompt-selected-index: -1"));
        assert_eq!(composer.matches("event.text == Key.DownArrow").count(), 2);
        assert_eq!(composer.matches("event.text == Key.UpArrow").count(), 2);
        assert_eq!(composer.matches("event.text == Key.Escape").count(), 2);
        assert!(composer.contains("AppState.prompt-history[root.prompt-history-selected-index]"));
        assert!(composer.contains("AppState.toggle-custom-prompt-selection("));
        assert!(composer.contains("root.scroll-prompt-history-selection-into-view()"));
        assert!(composer.contains("root.scroll-custom-prompt-selection-into-view()"));
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
        assert!(callbacks.contains("save_local_store(&app, &store.borrow())"));
        assert!(callbacks.contains("state.on_save_custom_prompt"));
        assert!(callbacks.contains("navigate_to(app, \"custom-prompt-editor\")"));
        assert!(callbacks.contains("state.set_custom_prompt_editor_open(false)"));
        assert!(callbacks.contains("state.on_begin_new_custom_prompt"));
        assert!(callbacks.contains("state.on_begin_edit_custom_prompt"));
        assert!(callbacks.contains("state.on_choose_custom_prompt_reference"));
        assert!(
            local_store.contains("custom_prompt_profiles: store.custom_prompt_profiles.clone()")
        );
    }

    #[test]
    fn double_slash_custom_prompts_are_single_select_title_tags() {
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
        assert!(callbacks.contains("current_workspace_category(&app)"));
        assert!(callbacks.contains("toggle_custom_prompt_selection_for_category"));
        assert!(local_store.contains("let was_selected = store"));
        assert!(local_store.contains("selected.clear();"));
        assert!(composer.contains("for item in AppState.selected-custom-prompt-items"));
        assert!(popup.contains("text: item.name"));
        assert!(popup.contains("item.selected ? AppTheme.accent"));
        assert!(popup.contains("root.queue-custom-prompt-selection(item.content)"));
        assert!(!popup.contains("AppState.toggle-custom-prompt-selection(item.content)"));
        assert!(popup.contains("custom-prompt-row := HorizontalLayout"));
        assert!(popup.contains("tag-title.preferred-width + 28px"));
        assert!(!composer.contains("function custom-prompt-tag-width()"));
        assert!(!popup.contains("root.apply-selected-prompt(item.content)"));
        assert!(!popup.contains("text: item.content"));
        assert!(!popup.contains("text: item.preview"));
        assert!(controller.contains("compose_selected_custom_prompts"));
        assert!(controller.contains("selected_custom_prompts_for_category"));
    }

    #[test]
    fn double_slash_selection_closes_inline_and_backspace_removes_the_tag() {
        let composer = include_str!("../../ui/components/prompt-composer.slint");
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

        assert!(composer.contains("prompt-entry-row := HorizontalLayout"));
        assert!(composer.contains("prompt-cursor-area := TouchArea"));
        assert!(composer.contains("mouse-cursor: text;"));
        assert!(composer.contains("horizontal-stretch: 1;"));
        assert!(composer.contains("? max(0px, (26px - AppState.settings-font-size * 1px) / 2)"));
        assert!(composer.contains("height: parent.height - self.y;"));
        assert!(composer.contains("width: selected-title.preferred-width + 38px;"));
        assert!(composer.contains("for item in AppState.selected-custom-prompt-items: Rectangle"));
        assert!(!composer.contains("selected-prompt-tags := Rectangle"));
        assert!(!composer.contains("selected-prompt-row := HorizontalLayout"));
        assert!(!composer.contains(
            "width: min(max(72px, selected-title.preferred-width + 38px), root.width - 104px);"
        ));
        let prompt_entry = composer
            .split("prompt-entry-row := HorizontalLayout")
            .nth(1)
            .and_then(|value| {
                value
                    .split("for item in AppState.selected-custom-prompt-items")
                    .next()
            })
            .expect("prompt entry row");
        assert!(!prompt_entry.contains("alignment: start;"));
        assert!(!composer
            .contains("x: AppState.selected-custom-prompt-items.length > 0 ? 270px : 24px;"));
        assert!(composer.contains("y: root.prompt-input-y();"));
        assert!(composer.contains("event.text == Key.Backspace"));
        assert!(composer.contains("&& AppState.prompt == \"\""));
        assert!(composer.contains("AppState.selected-custom-prompt-items[0].content"));
        assert!(composer.contains("property <bool> custom-prompt-selection-pending: false;"));
        assert!(composer.contains("function queue-custom-prompt-selection(value: string)"));
        assert!(composer.contains("interval: 1ms;"));
        assert!(composer.contains("running: root.custom-prompt-selection-pending;"));
        assert!(composer.contains("AppState.custom-prompt-open = false"));
        assert!(composer.contains("custom-prompt-popup.close()"));
        assert!(composer.contains("prompt-input.set-selection-offsets(2147483647, 2147483647)"));
        assert!(popup.contains("root.queue-custom-prompt-selection(item.content)"));
        assert!(!popup.contains("AppState.toggle-custom-prompt-selection(item.content)"));
        assert!(composer.contains("root.custom-prompt-selected-index = -1;"));
        assert!(composer.contains("root.custom-prompt-selected-index < 0"));
        assert!(composer
            .contains("visible: selected-tag-touch.has-hover || selected-close-touch.has-hover;"));
        assert!(composer.contains("selected-close-touch := TouchArea"));
        assert!(composer.contains("text: \"×\""));
        assert!(composer.contains("selected-title.preferred-width + 38px"));
        assert!(composer.contains("tag-title.preferred-width + 28px"));
        assert!(popup.contains("overflow: clip"));
        assert!(callbacks.contains("if state.get_prompt().trim() == \"//\""));
        assert!(callbacks.contains("state.set_prompt(\"\".into());"));
        assert!(callbacks.contains("slint::Timer::single_shot(Duration::ZERO"));
    }

    #[test]
    fn custom_prompt_selections_are_isolated_by_workspace_category() {
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
            vec!["角色提示词二".to_string()]
        );

        toggle_custom_prompt_selection_for_category(&mut store, "scene", "场景提示词");
        assert_eq!(
            selected_custom_prompts_for_category(&store, "scene"),
            vec!["场景提示词".to_string()]
        );
        assert_eq!(
            selected_custom_prompts_for_category(&store, "character"),
            vec!["角色提示词二".to_string()]
        );

        toggle_custom_prompt_selection_for_category(&mut store, "character", "角色提示词二");
        assert!(selected_custom_prompts_for_category(&store, "character").is_empty());
        assert_eq!(
            selected_custom_prompts_for_category(&store, "scene"),
            vec!["场景提示词".to_string()]
        );
    }

    #[test]
    fn selected_custom_prompt_tags_keep_the_default_placeholder_hidden_without_focus() {
        let composer = include_str!("../../ui/components/prompt-composer.slint");
        let placeholder = composer
            .split("text: root.prompt-placeholder()")
            .next()
            .and_then(|value| value.rsplit("Text {").next())
            .expect("prompt placeholder");

        assert!(placeholder.contains("prompt-input.text == \"\""));
        assert!(placeholder.contains("AppState.selected-custom-prompt-items.length == 0"));
        assert!(placeholder.contains("!prompt-input.has-focus"));
    }

    #[test]
    fn selected_custom_prompt_contents_are_composed_only_for_interactive_generation() {
        let selected = vec![
            "portrait lighting".to_string(),
            "  ink texture  ".to_string(),
        ];

        assert_eq!(
            compose_selected_custom_prompts("main subject", &selected),
            "portrait lighting\n\nink texture\n\nmain subject"
        );
        assert_eq!(
            compose_selected_custom_prompts("//", &selected),
            "portrait lighting\n\nink texture"
        );
        assert_eq!(compose_selected_custom_prompts("", &[]), "");
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
        assert!(popup.contains("custom-prompt-row := HorizontalLayout"));
        assert!(popup.contains("height: 36px;"));
    }

    #[test]
    fn custom_prompt_editor_uses_the_structured_reference_form() {
        let page = include_str!("../../ui/pages/custom-prompt-editor-page.slint");
        let app = include_str!("../../ui/app.slint");
        let state = include_str!("../../ui/app-state.slint");
        let callbacks = include_str!("callbacks/custom_prompt.rs");
        let prompt_tasks = include_str!("callbacks/prompt_tasks.rs");

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
        assert!(callbacks.contains(".pick_files()"));
        assert!(callbacks.contains("MAX_CUSTOM_PROMPT_REFERENCES: usize = 8"));
        assert!(!page.contains("Analyzed locally; the image is not uploaded"));
        assert!(!page.contains("由本地客户端分析，不会上传参考图"));
        assert!(state.contains("custom-prompt-analyzing"));
        assert!(callbacks.contains("sync_style_analysis_selection(&state)"));
        assert!(callbacks.contains("start_backend_prompt_task("));
        assert!(prompt_tasks.contains("GenerationApi::new(backend.api.clone())"));
        assert!(prompt_tasks.contains("api.upload_reference_scoped(&path, &session_scope)"));
        assert!(prompt_tasks.contains("reference_file_ids: (!record.uploaded_file_ids.is_empty())"));
        assert!(prompt_tasks.contains("result_prompt"));
        assert!(!callbacks.contains("analyze_reference_style("));
    }

    #[test]
    fn custom_prompt_reference_analysis_uses_the_selected_server_model() {
        let callbacks = include_str!("callbacks/custom_prompt.rs");
        let prompt_tasks = include_str!("callbacks/prompt_tasks.rs");

        assert!(callbacks.contains("task_type: \"image_style_analysis\""));
        assert!(callbacks.contains("model_code"));
        assert!(prompt_tasks.contains("task_type: record.task_type.clone()"));
        assert!(prompt_tasks.contains("model_code: record.model_code.clone()"));
        assert!(prompt_tasks.contains("reference_file_ids: (!record.uploaded_file_ids.is_empty())"));
        assert!(prompt_tasks.contains("api.create_task_scoped(&request, &session_scope)"));
        assert!(prompt_tasks.contains("IMAGE_POLL_INTERVAL_MS"));
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

        assert!(composer.contains("label: AppState.en ? \"Add image\" : \"添加图片\""));
        assert!(upload_card.contains("in property <string> label"));
        assert!(upload_card.contains("text: root.label"));
        assert!(upload_card.contains("border-radius: 10px"));
        assert!(composer.contains("width: 88px"));
        assert!(composer.contains("return 8;"));
        assert!(callbacks.contains("add_reference_from_path(&app, &store, &path)"));
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
        assert!(callbacks.contains("transfer.plain_text()"));
        assert!(callbacks.contains("external_image_url(data.as_str())"));
        assert!(callbacks.contains("start_external_reference_import"));
        assert!(callbacks.contains("download_external_reference"));
        assert!(callbacks.contains("take_external_image_drops"));
        assert!(callbacks.contains("on_process_external_image_drops"));
        assert!(!callbacks.contains("poll_external_image_drops"));
        assert!(callbacks.contains("ExternalImageDrop::Paths"));
        assert!(callbacks.contains("ExternalImageDrop::Text"));
        assert!(callbacks.contains("external_drop_inside_reference_input"));
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
        let runtime = include_str!("mod.rs");
        let references = include_str!("callbacks/reference.rs");
        let viewer = include_str!("callbacks/viewer.rs");
        let handler = drag
            .split("pub fn start_thumbnail_file_drag(path: PathBuf) -> bool")
            .nth(1)
            .and_then(|value| value.split("#[cfg(not(target_os = \"windows\"))]").next())
            .expect("Windows file drag handler");

        assert_eq!(handler.matches("ReleaseCapture()").count(), 2);
        assert!(handler.contains("windows_file_drag::run(path).is_ok()"));
        assert!(!handler.contains("std::thread::spawn"));
        assert!(drag
            .contains("DoDragDrop(&data_object, &drop_source, DROPEFFECT_COPY, &mut effect).ok()"));
        assert!(runtime.contains("fn reset_pointer_after_native_drag"));
        assert!(runtime.contains("WindowEvent::PointerExited"));
        assert!(references.contains("reset_pointer_after_native_drag(&app)"));
        assert!(viewer.contains("reset_pointer_after_native_drag(&app)"));
    }

    #[test]
    fn macos_file_drag_exposes_the_local_image_to_finder() {
        let drag = include_str!("../drag_preview.rs");
        let platform = include_str!("../platform.rs");

        assert!(drag.contains("crate::platform::queue_macos_file_drag(path)"));
        assert!(platform.contains("PENDING_MACOS_FILE_DRAG"));
        assert!(platform.contains("sel!(mouseDragged:)"));
        assert!(platform.contains("ORIGINAL_MOUSE_DRAGGED"));
        assert!(platform.contains("sel!(mouseUp:)"));
        assert!(platform.contains("ORIGINAL_MOUSE_UP"));
        assert!(platform.contains("start_native_file_drag(view, event, path)"));
        assert!(platform.contains("dragFile_fromRect_slideBack_event"));
        assert!(platform.contains("std::fs::canonicalize(&path)"));
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
        let section = include_str!("../../ui/components/time-group-section.slint");
        let waterfall = include_str!("../../ui/components/generation-waterfall.slint");
        let column = include_str!("../../ui/components/generation-masonry-column.slint");

        assert!(card.contains("in property <int> sequence-index: 0;"));
        assert!(card.contains("in property <int> bounce-step: 0;"));
        assert!(card.contains("root.bounce-step - root.sequence-index * 4 + 40"));
        assert!(card.contains("phase == 5 ? 0px - 7px"));
        assert!(card.contains("phase == 11 ? 1px"));
        assert!(card.contains(
            "animate y { duration: AppState.reduced-motion ? 0ms : 65ms; easing: ease-in-out; }"
        ));
        assert!(section.contains("interval: AppState.reduced-motion ? 200ms : 50ms;"));
        assert!(section.contains("running: root.loading-count > 0;"));
        assert!(section.contains("Math.mod(root.loading-bounce-step + 1, 40)"));
        for index in 0..4 {
            assert!(section.contains(&format!("sequence-index: {index};")));
            assert!(column.contains(&format!("sequence-index: {index};")));
        }
        assert!(section.contains("bounce-step: root.loading-bounce-step;"));
        assert!(waterfall.contains("bounce-step: root.bounce-step;"));
        assert!(column.contains("bounce-step: root.bounce-step;"));
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

        assert!(composer.contains("return 4;"));
        assert!(composer.contains("function reference-card-size() -> length"));
        assert!(composer.contains("width: root.reference-card-size();"));
        assert!(composer.contains("height: root.reference-card-size();"));
        assert!(composer.contains("(root.width - 48px - 30px) / 4"));
        assert!(composer.contains("function reference-grid-x() -> length"));
        assert!(composer.contains("return 24px;"));
        assert!(composer.contains("- root.reference-card-size()"));
        assert!(composer.contains("(root.reference-row-count() - 1) * root.reference-row-height()"));
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
        let groups = include_str!("../../ui/components/time-grouped-gallery.slint");
        let thumbnail = include_str!("../../ui/components/thumbnail-card.slint");
        let waterfall_column = include_str!("../../ui/components/waterfall-column.slint");
        let generation_column =
            include_str!("../../ui/components/generation-masonry-column.slint");
        let waterfall = include_str!("../../ui/components/gallery-waterfall.slint");
        let generation_waterfall = include_str!("../../ui/components/generation-waterfall.slint");

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
        assert!(app.contains("save_user_profile(&app);"));
        assert!(profile.contains("ui_preferences: UiPreferencesData"));
        assert!(profile.contains("state.set_generation_gallery_layout"));
        assert!(groups.contains("layout-mode: root.layout-mode;"));
        assert!(thumbnail.contains("in property <bool> masonry: false;"));
        assert!(thumbnail.contains("root.item.height / root.item.width"));
        assert!(waterfall_column.contains("masonry: true;"));
        for column in [waterfall_column, generation_column] {
            assert!(column.contains("in root.column-length(): ThumbnailCard"));
            assert!(column.contains("root.items[root.source-index(index)]"));
            assert!(!column.contains("for item[index] in root.items"));
            assert!(!column.contains("active: Math.mod"));
        }
        assert!(generation_column.contains("function first-source-index() -> int"));
        assert!(generation_column.contains("Math.mod(root.loading-count, root.column-count)"));
        assert!(waterfall
            .contains("floor((root.grid-width() + root.grid-gap()) / root.item-slot-width())"));
        assert!(!waterfall.contains("min(root.items.length"));
        assert!(waterfall.contains(
            "(root.grid-width() - (root.column-count() - 1) * root.grid-gap()) / root.column-count()"
        ));
        assert!(generation_waterfall.contains("card-width: root.item-width();"));
        assert!(generation_waterfall.contains("function column-count() -> int"));
        assert!(panel.contains(
            "AppState.generation-gallery-layout == \"waterfall\" ? root.base-thumb-width() : root.item-width()"
        ));
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
        let legacy: UserProfileData =
            serde_json::from_str("{}").expect("deserialize legacy user profile");
        assert_eq!(legacy.ui_preferences.generation_gallery_layout, "grid");
        assert_eq!(legacy.ui_preferences.asset_gallery_layout, "grid");
        assert_eq!(legacy.ui_preferences.inspiration_gallery_layout, "grid");

        let saved = UserProfileData {
            ui_preferences: UiPreferencesData {
                generation_gallery_layout: "waterfall".to_string(),
                asset_gallery_layout: "waterfall".to_string(),
                inspiration_gallery_layout: "waterfall".to_string(),
            },
            ..UserProfileData::default()
        };
        let serialized = serde_json::to_string(&saved).expect("serialize user profile");
        let restored: UserProfileData =
            serde_json::from_str(&serialized).expect("restore user profile");
        assert_eq!(
            restored.ui_preferences.generation_gallery_layout,
            "waterfall"
        );
        assert_eq!(normalize_gallery_layout(" WATERFALL "), "waterfall");
        assert_eq!(normalize_gallery_layout("unsupported"), "grid");
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

        let canvas = sidebar.find("page: \"canvas\"").expect("canvas nav");
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
        assert!(sidebar.contains(
            "active: AppState.page == \"settings\" || AppState.page == \"custom-prompt-editor\";"
        ));
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
        assert!(reference.contains("page.as_str() == \"toolbox-convert\""));
        assert!(reference.contains("toolbox_callbacks::add_conversion_paths"));
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
        assert!(callbacks.contains("persist_colorization_source(&source)"));
        assert!(callbacks.contains("state.on_start_colorize"));
        assert!(callbacks.contains("state.on_reveal_colorize_result"));
        assert!(callbacks.contains("start_image_colorization"));
        assert!(callbacks.contains("CreateImageColorization"));
        assert!(callbacks.contains("create_image_colorization"));
        assert!(callbacks.contains("model_code: \"aliyun_image_colorization\""));
        assert!(callbacks.contains("origin: \"image_colorization\".to_string()"));
        assert!(callbacks.contains("model: \"老照片上色\".to_string()"));
        assert!(callbacks.contains("本次老照片上色需要 20 积分"));
        assert!(!callbacks.contains("老照片上色能力等待后端配置"));
        assert!(api.contains("/v1/toolbox/image-colorizations"));
        assert!(recovery.contains("resume_pending_image_colorization"));
        assert!(viewer.contains("item.origin != \"image_colorization\""));
        assert!(reference_callbacks.contains("page.as_str() == \"toolbox-colorize\""));
        assert!(reference_callbacks.contains("toolbox_callbacks::add_colorization_paths"));
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
        assert!(reference.contains("page.as_str() == \"toolbox-crop\""));
        assert!(reference.contains("toolbox_callbacks::add_crop_paths"));
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
        let compression_import = callbacks
            .split("pub(super) fn add_compression_paths")
            .nth(1)
            .and_then(|block| block.split("fn paste_compression_image").next())
            .expect("compression import implementation");
        assert!(compression_import.contains("compression_source_extension(&canonical)"));
        assert!(compression_import
            .contains("load_preview_image(&canonical, PreviewPurpose::Toolbox)"));
        assert!(!compression_import.contains("is_compression_image_path"));
        assert!(!compression_import.contains("image::open(&canonical)"));
        assert!(image_processing.contains("ImageCompressionMode::Quality"));
        assert!(image_processing.contains("ImageCompressionMode::TargetBytes"));
        assert!(image_processing.contains("resize_image_by_scale"));
        assert!(image_processing.contains("CompressionFormat::Jpeg"));
        assert!(image_processing.contains("CompressionFormat::Png"));
        assert!(image_processing.contains("CompressionFormat::WebP"));
        assert!(image_processing.contains("CompressionFormat::Bmp"));
        assert!(reference.contains("page.as_str() == \"toolbox-compress\""));
        assert!(reference.contains("toolbox_callbacks::add_compression_paths"));
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
        assert!(callbacks.contains("ENHANCEMENT_MAX_INPUT_BYTES: u64 = 20 * 1024 * 1024"));
        assert!(callbacks.contains("ENHANCEMENT_MIN_EDGE: u32 = 64"));
        assert!(callbacks.contains("ENHANCEMENT_MAX_LONG_EDGE: u32 = 5000"));
        assert!(!callbacks.contains("ENHANCEMENT_MAX_SHORT_EDGE"));
        assert!(callbacks.contains("ENHANCEMENT_MAX_ASPECT_RATIO: u32 = 2"));
        assert!(callbacks.contains("state.set_enhance_estimated_credits(\"20\""));
        assert!(!callbacks.contains("state.set_enhance_estimated_credits(\"10\""));
        assert!(callbacks.contains("model: \"图片清晰\".to_string()"));
        assert!(callbacks.contains("target_quality:"));
        assert!(callbacks.contains("CreateImageEnhancement"));
        assert!(callbacks.contains("image_enhancement"));
        assert!(callbacks.contains("category: \"other\".to_string()"));
        assert!(callbacks.contains("origin: \"image_enhancement\".to_string()"));
        assert!(callbacks.contains("upscale_done: true"));
        assert!(api.contains("/v1/toolbox/image-enhancements"));
        assert!(api.contains("pub(crate) target_quality: String"));
        assert!(reference_callbacks.contains("page.as_str() == \"toolbox-enhance\""));
        assert!(reference_callbacks.contains("image_enhancement_callbacks::add_enhancement_paths"));
        assert!(recovery.contains("resume_pending_image_enhancement"));
        assert!(recovery.contains("model.code == \"aliyun_super_resolution\""));
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
        assert!(reference_callbacks.contains("page.as_str() == \"toolbox-watermark\""));
        assert!(reference_callbacks.contains("toolbox_callbacks::add_watermark_paths"));
        assert!(callbacks.contains("reveal_path_in_file_manager(&path)"));
        assert!(callbacks.contains("CreateWatermarkRemoval"));
        assert!(callbacks.contains("image_watermark_removal"));
        assert!(callbacks.contains("category: \"other\".to_string()"));
        assert!(callbacks.contains("origin: \"watermark_removal\".to_string()"));
        assert!(callbacks.contains("model: \"去水印\".to_string()"));
        assert!(callbacks.contains("store.assets.insert(0, item)"));
        assert!(callbacks.contains("save_local_store(app, &store)"));
        assert!(!callbacks.contains("store.generations.insert"));
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
    fn prompt_popups_show_ten_single_line_rows_without_losing_full_values() {
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
        assert!(
            composer.contains("viewport-width: max(self.width, custom-prompt-row.preferred-width)")
        );
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
            .find("AppState.prompt = value")
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
    fn enter_confirms_inputs_and_alt_enter_keeps_prompt_line_breaks() {
        let field = include_str!("../../ui/components/field.slint");
        let auth = include_str!("../../ui/dialogs/auth-dialog.slint");
        let invoice = include_str!("../../ui/dialogs/invoice-dialog.slint");
        let prompt = include_str!("../../ui/components/prompt-composer.slint");

        assert!(field.contains("callback accepted();"));
        assert!(field.contains("accepted => { root.accepted(); }"));

        assert!(auth.contains("function confirm-auth()"));
        assert_eq!(
            auth.matches("accepted => { root.confirm-auth(); }").count(),
            2
        );

        assert!(invoice.contains("function submit-form()"));
        assert_eq!(
            invoice
                .matches("accepted => { root.submit-form(); }")
                .count(),
            3
        );

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
        assert!(clear_function.contains("prompt-input.text = \"\";"));
        assert!(clear_function.contains("AppState.toggle-custom-prompt-selection("));
        assert!(clear_function.contains("AppState.invalidate-deep-prompt-binding();"));
        assert!(!clear_function.contains("AppState.references"));
        assert!(!clear_function.contains("AppState.negative-prompt"));
    }

    #[test]
    fn image_quality_is_not_limited_by_membership() {
        let chooser = include_str!("../../ui/components/inline-card-chooser.slint");
        let quality_button = include_str!("../../ui/components/quality-button.slint");
        let canvas = include_str!("../../ui/pages/infinite-canvas-page.slint");
        let membership = include_str!("../../ui/components/membership-plans.slint");
        let profile = include_str!("../../ui/dialogs/profile-dialog.slint");
        let app = include_str!("../../ui/app.slint");

        for source in [chooser, quality_button, canvas, membership, profile, app] {
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
        let top_bar = include_str!("../../ui/components/top-bar.slint").replace("\r\n", "\n");

        assert!(top_bar.contains("x: 18px;\n            y: 0px;"));
        assert!(top_bar
            .contains("width: max(360px, parent.width - 18px - root.actions-width() - 32px);"));
        assert!(top_bar.contains("(root.width - 18px - root.actions-width() - 70px) / 2"));
        assert!(top_bar.contains(
            "x: 0px;\n                    y: 6px;\n                    kind: \"image\";"
        ));
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
        assert!(page.contains("AppState.select-canvas-node(root.note.id, event.modifiers.shift);"));
        assert!(page.contains("root.marquee-additive = event.modifiers.shift;"));
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
        assert!(callbacks.contains("app_data_dir().join(\"canvas\").join(\"uploads\")"));
        assert!(callbacks.contains("atomic_write_file(&destination, &bytes)"));
        assert!(callbacks.contains("path: destination.display().to_string()"));
        assert!(callbacks.contains("image_path = image.path"));
        assert!(sync.contains("prepare_preview_image_if("));
        assert!(sync.contains("PreviewPurpose::Canvas"));
        assert!(sync.contains("CANVAS_PREVIEW_EPOCH.load(Ordering::Acquire) == preview_epoch"));
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
        assert!(callbacks.contains("inspect_image_dimensions(&source_path)"));
        assert!(ops.contains("fn resize_image_node_proportionally"));
        assert!(ops.contains("fn fit_image_node_to_intrinsic_aspect"));
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
        assert!(callbacks.contains("pick_canvas_image(&app, &id)"));
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
        assert!(callbacks.contains("persist_canvas_clipboard_image"));
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
        assert!(page.contains("toolbar.y - self.height - 10px"));
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
            3
        );
        assert!(!text_bar.contains("CanvasMediaAction { scale-factor: root.node-scale(); width:"));
        assert!(!media_bar.contains("CanvasMediaAction { scale-factor: root.node-scale(); width:"));
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
        assert!(callbacks.contains("open_payment_checkout"));
        assert!(callbacks.contains("Duration::from_secs(3)"));
        assert!(
            callbacks.contains("continue_payment_order(&app, context, backend, started, false)")
        );
        assert!(callbacks.contains("暂时无法确认支付结果，请稍后查看订单状态"));
        assert!(membership.contains("PurchaseAgreements"));
        assert!(credit_page.contains("PurchaseAgreements"));
        assert!(purchase_agreements.contains("purchase-membership-accepted"));
        assert!(purchase_agreements.contains("purchase-credit-rules-accepted"));
        assert!(callbacks.contains(
            "agreements_api.accept_agreements_scoped(&acceptances, &worker_scope)?;"
        ));
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
        assert!(profile.contains("x: parent.width - 128px;"));
        assert!(profile.contains("x: parent.width - 158px;"));
        assert!(profile.contains("x: parent.width - 186px;"));
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
        assert_eq!(viewer.matches("ViewerFooterActionButton {").count(), 4);
        assert!(viewer.contains("AppState.viewer-download-image();"));
        assert!(viewer.contains("AppState.viewer-use-reference();"));
        assert!(viewer.contains("AppState.viewer-open-image-editor();"));
        assert!(viewer.contains("AppState.viewer-edit();"));
        assert!(viewer.contains("AppState.request-delete-asset(AppState.viewer-id);"));
        assert!(viewer.contains("@image-url(\"../../assets/icons/download.svg\")"));
        assert!(viewer.contains("@image-url(\"../../assets/icons/edit.svg\")"));
        assert!(viewer.contains("@image-url(\"../../assets/icons/upload.svg\")"));
        assert!(viewer.contains("@image-url(\"../../assets/icons/trash.svg\")"));

        assert!(state.contains("callback viewer-open-image();"));
        assert!(callbacks.contains("state.on_viewer_open_image"));
        assert!(callbacks.contains("open_viewer_image(&app, &store.borrow())"));
        assert!(feature.contains("pub(super) fn open_viewer_image"));
        assert!(feature.contains("open_path_with_default_app(&source)"));
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
        assert!(callbacks.contains("CreateImageCutout"));
        assert!(callbacks.contains("subject_type:"));
        assert!(callbacks.contains("task_type: \"image_cutout\".to_string()"));
        assert!(callbacks.contains("model_code: \"aliyun_image_segmentation\".to_string()"));
        assert!(callbacks.contains("pending_delivery_saved"));
        assert!(callbacks.contains("acknowledge_delivery_after_local_save"));
        assert!(callbacks.contains("category: \"other\".to_string()"));
        assert!(callbacks.contains("origin: \"image_cutout\".to_string()"));
        assert!(callbacks.contains("cutout_done: true"));
        assert!(callbacks.contains("store.assets.insert(0, item)"));
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
    fn viewer_processing_actions_use_two_compact_two_column_cards() {
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
        assert_eq!(tools.matches("ViewerToolActionButton {").count(), 2);
        assert_eq!(tools.matches("horizontal-stretch: 1;").count(), 2);
        assert!(tools.contains("label: AppState.en ? \"Cutout\" : \"抠图\""));
        assert!(!tools.contains("label: AppState.en ? \"Remove Black\" : \"去黑\""));
        assert!(tools.contains("label: AppState.en ? \"Clear Upscale\" : \"清晰放大\""));
        assert!(tools.contains("@image-url(\"../../assets/icons/fit.svg\")"));
        assert!(!tools.contains("@image-url(\"../../assets/icons/palette.svg\")"));
        assert!(tools.contains("@image-url(\"../../assets/icons/focus.svg\")"));
        assert!(repeat.contains("HorizontalLayout"));
        assert_eq!(repeat.matches("ViewerToolActionButton {").count(), 2);
        assert!(repeat.contains("label: AppState.en ? \"Use Prompt\" : \"使用提示词\""));
        assert!(repeat.contains("label: AppState.en ? \"Generate Again\" : \"再次生成\""));
        assert!(repeat.contains("@image-url(\"../../assets/icons/edit.svg\")"));
        assert!(repeat.contains("@image-url(\"../../assets/icons/redo.svg\")"));
    }

    #[test]
    fn viewer_image_can_start_a_native_file_drag() {
        let viewer = include_str!("../../ui/dialogs/viewer-overlay.slint");
        let state = include_str!("../../ui/app-state.slint");
        let callbacks = include_str!("callbacks/viewer.rs");

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
        assert!(callbacks.contains("viewer_item(&store.borrow(), &id, &source)"));
        assert!(callbacks.contains("drag_preview::start_thumbnail_file_drag"));
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
        assert!(
            callbacks.contains("continue_payment_order(&app, context, backend, started, false);")
        );
        assert!(callbacks.contains("已恢复未完成订单，可重新打开支付宝继续支付"));
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
        assert!(backend.contains("remove_pending_generation_scoped("));
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
    fn generation_terminal_scope_guard_clears_every_busy_surface() {
        let generation_state = include_str!("generation/state.rs");
        let generation_poll = include_str!("generation/poll.rs");
        let cutout = include_str!("callbacks/image_cutout.rs");
        let enhancement = include_str!("callbacks/image_enhancement.rs");
        let toolbox = include_str!("callbacks/toolbox.rs");

        assert!(generation_state.contains("GenerationScopeDisposition::CapturedTerminal"));
        assert!(generation_state
            .contains("sign_out_locally(&app, context, true, Some(session_scope.auth_epoch))"));
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
            assert!(generation_state.contains(setter), "missing reset {setter}");
        }
        assert!(generation_poll.contains("generation_scope_allows_polling"));
        assert!(cutout.contains("generation_scope_allows_polling"));
        assert!(enhancement.contains("generation_scope_allows_polling"));
        assert_eq!(
            toolbox.matches("generation_scope_allows_polling").count(),
            4,
            "watermark and colorization must guard both before and after reading outcomes",
        );
    }

    #[test]
    fn detached_generation_ack_and_cancel_observe_terminal_scope() {
        let generation_state = include_str!("generation/state.rs");
        let generation_poll = include_str!("generation/poll.rs");
        let generation_controller = include_str!("generation/controller.rs");

        assert!(generation_state.contains("fn observe_detached_generation_scope"));
        assert!(generation_state.contains("generation_scope_allows_polling"));
        let ack = generation_poll
            .split("pub(super) fn acknowledge_delivery_after_local_save")
            .nth(1)
            .expect("delivery ack helper");
        assert!(ack.contains("observe_detached_generation_scope("));
        assert!(ack.contains("pending_delivery_acknowledged("));
        assert!(generation_controller.contains("cancel_scoped(&server_task_id, &worker_scope)"));
        assert!(generation_controller.contains("observe_detached_generation_scope("));
    }

    #[test]
    fn generation_recovery_discovery_observes_terminal_scope() {
        let backend = include_str!("generation/backend.rs");
        let discovery = backend
            .split("fn recover_server_generation_tasks")
            .nth(1)
            .and_then(|value| value.split("fn resume_pending_generation").next())
            .expect("server recovery discovery");

        assert!(discovery.contains("list_tasks_scoped(status, &worker_scope)"));
        assert!(discovery.contains("task_scoped(&summary.id, &worker_scope)"));
        assert!(discovery.contains("backend_generation_scope_active(&backend, &worker_scope)"));
        assert!(discovery.contains("sender.send(Err(()))"));
        assert!(discovery.contains("generation_scope_allows_polling"));
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
            .split("pub(super) fn start_backend_generation")
            .nth(1)
            .and_then(|value| value.split("pub(super) fn start_backend_upscale").next())
            .expect("generation submission");

        assert!(submission.contains("let original_references ="));
        assert!(!submission.contains("references_for_category_mut"));
        assert!(!submission.contains("push_references(app"));
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
        assert!(app.contains("wire_external_link_callbacks(app);"));
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
        assert!(profile.contains("AppState.profile-section == \"invitation\""));
        assert!(profile.contains("请填写邀请码"));
        assert!(profile.contains("填写邀请码，确认后将由服务端验证"));
        assert!(state.contains("invitation-code-submitted: false"));
        assert!(profile.contains("每个账号只能填写一次"));
        assert!(profile.contains(
            "disabled: AppState.invitation-code-busy || AppState.invitation-code-submitted"
        ));
        assert!(top_bar.contains("AppState.navigate(\"invitation-gift\")"));
        assert!(top_bar.contains("changed has-hover"));
        assert!(top_bar.contains("self.has-hover && !AppState.reduced-motion"));
        assert!(!top_bar.contains("interval: 5s"));
        assert!(top_bar.contains("width: 44px"));
        assert!(top_bar.contains("running: root.wobbling"));
        assert!(top_bar.contains("function wobble-angle() -> angle"));
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
        assert!(page.contains("复制邀请码"));
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
        assert!(page.contains("AppState.copy-contact-detail(root.value)"));
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
        assert!(callbacks.contains("if append"));
        assert!(callbacks.contains("notification_page_epoch != request_epoch"));
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
        assert!(!platform.contains("include_bytes!(\"../assets/app-icon.png\")"));
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
