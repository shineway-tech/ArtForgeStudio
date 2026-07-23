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
        assert!(custom_settings.contains("AppState.custom-prompt-editor-open = true"));
        assert!(custom_settings.contains("for item in AppState.custom-prompt-items"));
        assert!(custom_settings.contains("text: item.time"));
        assert!(custom_settings.contains("assets/icons/edit.svg"));
        assert!(custom_settings.contains("AppState.remove-custom-prompt"));
        assert!(custom_dialog.contains("event.text == Key.Return"));
        assert!(custom_dialog.contains("event.modifiers.alt"));
        assert!(custom_dialog.contains("AppState.save-custom-prompt"));

        assert!(composer.contains("event.text == \"/\" && AppState.prompt == \"/\""));
        assert!(composer.contains("AppState.prompt = \"//\";"));
        let double_slash_handler = composer
            .split("event.text == \"/\" && AppState.prompt == \"/\"")
            .nth(1)
            .and_then(|value| value.split("if event.text == Key.Return").next())
            .expect("double slash handler");
        assert!(double_slash_handler.contains("return accept;"));
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
    }

    #[test]
    fn prompt_popups_show_ten_single_line_previews_without_losing_full_values() {
        assert_eq!(
            single_line_prompt_preview("first line\nsecond\tline  end"),
            "first line second line end"
        );

        let composer = include_str!("../../ui/components/prompt-composer.slint");
        assert_eq!(composer.matches("min(10, AppState.").count(), 2);
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
        assert!(page.contains("AppState.delete-notification(item.id)"));
        assert!(page.contains("AppState.clear-all-notifications()"));
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
        let dialog_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("ui/dialogs/invoice-dialog.slint");
        let dialog = std::fs::read_to_string(dialog_path).unwrap_or_default();

        assert!(credits.contains("申请开票"));
        assert!(credits.contains("callback open-invoice()"));
        assert!(credits.contains("root.open-invoice()"));
        assert!(app.contains("InvoiceDialog"));
        assert!(app.contains("property <bool> invoice-open"));
        assert!(!state.contains("invoice-open"));
        assert!(!state.contains("submit-invoice-request"));
        assert!(dialog.contains("in-out property <bool> open"));
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
}
