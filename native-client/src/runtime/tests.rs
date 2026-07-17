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

        assert!(page.contains("text: item.success"));
        assert!(page.contains("\"成功说明：\" + item.reason"));
        assert!(page.contains("\"失败原因：\" + item.reason"));
        assert!(page.contains("color: item.success ? AppTheme.success : AppTheme.danger"));
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
        assert!(!membership.contains("purchase-membership-accepted"));
        assert!(!callbacks.contains("accept_agreements"));
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
}
