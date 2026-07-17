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
    fn generated_images_are_clamped_to_selected_quality() {
        let source = image::RgbaImage::from_pixel(1254, 1254, image::Rgba([40, 80, 120, 255]));
        let bytes = encode_png_rgba(&source, 1254, 1254).unwrap();
        let (_, _, width, height) = generated_image_from_bytes(&bytes, "1K").unwrap();

        assert_eq!((width, height), (1024, 1024));
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
        let membership = include_str!("../../ui/dialogs/membership-dialog.slint");
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
        assert!(membership.contains("AppState.close-payment-window();"));
        assert!(top_bar.contains("关闭支付码"));

        let combined = format!("{checkout}\n{membership}\n{top_bar}");
        for removed in ["支付宝收银台", "打开支付宝支付", "关闭收银台"] {
            assert!(!combined.contains(removed), "obsolete payment copy: {removed}");
        }
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
