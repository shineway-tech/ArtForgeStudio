//! Mock provider，仅用于阶段 3 之前的 UI / 任务流验证。

use artait_model::{ConnectionStatus, ProviderCapabilities, ProviderError, ProviderFamily};
use artait_provider::{
    meta::ProviderMeta, request::*, Analyzer, ImageGenerator, Provider, ProviderContext,
    ProviderModelList, ProviderResult,
};
use async_trait::async_trait;

const MOCK_META: ProviderMeta = ProviderMeta {
    id: "mock",
    display_name: "Mock Provider",
    family: ProviderFamily::Mock,
    capabilities: ProviderCapabilities {
        generate: true,
        generate_character: false,
        generate_video: false,
        analyze: true,
        test_connection: true,
        quota: false,
        upload_binary: false,
        poll_task: false,
    },
    default_generation_models: &["mock-img-1"],
    default_analysis_models: &["mock-text-1"],
    default_video_models: &[],
    config_schema: r#"{"type":"object","properties":{}}"#,
    is_legacy: false,
};

#[derive(Default)]
pub struct MockProvider;

#[async_trait]
impl Provider for MockProvider {
    fn meta(&self) -> &ProviderMeta {
        &MOCK_META
    }

    async fn test_connection(&self, _ctx: &ProviderContext) -> ProviderResult<ConnectionStatus> {
        Ok(ConnectionStatus {
            ok: true,
            message: "mock provider always reachable".to_string(),
        })
    }

    async fn list_models(&self, _ctx: &ProviderContext) -> ProviderResult<ProviderModelList> {
        Ok(ProviderModelList {
            generation: MOCK_META
                .default_generation_models
                .iter()
                .map(|m| (*m).to_string())
                .collect(),
            analysis: MOCK_META
                .default_analysis_models
                .iter()
                .map(|m| (*m).to_string())
                .collect(),
            video: Vec::new(),
        })
    }

    fn as_image_generator(&self) -> Option<&dyn ImageGenerator> {
        Some(self)
    }

    fn as_analyzer(&self) -> Option<&dyn Analyzer> {
        Some(self)
    }
}

#[async_trait]
impl ImageGenerator for MockProvider {
    async fn generate(
        &self,
        _req: ImageGenerationRequest,
        ctx: &ProviderContext,
    ) -> ProviderResult<GenerationOutput> {
        if ctx.is_cancelled() {
            return Err(ProviderError::TaskCancelled);
        }
        Ok(GenerationOutput::Url {
            url: "https://example.com/mock.png".to_string(),
            metadata: serde_json::json!({"mock": true}),
        })
    }
}

#[async_trait]
impl Analyzer for MockProvider {
    async fn analyze(
        &self,
        req: AnalysisRequest,
        ctx: &ProviderContext,
    ) -> ProviderResult<AnalysisOutput> {
        if ctx.is_cancelled() {
            return Err(ProviderError::TaskCancelled);
        }
        Ok(AnalysisOutput {
            text: format!("mock analysis of: {}", req.user_prompt),
            structured: None,
            usage: None,
        })
    }
}
