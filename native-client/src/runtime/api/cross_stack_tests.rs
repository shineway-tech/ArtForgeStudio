use super::session::test_support::MemoryRefreshTokenStore;
use super::*;
use reqwest::{Method, Url};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::{Arc, Barrier};
use std::time::Duration;
use uuid::Uuid;

const MOCK_PNG: [u8; 68] = [
    137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0,
    0, 1, 8, 4, 0, 0, 0, 181, 28, 12, 2, 0, 0, 0, 11, 73, 68, 65, 84, 120, 218, 99,
    100, 248, 15, 0, 1, 5, 1, 1, 39, 24, 227, 102, 0, 0, 0, 0, 73, 69, 78, 68, 174,
    66, 96, 130,
];

fn base_url() -> Url {
    Url::parse(
        &std::env::var("ARTFORGE_CROSS_STACK_BASE_URL")
            .expect("ARTFORGE_CROSS_STACK_BASE_URL is required"),
    )
    .expect("valid Mock API URL")
}

fn mock_code() -> String {
    std::env::var("ARTFORGE_MOCK_EMAIL_CODE").unwrap_or_else(|_| "654321".to_string())
}

fn new_client_with(device_id: String, app_version: &str) -> ApiClient {
    new_client_identity(
        device_id,
        "Cross-stack test device".to_string(),
        "macos".to_string(),
        app_version,
    )
}

fn new_client_identity(
    device_id: String,
    device_name: String,
    platform: String,
    app_version: &str,
) -> ApiClient {
    ApiClient::new(
        ApiClientConfig {
            base_url: base_url(),
            app_version: app_version.to_string(),
            timeout: Duration::from_secs(10),
        },
        DeviceIdentity {
            id: device_id,
            name: device_name,
            platform,
        },
        Arc::new(SessionManager::new(Arc::new(
            MemoryRefreshTokenStore::default(),
        ))),
    )
    .expect("create frontend API client")
}

fn new_client() -> ApiClient {
    new_client_with(
        format!("cross-stack-{}", Uuid::new_v4()),
        env!("CARGO_PKG_VERSION"),
    )
}

fn agreement_acceptances(auth: &AuthApi) -> Vec<AgreementAcceptance> {
    auth.list_agreements()
        .expect("load agreements")
        .into_iter()
        .map(|agreement| AgreementAcceptance {
            agreement_type: agreement.agreement_type,
            version: agreement.version,
        })
        .collect()
}

fn login_new_user() -> (ApiClient, LoginResponse) {
    let client = new_client();
    let auth = AuthApi::new(client.clone());
    let email = format!("client-stack-{}@example.com", Uuid::new_v4());
    let delivery = auth
        .request_email_code(&email)
        .expect("request Mock email code");
    assert!(delivery.expires_in_seconds > 0);
    assert!(delivery.resend_after_seconds > 0);
    let login = auth
        .login(&email, &mock_code(), &agreement_acceptances(&auth))
        .expect("login through backend");
    assert!(login.is_new_user);
    assert_eq!(login.registration_credit_granted, "500");
    (client, login)
}

fn assert_http_error<T>(result: Result<T, ApiError>, expected_status: u16, expected_code: &str) {
    assert_http_error_field(result, expected_status, expected_code, None);
}

fn assert_trusted_mock_checkout(order: &OrderDetail) {
    let checkout_url = order
        .payment
        .as_ref()
        .and_then(|payment| payment.checkout_url.as_deref())
        .expect("pending payment exposes checkout URL");
    assert!(checkout_url.starts_with("https://openapi.alipay.com/gateway.do?mock_order="));
}

fn assert_http_error_field<T>(
    result: Result<T, ApiError>,
    expected_status: u16,
    expected_code: &str,
    expected_field: Option<&str>,
) {
    match result {
        Err(ApiError::Http {
            status,
            code,
            request_id,
            details,
            ..
        }) => {
            assert_eq!(status, expected_status);
            assert_eq!(code, expected_code);
            assert!(request_id.is_some());
            if let Some(field) = expected_field {
                let fields = details
                    .as_ref()
                    .and_then(Value::as_array)
                    .expect("validation error details array");
                assert!(
                    fields.iter().any(|detail| detail.get("field").and_then(Value::as_str) == Some(field)),
                    "expected validation detail for {field}, got {details:?}"
                );
            }
        }
        Err(error) => panic!("expected HTTP {expected_status} {expected_code}, got {error:?}"),
        Ok(_) => panic!("expected HTTP {expected_status} {expected_code}, got success"),
    }
}

fn assert_raw_problem(
    response: reqwest::blocking::Response,
    expected_status: u16,
    expected_code: &str,
) -> Value {
    assert_eq!(response.status().as_u16(), expected_status);
    let response_request_id = response
        .headers()
        .get("X-Request-ID")
        .expect("error response X-Request-ID")
        .to_str()
        .expect("ASCII response request ID")
        .to_string();
    let body: Value = response.json().expect("JSON error envelope");
    assert_eq!(body["request_id"], response_request_id);
    assert!(body["data"].is_null());
    assert_eq!(body["error"]["code"], expected_code);
    assert!(body["error"]["message"].is_string());
    body
}

fn prompt_request(request_id: String, prompt: &str) -> CreateGenerationTask {
    CreateGenerationTask {
        client_request_id: request_id,
        task_type: "prompt_optimize".to_string(),
        model_code: "openai_prompt".to_string(),
        prompt: prompt.to_string(),
        quality: None,
        count: None,
        aspect_ratio: None,
        reference_file_ids: None,
        target_language: None,
    }
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_happy_path_and_dto_contract() {
    let (client, login) = login_new_user();
    assert!(login.user.nickname.is_none());

    let snapshot = AccountApi::new(client.clone())
        .snapshot()
        .expect("deserialize account snapshot");
    assert_eq!(snapshot.account.user.id, login.user.id);
    assert!(snapshot.account.user.nickname.is_none());
    assert_eq!(
        snapshot
            .account
            .credits
            .as_ref()
            .map(|value| value.available.as_str()),
        Some("500")
    );
    assert!(snapshot.plans.iter().any(|plan| plan.code == "basic"));
    assert!(snapshot.packs.iter().any(|pack| pack.code == "pack_1000"));
    assert!(snapshot
        .models
        .iter()
        .any(|model| model.code == "openai_prompt"));
    assert_eq!(snapshot.sessions.len(), 1);

    let auth = AuthApi::new(client.clone());
    assert!(!auth.refresh().expect("refresh frontend session").is_empty());
    auth.logout(false).expect("logout frontend session");
    assert!(client.session().access_token().is_none());
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_auth_validation_and_error_envelopes() {
    let client = new_client();
    assert_http_error(
        client.public_json::<Value>(
            Method::POST,
            "/v1/auth/email/code",
            Some(json!({ "email": "not-an-email", "app_version": env!("CARGO_PKG_VERSION") })),
        ),
        400,
        "validation_failed",
    );
    assert_http_error(
        client.public_json::<Value>(
            Method::POST,
            "/v1/auth/email/code",
            Some(json!({ "email": "valid@example.com", "app_version": "1.0" })),
        ),
        400,
        "validation_failed",
    );
    assert_http_error(
        client.public_json::<Value>(Method::GET, "/v1/account", None),
        401,
        "authentication_required",
    );

    let auth = AuthApi::new(client.clone());
    let email = format!("auth-matrix-{}@example.com", Uuid::new_v4());
    auth.request_email_code(&email).expect("request Mock code");
    assert_http_error(auth.login(&email, "000000", &agreement_acceptances(&auth)), 400, "email_code_invalid");
    let login = auth
        .login(&email, &mock_code(), &agreement_acceptances(&auth))
        .expect("correct code remains usable after one failed attempt");
    assert_eq!(login.tokens.token_type, "X-Token");
    auth.logout(false).expect("logout auth validation user");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_session_device_binding_and_refresh_replay() {
    let (client, login) = login_new_user();
    let wrong_device = new_client_with(
        format!("wrong-device-{}", Uuid::new_v4()),
        env!("CARGO_PKG_VERSION"),
    );
    wrong_device
        .session()
        .install_tokens(&login.tokens)
        .expect("install tokens on wrong test device");
    assert_http_error(
        wrong_device.authenticated_json::<Value>(Method::GET, "/v1/account", None, None),
        401,
        "session_device_mismatch",
    );
    assert!(wrong_device.session().access_token().is_none());

    assert_http_error(
        client.public_json::<Value>(
            Method::POST,
            "/v1/auth/refresh",
            Some(json!({
                "refresh_token": "short",
                "device_id": client.device().id,
                "app_version": client.app_version(),
            })),
        ),
        400,
        "validation_failed",
    );

    let old_refresh = login.tokens.refresh_token.clone();
    AuthApi::new(client.clone())
        .refresh()
        .expect("rotate refresh token");
    assert_http_error(
        client.public_json::<Value>(
            Method::POST,
            "/v1/auth/refresh",
            Some(json!({
                "refresh_token": old_refresh,
                "device_id": client.device().id,
                "app_version": client.app_version(),
            })),
        ),
        401,
        "refresh_token_reused",
    );
    assert_http_error(
        client.authenticated_json::<Value>(Method::GET, "/v1/account", None, None),
        401,
        "session_invalid",
    );
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_account_catalog_and_pagination_parameters() {
    let (client, _) = login_new_user();
    let snapshot = AccountApi::new(client.clone())
        .snapshot()
        .expect("load full account snapshot");
    assert!(snapshot.plans.len() >= 4);
    assert!(snapshot.packs.len() >= 4);
    assert!(snapshot.models.len() >= 2);
    assert!(snapshot.ledger.iter().any(|entry| entry.available_delta == "500"));

    for path in [
        "/v1/credits/ledger?limit=0",
        "/v1/credits/ledger?limit=101",
        "/v1/credits/ledger?cursor=abc",
        "/v1/notifications?limit=0",
        "/v1/notifications?limit=101",
        "/v1/notifications?cursor=abc",
        "/v1/notifications?unread_only=not-a-boolean",
    ] {
        assert_http_error(
            client.authenticated_json::<Value>(Method::GET, path, None, None),
            400,
            "validation_failed",
        );
    }

    let ledger = client
        .authenticated_json::<Vec<CreditLedgerItem>>(
            Method::GET,
            "/v1/credits/ledger?limit=1",
            None,
            None,
        )
        .expect("valid ledger page");
    assert_eq!(ledger.data.len(), 1);
    assert!(ledger.meta.is_some());
    AuthApi::new(client).logout(false).expect("logout account test");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_generation_parameter_matrix_and_idempotency() {
    let (client, _) = login_new_user();
    let generation = GenerationApi::new(client.clone());
    let valid_id = || format!("task_{}", Uuid::new_v4().simple());

    let invalid_requests = [
        CreateGenerationTask {
            client_request_id: "short".to_string(),
            ..prompt_request("short".to_string(), "prompt")
        },
        CreateGenerationTask {
            client_request_id: valid_id(),
            prompt: "".to_string(),
            ..prompt_request(valid_id(), "unused")
        },
        CreateGenerationTask {
            client_request_id: valid_id(),
            task_type: "unknown".to_string(),
            ..prompt_request(valid_id(), "prompt")
        },
        CreateGenerationTask {
            client_request_id: valid_id(),
            quality: Some("1K".to_string()),
            ..prompt_request(valid_id(), "prompt")
        },
        CreateGenerationTask {
            client_request_id: valid_id(),
            task_type: "prompt_translate".to_string(),
            ..prompt_request(valid_id(), "translate")
        },
        CreateGenerationTask {
            client_request_id: valid_id(),
            task_type: "image_generation".to_string(),
            model_code: "openai_image".to_string(),
            quality: None,
            count: None,
            aspect_ratio: Some("square".to_string()),
            ..prompt_request(valid_id(), "image")
        },
        CreateGenerationTask {
            client_request_id: valid_id(),
            task_type: "image_generation".to_string(),
            model_code: "openai_image".to_string(),
            quality: Some("8K".to_string()),
            count: Some(1),
            aspect_ratio: Some("square".to_string()),
            ..prompt_request(valid_id(), "image")
        },
        CreateGenerationTask {
            client_request_id: valid_id(),
            task_type: "image_generation".to_string(),
            model_code: "openai_image".to_string(),
            quality: Some("1K".to_string()),
            count: Some(0),
            aspect_ratio: Some("square".to_string()),
            ..prompt_request(valid_id(), "image")
        },
        CreateGenerationTask {
            client_request_id: valid_id(),
            task_type: "image_generation".to_string(),
            model_code: "openai_image".to_string(),
            quality: Some("1K".to_string()),
            count: Some(5),
            aspect_ratio: Some("16:9".to_string()),
            ..prompt_request(valid_id(), "image")
        },
        CreateGenerationTask {
            client_request_id: valid_id(),
            task_type: "image_generation".to_string(),
            model_code: "openai_image".to_string(),
            quality: Some("1K".to_string()),
            count: Some(1),
            aspect_ratio: Some("square".to_string()),
            reference_file_ids: Some((0..9).map(|_| Uuid::new_v4().to_string()).collect()),
            ..prompt_request(valid_id(), "image")
        },
    ];
    for request in invalid_requests {
        assert_http_error(generation.create_task(&request), 400, "validation_failed");
    }

    let unavailable = CreateGenerationTask {
        client_request_id: valid_id(),
        model_code: "missing_model".to_string(),
        ..prompt_request(valid_id(), "prompt")
    };
    assert_http_error(generation.create_task(&unavailable), 409, "model_unavailable");

    let image_request = CreateGenerationTask {
        client_request_id: valid_id(),
        task_type: "image_generation".to_string(),
        model_code: "openai_image".to_string(),
        quality: Some("1K".to_string()),
        count: Some(4),
        aspect_ratio: Some("landscape".to_string()),
        reference_file_ids: Some(Vec::new()),
        ..prompt_request(valid_id(), "valid image")
    };
    let image_task = generation
        .create_task(&image_request)
        .expect("valid image task");
    assert_eq!(image_task.requested_count, 4);
    generation.cancel(&image_task.id).expect("cancel valid image task");

    let request_id = valid_id();
    let original = prompt_request(request_id.clone(), "same prompt");
    let first = generation.create_task(&original).expect("create prompt task");
    let replay = generation.create_task(&original).expect("replay prompt task");
    assert_eq!(replay.id, first.id);
    let conflict = prompt_request(request_id, "different prompt");
    assert_http_error(
        generation.create_task(&conflict),
        409,
        "idempotency_key_conflict",
    );
    assert_http_error(
        generation.list_tasks("invalid-status"),
        400,
        "validation_failed",
    );
    assert_http_error(
        generation.task(&Uuid::new_v4().to_string()),
        404,
        "generation_task_not_found",
    );
    generation.cancel(&first.id).expect("cancel prompt task");
    AuthApi::new(client).logout(false).expect("logout generation test");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_payment_parameter_matrix_and_idempotency() {
    let (client, _) = login_new_user();
    let payment = PaymentApi::new(client.clone());
    let packs = payment.packs().expect("load packs");
    let first_pack = packs.first().expect("at least one pack");
    let other_pack = packs
        .iter()
        .find(|pack| pack.code != first_pack.code)
        .expect("at least two packs");

    assert_http_error(
        payment.create_credit_order("BAD-PACK", &format!("credit_{}", Uuid::new_v4().simple())),
        400,
        "validation_failed",
    );
    assert_http_error(
        payment.create_credit_order("missing_pack", &format!("credit_{}", Uuid::new_v4().simple())),
        404,
        "credit_pack_unavailable",
    );
    assert_http_error(
        payment.create_credit_order(&first_pack.code, "short"),
        400,
        "validation_failed",
    );

    let request_id = format!("credit_{}", Uuid::new_v4().simple());
    let order = payment
        .create_credit_order(&first_pack.code, &request_id)
        .expect("create credit order");
    assert_eq!(order.status, "pending_payment");
    assert_trusted_mock_checkout(&order);
    assert!(order.payable_amount_cents.parse::<u64>().is_ok());
    assert_eq!(
        payment
            .create_credit_order(&first_pack.code, &request_id)
            .expect("replay credit order")
            .id,
        order.id
    );
    assert_http_error(
        payment.create_credit_order(&other_pack.code, &request_id),
        409,
        "idempotency_key_conflict",
    );
    assert_eq!(
        payment.sync_order(&order.id).expect("sync pending order").status,
        "pending_payment"
    );
    assert_http_error(
        payment.order(&Uuid::new_v4().to_string()),
        404,
        "order_not_found",
    );

    let membership = MembershipApi::new(client.clone());
    assert_http_error(
        membership.create_order("BAD", &format!("member_{}", Uuid::new_v4().simple())),
        400,
        "validation_failed",
    );
    assert_http_error(
        membership.create_order(
            "missing_plan",
            &format!("member_{}", Uuid::new_v4().simple()),
        ),
        404,
        "membership_plan_unavailable",
    );
    let membership_request_id = format!("member_{}", Uuid::new_v4().simple());
    let membership_order = membership
        .create_order("basic", &membership_request_id)
        .expect("create membership order");
    assert_eq!(membership_order.status, "pending_payment");
    assert_trusted_mock_checkout(&membership_order);
    assert_eq!(
        membership
            .create_order("basic", &membership_request_id)
            .expect("replay membership order")
            .id,
        membership_order.id
    );
    assert_http_error(
        membership.create_upgrade_quote("advanced"),
        409,
        "membership_missing",
    );
    AuthApi::new(client).logout(false).expect("logout payment test");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_reference_upload_and_notification_parameters() {
    let (client, _) = login_new_user();
    for body in [
        json!({ "filename": "", "mime_type": "image/png", "size_bytes": 68 }),
        json!({ "filename": "test.gif", "mime_type": "image/gif", "size_bytes": 68 }),
        json!({ "filename": "test.png", "mime_type": "image/png", "size_bytes": 0 }),
    ] {
        assert_http_error(
            client.authenticated_json::<Value>(
                Method::POST,
                "/v1/uploads/references",
                Some(body),
                None,
            ),
            400,
            "validation_failed",
        );
    }
    assert_http_error(
        client.authenticated_json::<Value>(
            Method::POST,
            "/v1/uploads/references",
            Some(json!({
                "filename": "large.png",
                "mime_type": "image/png",
                "size_bytes": 10_485_761,
            })),
            None,
        ),
        413,
        "reference_image_too_large",
    );

    let path = std::env::temp_dir().join(format!("artforge-cross-stack-{}.png", Uuid::new_v4()));
    std::fs::write(&path, MOCK_PNG).expect("write Mock reference image");
    let generation = GenerationApi::new(client.clone());
    let file_id = generation
        .upload_reference(&path)
        .expect("multipart reference upload");
    let _ = std::fs::remove_file(&path);
    generation.delete_reference(&file_id);

    let notifications = NotificationsApi::new(client.clone());
    assert!(notifications.list().expect("empty notifications").is_empty());
    notifications.mark_all_read().expect("mark empty list read");
    assert_http_error(
        notifications.mark_read(&Uuid::new_v4().to_string()),
        404,
        "notification_not_found",
    );
    AuthApi::new(client).logout(false).expect("logout upload test");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_email_code_fields_report_exact_validation_details() {
    let client = new_client();
    for (body, field) in [
        (
            json!({ "email": "not-an-email", "app_version": env!("CARGO_PKG_VERSION") }),
            "email",
        ),
        (
            json!({ "email": format!("{}@example.com", "a".repeat(250)), "app_version": env!("CARGO_PKG_VERSION") }),
            "email",
        ),
        (json!({ "email": "valid@example.com", "app_version": "1.0" }), "app_version"),
        (json!({ "email": "valid@example.com" }), "app_version"),
    ] {
        assert_http_error_field(
            client.public_json::<Value>(Method::POST, "/v1/auth/email/code", Some(body)),
            400,
            "validation_failed",
            Some(field),
        );
    }
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_login_fields_report_exact_validation_details() {
    let client = new_client();
    let auth = AuthApi::new(client.clone());
    let email = format!("login-fields-{}@example.com", Uuid::new_v4());
    auth.request_email_code(&email).expect("request login field code");
    let device_id = format!("device-{}", Uuid::new_v4());
    let valid_body = || {
        json!({
            "email": email,
            "code": mock_code(),
            "device_id": device_id,
            "device_name": "field test",
            "platform": "macos",
            "app_version": env!("CARGO_PKG_VERSION"),
            "agreement_acceptances": [],
        })
    };
    let mut cases = Vec::new();
    let mut body = valid_body();
    body["code"] = json!("12345");
    cases.push((body, "code"));
    let mut body = valid_body();
    body["device_id"] = json!("short");
    cases.push((body, "device_id"));
    let mut body = valid_body();
    body["device_name"] = json!("x".repeat(129));
    cases.push((body, "device_name"));
    let mut body = valid_body();
    body["platform"] = json!("linux");
    cases.push((body, "platform"));
    let mut body = valid_body();
    body["app_version"] = json!("0.1");
    cases.push((body, "app_version"));
    for (body, field) in cases {
        assert_http_error_field(
            client.public_json::<Value>(Method::POST, "/v1/auth/email/login", Some(body)),
            400,
            "validation_failed",
            Some(field),
        );
    }
    auth.login(&email, &mock_code(), &[])
        .expect("valid login after validation cases");
    auth.logout(false).expect("logout login field test");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_agreement_acceptance_fields_and_duplicates() {
    let (client, _) = login_new_user();
    assert_http_error_field(
        client.authenticated_json::<Value>(
            Method::POST,
            "/v1/agreements/accept",
            Some(json!({ "agreements": [] })),
            None,
        ),
        400,
        "validation_failed",
        Some("agreements"),
    );
    assert_http_error_field(
        client.authenticated_json::<Value>(
            Method::POST,
            "/v1/agreements/accept",
            Some(json!({ "agreements": [{ "type": "unknown", "version": "1" }] })),
            None,
        ),
        400,
        "validation_failed",
        Some("agreements.0.type"),
    );
    assert_http_error(
        client.authenticated_json::<Value>(
            Method::POST,
            "/v1/agreements/accept",
            Some(json!({
                "agreements": [
                    { "type": "user_terms", "version": "1" },
                    { "type": "user_terms", "version": "1" }
                ]
            })),
            None,
        ),
        400,
        "validation_failed",
    );
    AuthApi::new(client).logout(false).expect("logout agreement test");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_prompt_task_fields_report_exact_validation_details() {
    let (client, _) = login_new_user();
    let generation = GenerationApi::new(client.clone());
    let task_id = || format!("prompt_{}", Uuid::new_v4().simple());
    let cases = [
        (
            CreateGenerationTask {
                client_request_id: "1234567".to_string(),
                ..prompt_request(task_id(), "prompt")
            },
            "client_request_id",
        ),
        (
            CreateGenerationTask {
                client_request_id: "x".repeat(65),
                ..prompt_request(task_id(), "prompt")
            },
            "client_request_id",
        ),
        (
            CreateGenerationTask {
                client_request_id: task_id(),
                model_code: "OpenAI".to_string(),
                ..prompt_request(task_id(), "prompt")
            },
            "model_code",
        ),
        (
            CreateGenerationTask {
                client_request_id: task_id(),
                task_type: "unknown".to_string(),
                ..prompt_request(task_id(), "prompt")
            },
            "task_type",
        ),
        (
            CreateGenerationTask {
                client_request_id: task_id(),
                prompt: "".to_string(),
                ..prompt_request(task_id(), "unused")
            },
            "prompt",
        ),
        (
            CreateGenerationTask {
                client_request_id: task_id(),
                prompt: "p".repeat(10_001),
                ..prompt_request(task_id(), "unused")
            },
            "prompt",
        ),
        (
            CreateGenerationTask {
                client_request_id: task_id(),
                quality: Some("1K".to_string()),
                ..prompt_request(task_id(), "prompt")
            },
            "quality",
        ),
        (
            CreateGenerationTask {
                client_request_id: task_id(),
                task_type: "prompt_translate".to_string(),
                ..prompt_request(task_id(), "translate")
            },
            "target_language",
        ),
        (
            CreateGenerationTask {
                client_request_id: task_id(),
                task_type: "prompt_translate".to_string(),
                target_language: Some("x".to_string()),
                ..prompt_request(task_id(), "translate")
            },
            "target_language",
        ),
        (
            CreateGenerationTask {
                client_request_id: task_id(),
                task_type: "prompt_translate".to_string(),
                target_language: Some("x".repeat(65)),
                ..prompt_request(task_id(), "translate")
            },
            "target_language",
        ),
    ];
    for (request, field) in cases {
        assert_http_error_field(
            generation.create_task(&request),
            400,
            "validation_failed",
            Some(field),
        );
    }
    AuthApi::new(client).logout(false).expect("logout prompt field test");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_image_task_fields_report_exact_validation_details() {
    let (client, _) = login_new_user();
    let generation = GenerationApi::new(client.clone());
    let task_id = || format!("image_{}", Uuid::new_v4().simple());
    let image_request = || CreateGenerationTask {
        client_request_id: task_id(),
        task_type: "image_generation".to_string(),
        model_code: "openai_image".to_string(),
        prompt: "image".to_string(),
        quality: Some("1K".to_string()),
        count: Some(1),
        aspect_ratio: Some("square".to_string()),
        reference_file_ids: Some(Vec::new()),
        target_language: None,
    };
    let mut cases = Vec::new();
    let mut request = image_request();
    request.quality = None;
    cases.push((request, "quality"));
    let mut request = image_request();
    request.count = None;
    cases.push((request, "count"));
    let mut request = image_request();
    request.quality = Some("8K".to_string());
    cases.push((request, "quality"));
    let mut request = image_request();
    request.count = Some(0);
    cases.push((request, "count"));
    let mut request = image_request();
    request.count = Some(5);
    cases.push((request, "count"));
    let mut request = image_request();
    request.aspect_ratio = Some("7:5".to_string());
    cases.push((request, "aspect_ratio"));
    let mut request = image_request();
    request.reference_file_ids = Some((0..9).map(|_| Uuid::new_v4().to_string()).collect());
    cases.push((request, "reference_file_ids"));
    let mut request = image_request();
    request.reference_file_ids = Some(vec!["not-a-uuid".to_string()]);
    cases.push((request, "reference_file_ids.0"));
    for (request, field) in cases {
        assert_http_error_field(
            generation.create_task(&request),
            400,
            "validation_failed",
            Some(field),
        );
    }
    let duplicate = Uuid::new_v4().to_string();
    let mut request = image_request();
    request.reference_file_ids = Some(vec![duplicate.clone(), duplicate]);
    assert_http_error(generation.create_task(&request), 400, "validation_failed");
    let mut request = image_request();
    request.quality = Some("2K".to_string());
    assert_http_error(
        generation.create_task(&request),
        403,
        "membership_quality_forbidden",
    );
    AuthApi::new(client).logout(false).expect("logout image field test");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_generation_success_variants_and_credit_reservation_limit() {
    let (client, _) = login_new_user();
    let generation = GenerationApi::new(client.clone());
    let translate = CreateGenerationTask {
        client_request_id: format!("translate_{}", Uuid::new_v4().simple()),
        task_type: "prompt_translate".to_string(),
        model_code: "openai_prompt".to_string(),
        prompt: "translate this".to_string(),
        quality: None,
        count: None,
        aspect_ratio: None,
        reference_file_ids: None,
        target_language: Some("English".to_string()),
    };
    let translate_task = generation.create_task(&translate).expect("valid translate task");
    generation.cancel(&translate_task.id).expect("cancel translate task");

    for (ratio, normalized, width, height) in [
        ("1:1", "1:1", 1024, 1024),
        ("3:2", "3:2", 1024, 680),
        ("2:3", "2:3", 680, 1024),
        ("4:3", "4:3", 1024, 768),
        ("3:4", "3:4", 768, 1024),
        ("5:4", "5:4", 1024, 816),
        ("4:5", "4:5", 816, 1024),
        ("16:9", "16:9", 1024, 576),
        ("9:16", "9:16", 576, 1024),
        ("2:1", "2:1", 1024, 512),
        ("1:2", "1:2", 512, 1024),
        ("21:9", "21:9", 1024, 440),
        ("9:21", "9:21", 440, 1024),
        ("square", "1:1", 1024, 1024),
        ("landscape", "3:2", 1024, 680),
        ("portrait", "2:3", 680, 1024),
    ] {
        let request = CreateGenerationTask {
            client_request_id: format!("ratio_{}", Uuid::new_v4().simple()),
            task_type: "image_generation".to_string(),
            model_code: "openai_image".to_string(),
            prompt: format!("valid {ratio} image"),
            quality: Some("1K".to_string()),
            count: Some(1),
            aspect_ratio: Some(ratio.to_string()),
            reference_file_ids: Some(Vec::new()),
            target_language: None,
        };
        let task = generation.create_task(&request).expect("valid ratio task");
        assert_eq!(task.request["aspect_ratio"], normalized);
        assert_eq!(task.request["target_width"], width);
        assert_eq!(task.request["target_height"], height);
        assert_eq!(task.request["provider_size"], format!("{width}x{height}"));
        generation.cancel(&task.id).expect("cancel ratio task");
    }

    let four_images = |label: &str| CreateGenerationTask {
        client_request_id: format!("{label}_{}", Uuid::new_v4().simple()),
        task_type: "image_generation".to_string(),
        model_code: "openai_image".to_string(),
        prompt: label.to_string(),
        quality: Some("1K".to_string()),
        count: Some(4),
        aspect_ratio: Some("square".to_string()),
        reference_file_ids: Some(Vec::new()),
        target_language: None,
    };
    let first = generation.create_task(&four_images("reserve_a")).expect("reserve 200 credits A");
    let second = generation.create_task(&four_images("reserve_b")).expect("reserve 200 credits B");
    assert_http_error(
        generation.create_task(&four_images("reserve_c")),
        409,
        "insufficient_credits",
    );
    generation.cancel(&first.id).expect("release reservation A");
    generation.cancel(&second.id).expect("release reservation B");
    let credits = client
        .authenticated_json::<CreditAccount>(Method::GET, "/v1/credits/account", None, None)
        .expect("load credits after cancellation")
        .data;
    assert_eq!(credits.available, "500");
    assert_eq!(credits.reserved, "0");
    AuthApi::new(client).logout(false).expect("logout generation success test");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_generation_queue_limit_is_exactly_twenty() {
    let (client, _) = login_new_user();
    let generation = GenerationApi::new(client.clone());
    let mut task_ids = Vec::new();
    for index in 0..20 {
        let request = prompt_request(
            format!("queue_{index}_{}", Uuid::new_v4().simple()),
            &format!("queued prompt {index}"),
        );
        task_ids.push(generation.create_task(&request).expect("queue task").id);
    }
    assert_eq!(generation.list_tasks("queued").expect("list queued tasks").len(), 20);
    assert_http_error(
        generation.create_task(&prompt_request(
            format!("queue_over_{}", Uuid::new_v4().simple()),
            "one task too many",
        )),
        429,
        "generation_queue_limit_reached",
    );
    for task_id in task_ids {
        generation.cancel(&task_id).expect("cancel queued task");
    }
    AuthApi::new(client).logout(false).expect("logout queue limit test");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_order_request_id_boundaries_report_exact_fields() {
    let (client, _) = login_new_user();
    let payment = PaymentApi::new(client.clone());
    for request_id in ["1234567".to_string(), "x".repeat(65)] {
        assert_http_error_field(
            payment.create_credit_order("pack_1000", &request_id),
            400,
            "validation_failed",
            Some("client_request_id"),
        );
        assert_http_error_field(
            MembershipApi::new(client.clone()).create_order("basic", &request_id),
            400,
            "validation_failed",
            Some("client_request_id"),
        );
    }
    assert_http_error_field(
        payment.create_credit_order("BAD-PACK", &format!("credit_{}", Uuid::new_v4().simple())),
        400,
        "validation_failed",
        Some("pack_code"),
    );
    assert_http_error_field(
        MembershipApi::new(client.clone())
            .create_order("BAD-PLAN", &format!("member_{}", Uuid::new_v4().simple())),
        400,
        "validation_failed",
        Some("plan_code"),
    );
    AuthApi::new(client).logout(false).expect("logout order boundary test");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_upgrade_and_order_identifier_boundaries() {
    let (client, _) = login_new_user();
    let membership = MembershipApi::new(client.clone());
    assert_http_error_field(
        membership.create_upgrade_quote("BAD-PLAN"),
        400,
        "validation_failed",
        Some("target_plan_code"),
    );
    assert_http_error(
        membership.create_upgrade_quote("free"),
        404,
        "membership_plan_unavailable",
    );
    assert_http_error_field(
        membership.create_upgrade_order("not-a-uuid", "upgrade_12345678"),
        400,
        "validation_failed",
        Some("quote_id"),
    );
    assert_http_error(
        membership.create_upgrade_order(&Uuid::new_v4().to_string(), "upgrade_12345678"),
        409,
        "upgrade_quote_unavailable",
    );
    assert_http_error(
        client.authenticated_json::<Value>(Method::GET, "/v1/orders/not-a-uuid", None, None),
        400,
        "validation_failed",
    );
    assert_http_error(
        AccountApi::new(client.clone()).revoke_session(&Uuid::new_v4().to_string()),
        404,
        "session_not_found",
    );
    AuthApi::new(client).logout(false).expect("logout identifier test");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_upload_filename_and_size_boundaries_report_exact_fields() {
    let (client, _) = login_new_user();
    let cases = [
        (
            json!({ "filename": "", "mime_type": "image/png", "size_bytes": 68 }),
            "filename",
        ),
        (
            json!({ "filename": "x".repeat(256), "mime_type": "image/png", "size_bytes": 68 }),
            "filename",
        ),
        (
            json!({ "filename": "test.gif", "mime_type": "image/gif", "size_bytes": 68 }),
            "mime_type",
        ),
        (
            json!({ "filename": "test.png", "mime_type": "image/png", "size_bytes": 0 }),
            "size_bytes",
        ),
    ];
    for (body, field) in cases {
        assert_http_error_field(
            client.authenticated_json::<Value>(
                Method::POST,
                "/v1/uploads/references",
                Some(body),
                None,
            ),
            400,
            "validation_failed",
            Some(field),
        );
    }
    assert_http_error(
        client.authenticated_json::<Value>(
            Method::POST,
            "/v1/uploads/references",
            Some(json!({
                "filename": "large.png",
                "mime_type": "image/png",
                "size_bytes": 10_485_761,
            })),
            None,
        ),
        413,
        "reference_image_too_large",
    );
    AuthApi::new(client).logout(false).expect("logout upload boundary test");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_auth_required_fields_and_minimum_accepted_boundaries() {
    let client = new_client();
    assert_http_error_field(
        client.public_json::<Value>(
            Method::POST,
            "/v1/auth/email/code",
            Some(json!({ "app_version": env!("CARGO_PKG_VERSION") })),
        ),
        400,
        "validation_failed",
        Some("email"),
    );

    let email = format!("required-fields-{}@example.com", Uuid::new_v4());
    let auth = AuthApi::new(client.clone());
    auth.request_email_code(&email).expect("request required field code");
    let valid_login = json!({
        "email": email,
        "code": mock_code(),
        "device_id": "12345678",
        "device_name": "",
        "platform": "windows",
        "app_version": env!("CARGO_PKG_VERSION"),
        "agreement_acceptances": agreement_acceptances(&auth),
    });
    for field in ["email", "code", "device_id", "platform", "app_version"] {
        let mut body = valid_login.clone();
        body.as_object_mut().expect("login object").remove(field);
        assert_http_error_field(
            client.public_json::<Value>(Method::POST, "/v1/auth/email/login", Some(body)),
            400,
            "validation_failed",
            Some(field),
        );
    }

    let min_client = new_client_identity(
        "12345678".to_string(),
        String::new(),
        "windows".to_string(),
        env!("CARGO_PKG_VERSION"),
    );
    AuthApi::new(min_client.clone())
        .login(&email, &mock_code(), &agreement_acceptances(&AuthApi::new(min_client.clone())))
        .expect("minimum device and empty optional name are accepted");
    let raw_sessions = min_client
        .authenticated_json::<Value>(Method::GET, "/v1/account/sessions", None, None)
        .expect("load raw minimum boundary sessions")
        .data;
    assert!(
        raw_sessions["items"][0]["device_name"].is_string(),
        "account session device_name must follow the OpenAPI string contract, got {}",
        raw_sessions["items"][0]["device_name"]
    );
    let min_session = AccountApi::new(min_client.clone())
        .snapshot()
        .expect("minimum boundary snapshot")
        .sessions
        .into_iter()
        .find(|session| session.is_current)
        .expect("current minimum boundary session");
    assert_eq!(min_session.device_name, "");
    assert_eq!(min_session.platform, "windows");
    AuthApi::new(min_client).logout(false).expect("logout minimum boundary user");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_auth_maximum_boundaries_and_email_normalization() {
    let max_client = new_client_identity(
        "d".repeat(256),
        "n".repeat(128),
        "macos".to_string(),
        env!("CARGO_PKG_VERSION"),
    );
    let max_auth = AuthApi::new(max_client.clone());
    let normalized_email = format!("  Boundary-{}@Example.COM  ", Uuid::new_v4());
    max_auth.request_email_code(&normalized_email).expect("request normalized email code");
    max_auth
        .login(&normalized_email, &mock_code(), &agreement_acceptances(&max_auth))
        .expect("maximum device and name lengths are accepted");
    let max_session = AccountApi::new(max_client.clone())
        .snapshot()
        .expect("maximum boundary snapshot")
        .sessions
        .into_iter()
        .find(|session| session.is_current)
        .expect("current maximum boundary session");
    assert_eq!(max_session.device_name.chars().count(), 128);
    assert_eq!(max_session.platform, "macos");
    AuthApi::new(max_client).logout(false).expect("logout maximum boundary user");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_refresh_required_fields_and_authenticated_client_version() {
    let (client, login) = login_new_user();
    let valid_refresh = json!({
        "refresh_token": login.tokens.refresh_token,
        "device_id": client.device().id,
        "app_version": client.app_version(),
    });
    for field in ["refresh_token", "device_id", "app_version"] {
        let mut body = valid_refresh.clone();
        body.as_object_mut().expect("refresh object").remove(field);
        assert_http_error_field(
            client.public_json::<Value>(Method::POST, "/v1/auth/refresh", Some(body)),
            400,
            "validation_failed",
            Some(field),
        );
    }
    for (value, field) in [
        (json!("1234567"), "device_id"),
        (json!("d".repeat(257)), "device_id"),
        (json!("1.0"), "app_version"),
    ] {
        let mut body = valid_refresh.clone();
        body[field] = value;
        assert_http_error_field(
            client.public_json::<Value>(Method::POST, "/v1/auth/refresh", Some(body)),
            400,
            "validation_failed",
            Some(field),
        );
    }

    let invalid_version = new_client_with(client.device().id.clone(), "1.0");
    invalid_version
        .session()
        .install_tokens(&login.tokens)
        .expect("install tokens for malformed version test");
    assert_http_error(
        invalid_version.authenticated_json::<Value>(Method::GET, "/v1/account", None, None),
        400,
        "client_version_invalid",
    );
    let dev_minimum = new_client_with(client.device().id.clone(), "0.0.0");
    dev_minimum
        .session()
        .install_tokens(&login.tokens)
        .expect("install tokens for dev minimum version test");
    assert!(!dev_minimum
        .authenticated_json::<Value>(Method::GET, "/v1/account", None, None)
        .expect("dev minimum client version is accepted")
        .request_id
        .is_empty());
    AuthApi::new(client).logout(false).expect("logout refresh field user");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_email_code_cooldown_and_attempt_exhaustion() {
    let client = new_client();
    let auth = AuthApi::new(client.clone());
    let cooldown_email = format!("cooldown-{}@example.com", Uuid::new_v4());
    auth.request_email_code(&cooldown_email).expect("first code request");
    assert_http_error(
        auth.request_email_code(&cooldown_email),
        429,
        "email_code_cooldown",
    );

    let attempts_email = format!("attempts-{}@example.com", Uuid::new_v4());
    auth.request_email_code(&attempts_email).expect("request attempt limit code");
    let agreements = agreement_acceptances(&auth);
    for _ in 0..4 {
        assert_http_error(
            auth.login(&attempts_email, "000000", &agreements),
            400,
            "email_code_invalid",
        );
    }
    assert_http_error(
        auth.login(&attempts_email, "000000", &agreements),
        400,
        "email_code_attempts_exceeded",
    );
    assert_http_error(
        auth.login(&attempts_email, &mock_code(), &agreements),
        400,
        "email_code_invalid",
    );
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_agreement_item_required_fields_and_version_boundaries() {
    let (client, _) = login_new_user();
    for (agreement, field) in [
        (json!({ "version": "1" }), "agreements.0.type"),
        (json!({ "type": "user_terms" }), "agreements.0.version"),
        (json!({ "type": "user_terms", "version": "" }), "agreements.0.version"),
        (
            json!({ "type": "user_terms", "version": "v".repeat(33) }),
            "agreements.0.version",
        ),
    ] {
        assert_http_error_field(
            client.authenticated_json::<Value>(
                Method::POST,
                "/v1/agreements/accept",
                Some(json!({ "agreements": [agreement] })),
                None,
            ),
            400,
            "validation_failed",
            Some(field),
        );
    }
    let current = agreement_acceptances(&AuthApi::new(client.clone()));
    AuthApi::new(client.clone())
        .accept_agreements(&current)
        .expect("re-accept current agreements once");
    AuthApi::new(client.clone())
        .accept_agreements(&current)
        .expect("re-accept current agreements idempotently");
    AuthApi::new(client).logout(false).expect("logout agreement item test");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_generation_required_fields_and_forbidden_combinations() {
    let (client, _) = login_new_user();
    let valid = serde_json::to_value(prompt_request("required1".to_string(), "prompt"))
        .expect("serialize valid prompt request");
    for field in ["client_request_id", "task_type", "model_code", "prompt"] {
        let mut body = valid.clone();
        body.as_object_mut().expect("task body object").remove(field);
        assert_http_error_field(
            client.authenticated_json::<Value>(
                Method::POST,
                "/v1/generation/tasks",
                Some(body),
                Some("required_header_1"),
            ),
            400,
            "validation_failed",
            Some(field),
        );
    }
    for (field, value) in [
        ("count", json!(1)),
        ("aspect_ratio", json!("square")),
        ("reference_file_ids", json!([])),
        ("target_language", json!("English")),
    ] {
        let mut body = valid.clone();
        body[field] = value;
        assert_http_error_field(
            client.authenticated_json::<Value>(
                Method::POST,
                "/v1/generation/tasks",
                Some(body),
                Some("forbidden_header_1"),
            ),
            400,
            "validation_failed",
            Some(field),
        );
    }
    AuthApi::new(client).logout(false).expect("logout generation required field test");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_generation_exact_valid_boundaries_defaults_and_header_idempotency() {
    let (client, _) = login_new_user();
    let generation = GenerationApi::new(client.clone());
    let boundary_requests = [
        prompt_request("12345678".to_string(), "p"),
        prompt_request("x".repeat(64), &"p".repeat(10_000)),
        CreateGenerationTask {
            client_request_id: format!("language_min_{}", Uuid::new_v4().simple()),
            task_type: "prompt_translate".to_string(),
            model_code: "openai_prompt".to_string(),
            prompt: "translate".to_string(),
            quality: None,
            count: None,
            aspect_ratio: None,
            reference_file_ids: None,
            target_language: Some("zh".to_string()),
        },
        CreateGenerationTask {
            client_request_id: format!("language_max_{}", Uuid::new_v4().simple()),
            task_type: "prompt_translate".to_string(),
            model_code: "openai_prompt".to_string(),
            prompt: "translate".to_string(),
            quality: None,
            count: None,
            aspect_ratio: None,
            reference_file_ids: None,
            target_language: Some("l".repeat(64)),
        },
    ];
    let mut task_ids = Vec::new();
    for request in boundary_requests {
        task_ids.push(generation.create_task(&request).expect("accepted generation boundary").id);
    }
    let image_default = json!({
        "client_request_id": format!("defaults_{}", Uuid::new_v4().simple()),
        "task_type": "image_generation",
        "model_code": "openai_image",
        "prompt": "default image fields",
        "quality": "1K",
        "count": 1,
    });
    let default_task = client
        .authenticated_json::<GenerationTaskDetail>(
            Method::POST,
            "/v1/generation/tasks",
            Some(image_default.clone()),
            Some(image_default["client_request_id"].as_str().expect("default request id")),
        )
        .expect("image defaults are accepted")
        .data;
    assert_eq!(default_task.request["aspect_ratio"], "1:1");
    assert_eq!(default_task.request["provider_size"], "1024x1024");
    assert_eq!(default_task.request["reference_file_ids"], json!([]));
    task_ids.push(default_task.id);

    let header_body = serde_json::to_value(prompt_request(
        format!("header_body_{}", Uuid::new_v4().simple()),
        "header boundary",
    ))
    .expect("serialize header task");
    for header in ["1234567".to_string(), "h".repeat(129)] {
        assert_http_error(
            client.authenticated_json::<Value>(
                Method::POST,
                "/v1/generation/tasks",
                Some(header_body.clone()),
                Some(&header),
            ),
            400,
            "idempotency_key_required",
        );
    }
    let header_task = client
        .authenticated_json::<GenerationTaskDetail>(
            Method::POST,
            "/v1/generation/tasks",
            Some(header_body),
            Some(&"h".repeat(128)),
        )
        .expect("128 character idempotency header is accepted")
        .data;
    task_ids.push(header_task.id);
    for task_id in task_ids {
        generation.cancel(&task_id).expect("cancel accepted boundary task");
    }
    AuthApi::new(client).logout(false).expect("logout generation boundary test");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_generation_list_cancel_purge_and_delivery_state_matrix() {
    let (client, _) = login_new_user();
    let generation = GenerationApi::new(client.clone());
    let task = generation
        .create_task(&prompt_request(
            format!("state_{}", Uuid::new_v4().simple()),
            "state matrix",
        ))
        .expect("create state task");
    for query in [
        "limit=1",
        "limit=100",
        "limit=20&cursor=0",
        "status=queued",
        "status=processing",
        "status=completed",
        "status=partially_completed",
        "status=failed",
        "status=cancelled",
    ] {
        assert!(!client
            .authenticated_json::<Value>(
                Method::GET,
                &format!("/v1/generation/tasks?{query}"),
                None,
                None,
            )
            .expect("valid generation list query")
            .request_id
            .is_empty());
    }
    assert_http_error(
        client.authenticated_json::<Value>(
            Method::DELETE,
            &format!("/v1/generation/tasks/{}/content", task.id),
            None,
            None,
        ),
        409,
        "generation_task_active",
    );
    generation.cancel(&task.id).expect("first cancellation");
    generation.cancel(&task.id).expect("idempotent second cancellation");
    let cancelled = generation.task(&task.id).expect("load cancelled task");
    assert_eq!(cancelled.status, "cancelled");
    for _ in 0..2 {
        let purged = client
            .authenticated_json::<Value>(
                Method::DELETE,
                &format!("/v1/generation/tasks/{}/content", task.id),
                None,
                None,
            )
            .expect("content purge is idempotent")
            .data;
        assert_eq!(purged["content_status"], "deleted");
    }

    let ack_path = format!(
        "/v1/generation/tasks/{}/deliveries/{}/ack",
        task.id,
        Uuid::new_v4()
    );
    for (body, field) in [
        (json!({ "size_bytes": 1 }), "sha256"),
        (json!({ "sha256": "0".repeat(64) }), "size_bytes"),
        (json!({ "sha256": "bad", "size_bytes": 1 }), "sha256"),
        (json!({ "sha256": "0".repeat(64), "size_bytes": 0 }), "size_bytes"),
    ] {
        assert_http_error_field(
            client.authenticated_json::<Value>(Method::POST, &ack_path, Some(body), None),
            400,
            "validation_failed",
            Some(field),
        );
    }
    assert_http_error(
        client.authenticated_json::<Value>(
            Method::POST,
            &ack_path,
            Some(json!({ "sha256": "a".repeat(64), "size_bytes": 1 })),
            None,
        ),
        404,
        "result_file_not_found",
    );
    for method_path in [
        (Method::GET, "/v1/generation/tasks/not-a-uuid".to_string()),
        (Method::POST, "/v1/generation/tasks/not-a-uuid/cancel".to_string()),
        (Method::DELETE, "/v1/generation/tasks/not-a-uuid/content".to_string()),
    ] {
        assert_http_error(
            client.authenticated_json::<Value>(method_path.0, &method_path.1, None, None),
            404,
            "generation_task_not_found",
        );
    }
    AuthApi::new(client).logout(false).expect("logout generation state test");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_upload_accepted_boundaries_repeated_states_and_pending_limit() {
    let (client, _) = login_new_user();
    let prepare = |body: Value| {
        client
            .authenticated_json::<Value>(
                Method::POST,
                "/v1/uploads/references",
                Some(body),
                None,
            )
            .expect("prepare accepted reference")
            .data["file"]["id"]
            .as_str()
            .expect("prepared file id")
            .to_string()
    };
    for body in [
        json!({ "filename": "x", "mime_type": "image/png", "size_bytes": 1 }),
        json!({ "filename": "x".repeat(255), "mime_type": "image/png", "size_bytes": 10_485_760 }),
        json!({ "filename": "a.jpg", "mime_type": "image/jpeg", "size_bytes": 68 }),
        json!({ "filename": "a.webp", "mime_type": "image/webp", "size_bytes": 68 }),
    ] {
        let file_id = prepare(body);
        client
            .authenticated_json::<Value>(
                Method::DELETE,
                &format!("/v1/uploads/references/{file_id}"),
                None,
                None,
            )
            .expect("delete accepted reference boundary");
    }

    let file_id = prepare(json!({
        "filename": "state.png",
        "mime_type": "image/png",
        "size_bytes": 68,
    }));
    for _ in 0..2 {
        let completed = client
            .authenticated_json::<Value>(
                Method::POST,
                &format!("/v1/uploads/references/{file_id}/complete"),
                None,
                None,
            )
            .expect("reference completion is idempotent")
            .data;
        assert_eq!(completed["status"], "uploaded");
    }
    for _ in 0..2 {
        let deleted = client
            .authenticated_json::<Value>(
                Method::DELETE,
                &format!("/v1/uploads/references/{file_id}"),
                None,
                None,
            )
            .expect("reference deletion is idempotent")
            .data;
        assert_eq!(deleted["status"], "deleted");
    }
    assert_http_error(
        client.authenticated_json::<Value>(
            Method::POST,
            &format!("/v1/uploads/references/{file_id}/complete"),
            None,
            None,
        ),
        409,
        "reference_upload_unavailable",
    );

    let mut pending = Vec::new();
    for index in 0..32 {
        pending.push(prepare(json!({
            "filename": format!("pending-{index}.png"),
            "mime_type": "image/png",
            "size_bytes": 1,
        })));
    }
    assert_http_error(
        client.authenticated_json::<Value>(
            Method::POST,
            "/v1/uploads/references",
            Some(json!({
                "filename": "pending-overflow.png",
                "mime_type": "image/png",
                "size_bytes": 1,
            })),
            None,
        ),
        429,
        "reference_upload_limit_reached",
    );
    for pending_id in pending {
        client
            .authenticated_json::<Value>(
                Method::DELETE,
                &format!("/v1/uploads/references/{pending_id}"),
                None,
                None,
            )
            .expect("delete pending limit fixture");
    }
    AuthApi::new(client).logout(false).expect("logout upload state test");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_cross_user_resource_and_idempotency_isolation() {
    let (client_a, _) = login_new_user();
    let (client_b, _) = login_new_user();
    let generation_a = GenerationApi::new(client_a.clone());
    let generation_b = GenerationApi::new(client_b.clone());
    let shared_request_id = format!("shared_{}", Uuid::new_v4().simple());
    let task_a = generation_a
        .create_task(&prompt_request(shared_request_id.clone(), "user A"))
        .expect("create user A task");
    let task_b = generation_b
        .create_task(&prompt_request(shared_request_id, "user B"))
        .expect("same idempotency key is isolated by user");
    assert_ne!(task_a.id, task_b.id);
    assert_http_error(generation_b.task(&task_a.id), 404, "generation_task_not_found");
    assert_http_error(
        client_b.authenticated_json::<Value>(
            Method::POST,
            &format!("/v1/generation/tasks/{}/cancel", task_a.id),
            None,
            None,
        ),
        404,
        "generation_task_not_found",
    );
    assert_http_error(
        client_b.authenticated_json::<Value>(
            Method::DELETE,
            &format!("/v1/generation/tasks/{}/content", task_a.id),
            None,
            None,
        ),
        404,
        "generation_task_not_found",
    );

    let session_a = AccountApi::new(client_a.clone())
        .snapshot()
        .expect("load user A session")
        .sessions[0]
        .id
        .clone();
    assert_http_error(
        AccountApi::new(client_b.clone()).revoke_session(&session_a),
        404,
        "session_not_found",
    );

    let order_a = PaymentApi::new(client_a.clone())
        .create_credit_order("pack_1000", &format!("order_a_{}", Uuid::new_v4().simple()))
        .expect("create user A order");
    assert_http_error(PaymentApi::new(client_b.clone()).order(&order_a.id), 404, "order_not_found");
    assert_http_error(
        PaymentApi::new(client_b.clone()).sync_order(&order_a.id),
        404,
        "order_not_found",
    );

    let prepared = client_a
        .authenticated_json::<Value>(
            Method::POST,
            "/v1/uploads/references",
            Some(json!({ "filename": "owned.png", "mime_type": "image/png", "size_bytes": 68 })),
            None,
        )
        .expect("prepare user A reference")
        .data;
    let file_a = prepared["file"]["id"].as_str().expect("user A file id").to_string();
    client_a
        .authenticated_json::<Value>(
            Method::POST,
            &format!("/v1/uploads/references/{file_a}/complete"),
            None,
            None,
        )
        .expect("complete user A reference");
    assert_http_error(
        client_b.authenticated_json::<Value>(
            Method::POST,
            &format!("/v1/uploads/references/{file_a}/complete"),
            None,
            None,
        ),
        404,
        "reference_file_not_found",
    );
    assert_http_error(
        client_b.authenticated_json::<Value>(
            Method::DELETE,
            &format!("/v1/uploads/references/{file_a}"),
            None,
            None,
        ),
        404,
        "reference_file_not_found",
    );
    let foreign_reference = CreateGenerationTask {
        client_request_id: format!("foreign_ref_{}", Uuid::new_v4().simple()),
        task_type: "image_generation".to_string(),
        model_code: "openai_image".to_string(),
        prompt: "foreign reference".to_string(),
        quality: Some("1K".to_string()),
        count: Some(1),
        aspect_ratio: Some("square".to_string()),
        reference_file_ids: Some(vec![file_a.clone()]),
        target_language: None,
    };
    assert_http_error(
        generation_b.create_task(&foreign_reference),
        409,
        "reference_file_unavailable",
    );
    generation_a.cancel(&task_a.id).expect("cancel user A fixture");
    generation_b.cancel(&task_b.id).expect("cancel user B fixture");
    client_a
        .authenticated_json::<Value>(
            Method::DELETE,
            &format!("/v1/uploads/references/{file_a}"),
            None,
            None,
        )
        .expect("delete user A fixture");
    AuthApi::new(client_a).logout(false).expect("logout user A");
    AuthApi::new(client_b).logout(false).expect("logout user B");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_payment_required_fields_exact_boundaries_and_repeated_sync() {
    let (client, _) = login_new_user();
    for (path, valid, missing_fields) in [
        (
            "/v1/credits/orders",
            json!({ "pack_code": "pack_1000", "client_request_id": "12345678" }),
            vec!["pack_code", "client_request_id"],
        ),
        (
            "/v1/membership/orders",
            json!({ "plan_code": "basic", "client_request_id": "12345678" }),
            vec!["plan_code", "client_request_id"],
        ),
    ] {
        for field in missing_fields {
            let mut body = valid.clone();
            body.as_object_mut().expect("order body object").remove(field);
            assert_http_error_field(
                client.authenticated_json::<Value>(Method::POST, path, Some(body), None),
                400,
                "validation_failed",
                Some(field),
            );
        }
    }
    assert_http_error_field(
        client.authenticated_json::<Value>(
            Method::POST,
            "/v1/membership/upgrade-quotes",
            Some(json!({})),
            None,
        ),
        400,
        "validation_failed",
        Some("target_plan_code"),
    );
    for field in ["quote_id", "client_request_id"] {
        let mut body = json!({
            "quote_id": Uuid::new_v4(),
            "client_request_id": "12345678",
        });
        body.as_object_mut().expect("upgrade body object").remove(field);
        assert_http_error_field(
            client.authenticated_json::<Value>(
                Method::POST,
                "/v1/membership/upgrade-orders",
                Some(body),
                None,
            ),
            400,
            "validation_failed",
            Some(field),
        );
    }
    for (path, field, body) in [
        (
            "/v1/credits/orders",
            "pack_code",
            json!({ "pack_code": "a", "client_request_id": "invalid01" }),
        ),
        (
            "/v1/credits/orders",
            "pack_code",
            json!({ "pack_code": format!("p{}", "x".repeat(32)), "client_request_id": "invalid02" }),
        ),
        (
            "/v1/membership/orders",
            "plan_code",
            json!({ "plan_code": "a", "client_request_id": "invalid03" }),
        ),
        (
            "/v1/membership/orders",
            "plan_code",
            json!({ "plan_code": format!("p{}", "x".repeat(32)), "client_request_id": "invalid04" }),
        ),
    ] {
        assert_http_error_field(
            client.authenticated_json::<Value>(Method::POST, path, Some(body), None),
            400,
            "validation_failed",
            Some(field),
        );
    }

    let payment = PaymentApi::new(client.clone());
    let short_boundary = payment
        .create_credit_order("pack_1000", "12345678")
        .expect("eight character order request id");
    let long_boundary = MembershipApi::new(client.clone())
        .create_order("basic", &"m".repeat(64))
        .expect("64 character order request id");
    for order_id in [&short_boundary.id, &long_boundary.id] {
        let first = payment.sync_order(order_id).expect("first pending sync");
        let second = payment.sync_order(order_id).expect("second pending sync");
        assert_eq!(first.status, "pending_payment");
        assert_eq!(second.status, "pending_payment");
        assert_eq!(first.id, second.id);
    }
    AuthApi::new(client).logout(false).expect("logout payment required field test");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_session_self_revoke_and_logout_all_are_terminal() {
    let (client, login) = login_new_user();
    let current_session = AccountApi::new(client.clone())
        .snapshot()
        .expect("load current session")
        .sessions
        .into_iter()
        .find(|session| session.is_current)
        .expect("current session");
    AccountApi::new(client.clone())
        .revoke_session(&current_session.id)
        .expect("revoke current session");
    assert_http_error(
        client.authenticated_json::<Value>(Method::GET, "/v1/account", None, None),
        401,
        "session_invalid",
    );
    assert!(client.session().access_token().is_none());

    let (logout_all_client, logout_all_login) = login_new_user();
    let copied_session = new_client_with(
        logout_all_client.device().id.clone(),
        logout_all_client.app_version(),
    );
    copied_session
        .session()
        .install_tokens(&logout_all_login.tokens)
        .expect("copy session before logout all");
    AuthApi::new(logout_all_client.clone())
        .logout(true)
        .expect("logout all sessions");
    assert!(logout_all_client.session().access_token().is_none());
    assert_http_error(
        copied_session.authenticated_json::<Value>(Method::GET, "/v1/account", None, None),
        401,
        "session_invalid",
    );
    assert!(copied_session.session().access_token().is_none());
    assert!(!login.tokens.access_token.is_empty());
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_http_route_errors_and_request_ids() {
    let http = reqwest::blocking::Client::new();
    let supplied_request_id = "protocol.request-0001";
    let response = http
        .get(base_url().join("/v1/route-does-not-exist").expect("unknown route URL"))
        .header("X-Request-ID", supplied_request_id)
        .send()
        .expect("unknown route response");
    let body = assert_raw_problem(response, 404, "route_not_found");
    assert_eq!(body["request_id"], supplied_request_id);

    let response = http
        .get(base_url().join("/v1/another-missing-route").expect("missing route URL"))
        .header("X-Request-ID", "short")
        .send()
        .expect("invalid request ID response");
    let body = assert_raw_problem(response, 404, "route_not_found");
    assert_ne!(body["request_id"], "short");
    assert!(body["request_id"].as_str().expect("generated request ID").len() >= 8);
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_method_not_allowed_uses_json_error_envelope() {
    let http = reqwest::blocking::Client::new();
    let response = http
        .post(base_url().join("/v1").expect("method route URL"))
        .header("X-Request-ID", "method-test-0001")
        .send()
        .expect("method not allowed response");
    assert_eq!(response.headers().get("Allow").and_then(|value| value.to_str().ok()), Some("HEAD, GET"));
    assert_eq!(
        response.headers().get("Content-Type").and_then(|value| value.to_str().ok()),
        Some("application/json; charset=utf-8"),
        "405 responses must use the same JSON envelope as other API errors"
    );
    assert_raw_problem(response, 405, "request_error");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_body_parser_errors_use_json_envelopes() {
    let http = reqwest::blocking::Client::new();
    let response = http
        .post(base_url().join("/v1/auth/email/code").expect("bad JSON URL"))
        .header("Content-Type", "application/json")
        .body("{")
        .send()
        .expect("malformed JSON response");
    assert_raw_problem(response, 400, "request_error");

    let oversized = json!({
        "email": format!("{}@example.com", "x".repeat(70_000)),
        "app_version": env!("CARGO_PKG_VERSION"),
    })
    .to_string();
    let response = http
        .post(base_url().join("/v1/auth/email/code").expect("oversized JSON URL"))
        .header("Content-Type", "application/json")
        .body(oversized)
        .send()
        .expect("oversized JSON response");
    assert_raw_problem(response, 413, "request_error");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_authentication_header_matrix() {
    let (client, login) = login_new_user();
    let http = reqwest::blocking::Client::new();
    let account_url = base_url().join("/v1/account").expect("account URL");

    let response = http
        .get(account_url.clone())
        .header("Authorization", format!("Bearer {}", login.tokens.access_token))
        .header("X-Client-Version", client.app_version())
        .header("X-Device-ID", &client.device().id)
        .send()
        .expect("Bearer-only response");
    assert_raw_problem(response, 401, "authentication_required");

    let response = http
        .get(account_url.clone())
        .header("X-Token", &login.tokens.access_token)
        .send()
        .expect("missing identity headers response");
    assert_raw_problem(response, 400, "client_identity_required");

    let response = http
        .get(account_url.clone())
        .header("X-Token", &login.tokens.access_token)
        .header("X-Client-Version", client.app_version())
        .send()
        .expect("missing device header response");
    assert_raw_problem(response, 400, "client_identity_required");

    let response = http
        .get(account_url.clone())
        .header("X-Token", &login.tokens.access_token)
        .header("X-Device-ID", &client.device().id)
        .send()
        .expect("missing version header response");
    assert_raw_problem(response, 400, "client_identity_required");

    let response = http
        .get(account_url.clone())
        .header("X-Token", "not-a-jwt")
        .header("X-Client-Version", client.app_version())
        .header("X-Device-ID", &client.device().id)
        .send()
        .expect("invalid token response");
    assert_raw_problem(response, 401, "access_token_invalid");

    let response = http
        .get(account_url)
        .header("X-Token", &login.tokens.access_token)
        .header("X-Client-Version", "1.0")
        .header("X-Device-ID", &client.device().id)
        .send()
        .expect("invalid authenticated client version response");
    assert_raw_problem(response, 400, "client_version_invalid");
    AuthApi::new(client).logout(false).expect("logout header matrix user");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_exact_http_success_statuses_and_envelopes() {
    let (client, login) = login_new_user();
    let http = reqwest::blocking::Client::new();
    let request_id = format!("status_{}", Uuid::new_v4().simple());
    let task_body = serde_json::to_value(prompt_request(request_id.clone(), "status contract"))
        .expect("serialize status task");
    let response = http
        .post(base_url().join("/v1/generation/tasks").expect("create task URL"))
        .header("Content-Type", "application/json")
        .header("X-Token", &login.tokens.access_token)
        .header("X-Client-Version", client.app_version())
        .header("X-Device-ID", &client.device().id)
        .header("Idempotency-Key", &request_id)
        .header("X-Request-ID", "success-status-0001")
        .json(&task_body)
        .send()
        .expect("raw task creation response");
    assert_eq!(response.status().as_u16(), 202);
    assert_eq!(
        response.headers().get("X-Request-ID").and_then(|value| value.to_str().ok()),
        Some("success-status-0001")
    );
    let body: Value = response.json().expect("task success envelope");
    assert_eq!(body["request_id"], "success-status-0001");
    assert!(body["error"].is_null());
    assert!(body.get("meta").is_none());
    let task_id = body["data"]["id"].as_str().expect("created task ID");

    let response = http
        .post(
            base_url()
                .join(&format!("/v1/generation/tasks/{task_id}/cancel"))
                .expect("cancel task URL"),
        )
        .header("X-Token", &login.tokens.access_token)
        .header("X-Client-Version", client.app_version())
        .header("X-Device-ID", &client.device().id)
        .send()
        .expect("raw task cancellation response");
    assert_eq!(response.status().as_u16(), 200);
    let body: Value = response.json().expect("cancel success envelope");
    assert!(body["request_id"].is_string());
    assert!(body["error"].is_null());
    assert_eq!(body["data"]["status"], "cancelled");

    let order_request_id = format!("status_order_{}", Uuid::new_v4().simple());
    let response = http
        .post(base_url().join("/v1/credits/orders").expect("credit order URL"))
        .header("X-Token", &login.tokens.access_token)
        .header("X-Client-Version", client.app_version())
        .header("X-Device-ID", &client.device().id)
        .header("Idempotency-Key", &order_request_id)
        .json(&json!({
            "pack_code": "pack_1000",
            "client_request_id": order_request_id,
        }))
        .send()
        .expect("raw credit order response");
    assert_eq!(response.status().as_u16(), 200);
    let body: Value = response.json().expect("order success envelope");
    assert!(body["error"].is_null());
    assert_eq!(body["data"]["status"], "pending_payment");
    AuthApi::new(client).logout(false).expect("logout status contract user");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_concurrent_refresh_is_single_flight_against_backend() {
    let (client, _) = login_new_user();
    let workers = 8;
    let barrier = Arc::new(Barrier::new(workers));
    let handles: Vec<_> = (0..workers)
        .map(|_| {
            let thread_client = client.clone();
            let thread_barrier = barrier.clone();
            std::thread::spawn(move || {
                thread_barrier.wait();
                AuthApi::new(thread_client).refresh()
            })
        })
        .collect();
    let tokens: Vec<String> = handles
        .into_iter()
        .map(|handle| handle.join().expect("refresh thread").expect("concurrent refresh"))
        .collect();
    assert_eq!(tokens.len(), workers);
    assert!(tokens.iter().all(|token| token == &tokens[0]));
    assert!(!client
        .authenticated_json::<Value>(Method::GET, "/v1/account", None, None)
        .expect("account works after concurrent refresh")
        .request_id
        .is_empty());
    AuthApi::new(client).logout(false).expect("logout concurrent refresh user");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_concurrent_generation_idempotency_never_duplicates_resource() {
    let (client, _) = login_new_user();
    let workers = 8;
    let request = Arc::new(prompt_request(
        format!("concurrent_{}", Uuid::new_v4().simple()),
        "same concurrent prompt",
    ));
    let barrier = Arc::new(Barrier::new(workers));
    let handles: Vec<_> = (0..workers)
        .map(|_| {
            let thread_client = client.clone();
            let thread_request = request.clone();
            let thread_barrier = barrier.clone();
            std::thread::spawn(move || {
                thread_barrier.wait();
                GenerationApi::new(thread_client).create_task(&thread_request)
            })
        })
        .collect();
    let mut task_ids = HashSet::new();
    let mut in_progress = 0;
    for result in handles.into_iter().map(|handle| handle.join().expect("generation thread")) {
        match result {
            Ok(task) => {
                task_ids.insert(task.id);
            }
            Err(ApiError::Http { status, code, .. }) => {
                assert_eq!(status, 409);
                assert_eq!(code, "request_in_progress");
                in_progress += 1;
            }
            Err(error) => panic!("unexpected concurrent idempotency error: {error:?}"),
        }
    }
    assert_eq!(task_ids.len(), 1, "concurrent replays created more than one task");
    assert!(task_ids.len() + in_progress >= 1);
    let task_id = task_ids.into_iter().next().expect("one idempotent task");
    GenerationApi::new(client.clone())
        .cancel(&task_id)
        .expect("cancel concurrent idempotency task");
    AuthApi::new(client).logout(false).expect("logout concurrent idempotency user");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_generation_and_ledger_cursor_continuity() {
    let (client, _) = login_new_user();
    let generation = GenerationApi::new(client.clone());
    let mut created_ids = Vec::new();
    for index in 0..3 {
        created_ids.push(
            generation
                .create_task(&prompt_request(
                    format!("cursor_{index}_{}", Uuid::new_v4().simple()),
                    &format!("cursor prompt {index}"),
                ))
                .expect("create cursor task")
                .id,
        );
    }
    let first = client
        .authenticated_json::<Value>(
            Method::GET,
            "/v1/generation/tasks?limit=2",
            None,
            None,
        )
        .expect("first task page")
        .data;
    let first_items = first["items"].as_array().expect("first task items");
    assert_eq!(first_items.len(), 2);
    let cursor = first["next_cursor"].as_str().expect("task next cursor");
    let second = client
        .authenticated_json::<Value>(
            Method::GET,
            &format!("/v1/generation/tasks?limit=2&cursor={cursor}"),
            None,
            None,
        )
        .expect("second task page")
        .data;
    let second_items = second["items"].as_array().expect("second task items");
    assert_eq!(second_items.len(), 1);
    assert!(second["next_cursor"].is_null());
    let first_ids: HashSet<_> = first_items.iter().map(|item| item["id"].as_str().unwrap()).collect();
    assert!(second_items
        .iter()
        .all(|item| !first_ids.contains(item["id"].as_str().expect("second task ID"))));

    let ledger_first = client
        .authenticated_json::<Vec<CreditLedgerItem>>(
            Method::GET,
            "/v1/credits/ledger?limit=1",
            None,
            None,
        )
        .expect("first ledger page");
    assert_eq!(ledger_first.data.len(), 1);
    let ledger_cursor = ledger_first
        .meta
        .as_ref()
        .and_then(|meta| meta.next_cursor.as_deref())
        .expect("ledger next cursor");
    let ledger_second = client
        .authenticated_json::<Vec<CreditLedgerItem>>(
            Method::GET,
            &format!("/v1/credits/ledger?limit=1&cursor={ledger_cursor}"),
            None,
            None,
        )
        .expect("second ledger page");
    assert_eq!(ledger_second.data.len(), 1);
    assert_ne!(ledger_first.data[0].id, ledger_second.data[0].id);
    for task_id in created_ids {
        generation.cancel(&task_id).expect("cancel cursor fixture");
    }
    AuthApi::new(client).logout(false).expect("logout cursor continuity user");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_reference_attachment_lifecycle_prevents_delete_and_reuse() {
    let (client, _) = login_new_user();
    let path = std::env::temp_dir().join(format!("artforge-reference-lifecycle-{}.png", Uuid::new_v4()));
    std::fs::write(&path, MOCK_PNG).expect("write lifecycle reference fixture");
    let generation = GenerationApi::new(client.clone());
    let file_id = generation.upload_reference(&path).expect("upload lifecycle reference");
    let _ = std::fs::remove_file(&path);
    let request = CreateGenerationTask {
        client_request_id: format!("reference_owner_{}", Uuid::new_v4().simple()),
        task_type: "image_generation".to_string(),
        model_code: "openai_image".to_string(),
        prompt: "reference owner".to_string(),
        quality: Some("1K".to_string()),
        count: Some(1),
        aspect_ratio: Some("square".to_string()),
        reference_file_ids: Some(vec![file_id.clone()]),
        target_language: None,
    };
    let task = generation.create_task(&request).expect("attach lifecycle reference");
    assert_eq!(task.request["reference_file_ids"], json!([file_id]));
    assert_http_error(
        client.authenticated_json::<Value>(
            Method::DELETE,
            &format!("/v1/uploads/references/{file_id}"),
            None,
            None,
        ),
        409,
        "reference_file_in_use",
    );
    let replay_reference = CreateGenerationTask {
        client_request_id: format!("reference_reuse_{}", Uuid::new_v4().simple()),
        prompt: "reference reuse".to_string(),
        ..request
    };
    assert_http_error(
        generation.create_task(&replay_reference),
        409,
        "reference_file_unavailable",
    );
    generation.cancel(&task.id).expect("cancel reference lifecycle task");
    AuthApi::new(client).logout(false).expect("logout reference lifecycle user");
}

#[test]
#[ignore = "requires the dev Mock API server"]
fn cross_stack_catalog_and_account_dto_invariants() {
    let (client, login) = login_new_user();
    let snapshot = AccountApi::new(client.clone()).snapshot().expect("load invariant snapshot");
    assert!(Uuid::parse_str(&snapshot.account.user.id).is_ok());
    assert_eq!(snapshot.account.user.id, login.user.id);
    assert_eq!(snapshot.account.user.status, "active");
    assert!(snapshot.account.user.registered_at.contains('T'));
    assert!(snapshot.account.membership.revision.parse::<u64>().is_ok());
    let credits = snapshot.account.credits.as_ref().expect("credit account");
    for value in [
        &credits.available,
        &credits.reserved,
        &credits.lifetime_granted,
        &credits.lifetime_spent,
    ] {
        assert!(value.parse::<u64>().is_ok(), "credit amount is not an unsigned decimal: {value}");
    }

    let plan_codes: HashSet<_> = snapshot.plans.iter().map(|plan| plan.code.as_str()).collect();
    assert_eq!(plan_codes.len(), snapshot.plans.len());
    for plan in &snapshot.plans {
        assert!(!plan.name.is_empty());
        assert!(plan.version > 0);
        assert!(plan.tier_rank >= 0);
        assert!(plan.price_cents.parse::<u64>().is_ok());
        assert!(plan.period_days > 0);
        assert!(plan.grant_credits.parse::<u64>().is_ok());
        assert!((0..=10_000).contains(&plan.recharge_discount_bps));
        assert!(["1K", "2K", "4K"].contains(&plan.max_quality.as_str()));
        assert!(plan.entitlements.is_object());
    }
    let pack_codes: HashSet<_> = snapshot.packs.iter().map(|pack| pack.code.as_str()).collect();
    assert_eq!(pack_codes.len(), snapshot.packs.len());
    for pack in &snapshot.packs {
        assert!(!pack.name.is_empty());
        assert!(pack.price_cents.parse::<u64>().expect("pack price") > 0);
        assert!(pack.credits.parse::<u64>().expect("pack credits") > 0);
    }
    let model_codes: HashSet<_> = snapshot.models.iter().map(|model| model.code.as_str()).collect();
    assert_eq!(model_codes.len(), snapshot.models.len());
    for model in &snapshot.models {
        assert!(model.version > 0);
        assert!(!model.name.is_empty());
        assert!(["image_generation", "prompt_processing"].contains(&model.purpose.as_str()));
        assert!(model.capabilities.is_object());
        assert!(!model.prices.is_empty());
        for price in &model.prices {
            assert!(["standard", "1K", "2K", "4K"].contains(&price.quality.as_str()));
            assert!(price.credit_cost.parse::<u64>().expect("model credit cost") > 0);
            if let Some(edge) = price.max_long_edge {
                assert!(edge > 0);
            }
        }
    }
    assert_eq!(snapshot.sessions.iter().filter(|session| session.is_current).count(), 1);
    for session in &snapshot.sessions {
        assert!(Uuid::parse_str(&session.id).is_ok());
        assert!(["windows", "macos"].contains(&session.platform.as_str()));
        assert_eq!(session.app_version.split('.').count(), 3);
        assert!(session.last_seen_at.contains('T'));
    }
    for entry in &snapshot.ledger {
        assert!(!entry.entry_type.is_empty());
        assert!(entry.available_delta.parse::<i128>().is_ok());
        assert!(entry.reserved_delta.parse::<i128>().is_ok());
        assert!(entry.available_after.parse::<u128>().is_ok());
        assert!(entry.reserved_after.parse::<u128>().is_ok());
        assert!(!entry.business_type.is_empty());
        assert!(!entry.description.is_empty());
        assert!(entry.created_at.contains('T'));
    }
    AuthApi::new(client).logout(false).expect("logout DTO invariant user");
}
