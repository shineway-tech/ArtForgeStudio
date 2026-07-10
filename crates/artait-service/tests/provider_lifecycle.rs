//! Provider 生命周期集成测试：创建 → 编辑 → 校验端点 → 连接测试。

use std::sync::Arc;

use artait_model::AppConfig;
use artait_provider::ProviderRegistry;
use artait_providers::MockProvider;
use artait_service::provider::{
    create_provider, edit_provider, validate_endpoint, CreateProviderParams, EditProviderParams,
    ProviderOpResult,
};

fn test_registry() -> Arc<ProviderRegistry> {
    let mut reg = ProviderRegistry::new();
    reg.register(Arc::new(MockProvider::default()));
    Arc::new(reg)
}

fn test_http() -> Arc<dyn artait_provider::HttpClient> {
    Arc::new(artait_provider::ReqwestClient::new())
}

#[test]
fn validate_endpoint_rejects_empty() {
    assert!(validate_endpoint("").is_err());
    assert!(validate_endpoint("not-a-url").is_err());
    assert!(validate_endpoint("ftp://example.com").is_err());
}

#[test]
fn validate_endpoint_accepts_http() {
    assert!(validate_endpoint("http://localhost:8080").is_ok());
    assert!(validate_endpoint("https://api.example.com/v1").is_ok());
}

#[test]
fn create_provider_adds_to_config() {
    let mut cfg = AppConfig::default();
    let result = create_provider(
        &mut cfg,
        CreateProviderParams {
            name: "Test Provider".into(),
            endpoint: "https://api.test.com".into(),
            api_key: "sk-test-key".into(),
            api_style: "auto".into(),
            node_kind: "both".into(),
            generation_model: Some("gpt-image-1".into()),
            analysis_model: Some("gpt-4o".into()),
        },
    )
    .unwrap();

    assert!(matches!(result, ProviderOpResult::Created { .. }));
    assert_eq!(cfg.providers.len(), 1);
    let inst = &cfg.providers[0];
    assert_eq!(inst.name, "Test Provider");
    assert!(inst.endpoint.as_deref() == Some("https://api.test.com"));
    assert!(inst.secret_ref.is_some());
    assert!(inst.api_key.as_deref() == Some("sk-test-key"));
    assert!(inst.models.generation_model.as_deref() == Some("gpt-image-1"));
    assert!(inst.models.analysis_model.as_deref() == Some("gpt-4o"));
}

#[test]
fn create_provider_rejects_empty_key() {
    let mut cfg = AppConfig::default();
    let result = create_provider(
        &mut cfg,
        CreateProviderParams {
            name: "NoKey".into(),
            endpoint: "https://api.test.com".into(),
            api_key: "  ".into(),
            api_style: "auto".into(),
            node_kind: "both".into(),
            generation_model: None,
            analysis_model: None,
        },
    );
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("API Key"));
}

#[test]
fn edit_provider_updates_fields() {
    let mut cfg = AppConfig::default();
    // first create
    let created = create_provider(
        &mut cfg,
        CreateProviderParams {
            name: "Original".into(),
            endpoint: "https://old.example.com".into(),
            api_key: "sk-old".into(),
            api_style: "auto".into(),
            node_kind: "both".into(),
            generation_model: Some("gpt-image-1".into()),
            analysis_model: Some("gpt-4o".into()),
        },
    )
    .unwrap();
    let id = match &created {
        ProviderOpResult::Created { instance, .. } => instance.id.clone(),
        _ => panic!("expected Created"),
    };

    // then edit
    let result = edit_provider(
        &mut cfg,
        &id,
        EditProviderParams {
            name: "Updated".into(),
            endpoint: "https://new.example.com".into(),
            api_key: "sk-new".into(),
            api_style: "gemini".into(),
            node_kind: "generation".into(),
            generation_model: Some("gemini-2.5-flash-image-preview".into()),
            analysis_model: None,
        },
    )
    .unwrap();

    assert!(matches!(result, ProviderOpResult::Updated { .. }));
    let inst = cfg.providers.iter().find(|p| p.id == id).unwrap();
    assert_eq!(inst.name, "Updated");
    assert!(inst.endpoint.as_deref() == Some("https://new.example.com"));
    assert_eq!(inst.scopes.len(), 1);
    assert!(inst
        .scopes
        .contains(&artait_model::ProviderScope::Generation));
    assert!(inst.models.analysis_model.is_none());
}

#[tokio::test]
async fn mock_connection_test_succeeds() {
    let registry = test_registry();
    let provider = registry.get("mock").expect("mock provider registered");
    let http = test_http();

    let pctx = artait_provider::ProviderContext::with_http(
        String::from("test"),
        String::from("mock"),
        http,
    );

    let status = provider.test_connection(&pctx).await.unwrap();
    assert!(status.ok);
    assert!(status.message.contains("always reachable"));
}
