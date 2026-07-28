use super::{ApiClient, ApiError};
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct PromptOptimizationPricing {
    #[serde(default)]
    pub(crate) unit_credit_cost: String,
    #[serde(default)]
    pub(crate) maximum_credits: String,
    #[serde(default)]
    pub(crate) consumed_credits: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct PromptOptimizationFailure {
    #[serde(default)]
    pub(crate) code: String,
    #[serde(default)]
    pub(crate) message: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct PromptOptimizationResult {
    #[serde(default)]
    pub(crate) chinese_prompt: String,
    #[serde(default)]
    pub(crate) english_prompt: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct PromptOptimizationTopBand {
    #[serde(default)]
    pub(crate) triggered: bool,
    #[serde(default)]
    pub(crate) qualifies: bool,
    #[serde(default)]
    pub(crate) blocking_issues: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct PromptOptimizationRound {
    pub(crate) round: i32,
    #[serde(default)]
    pub(crate) status: String,
    pub(crate) score_before: i32,
    pub(crate) score_after: Option<i32>,
    #[serde(default)]
    pub(crate) accepted: bool,
    #[serde(default)]
    pub(crate) chinese_prompt: Option<String>,
    #[serde(default)]
    pub(crate) english_prompt: Option<String>,
    #[serde(default)]
    pub(crate) dimension_scores: Value,
    #[serde(default)]
    pub(crate) issues: Vec<String>,
    #[serde(default)]
    pub(crate) major_changes: Vec<String>,
    #[serde(default)]
    pub(crate) drift_detected: bool,
    #[serde(default)]
    pub(crate) drift_reason: Option<String>,
    #[serde(default)]
    pub(crate) top_band: Option<PromptOptimizationTopBand>,
    #[serde(default)]
    pub(crate) credit_cost: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct PromptOptimizationDetail {
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) status: String,
    #[serde(default)]
    pub(crate) phase: String,
    #[serde(default)]
    pub(crate) run_mode: String,
    #[serde(default)]
    pub(crate) focus_mode: String,
    pub(crate) max_rounds: i32,
    pub(crate) current_round: i32,
    pub(crate) completed_rounds: i32,
    pub(crate) target_score: i32,
    pub(crate) baseline_score: Option<i32>,
    pub(crate) best_score: Option<i32>,
    pub(crate) best_round_no: Option<i32>,
    pub(crate) progress_percent: i32,
    #[serde(default)]
    pub(crate) pricing: PromptOptimizationPricing,
    #[serde(default)]
    pub(crate) stop_reason: Option<String>,
    #[serde(default)]
    pub(crate) failure: Option<PromptOptimizationFailure>,
    #[serde(default)]
    pub(crate) original_prompt: Option<String>,
    #[serde(default)]
    pub(crate) result: Option<PromptOptimizationResult>,
    pub(crate) result_score: Option<i32>,
    pub(crate) result_round_no: Option<i32>,
    #[serde(default)]
    pub(crate) result_accepted: bool,
    #[serde(default)]
    pub(crate) final_result: Option<PromptOptimizationResult>,
    #[serde(default)]
    pub(crate) pending_feedback: Option<String>,
    #[serde(default)]
    pub(crate) stable_feedback: Vec<String>,
    #[serde(default)]
    pub(crate) rounds: Vec<PromptOptimizationRound>,
    #[serde(default)]
    pub(crate) can_pause: bool,
    #[serde(default)]
    pub(crate) can_resume: bool,
    #[serde(default)]
    pub(crate) can_retry: bool,
    #[serde(default)]
    pub(crate) can_cancel: bool,
    #[serde(default)]
    pub(crate) can_continue: bool,
    #[serde(default)]
    pub(crate) can_apply: bool,
    #[serde(default)]
    pub(crate) can_clear_stable_feedback: bool,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct PromptOptimizationSummary {
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) status: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct PromptOptimizationList {
    #[serde(default)]
    items: Vec<PromptOptimizationSummary>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct CreatePromptOptimization {
    pub(crate) client_request_id: String,
    pub(crate) prompt: String,
    pub(crate) run_mode: String,
    pub(crate) focus_mode: String,
    pub(crate) max_rounds: i32,
    pub(crate) target_score: i32,
}

#[derive(Clone, Debug, Serialize)]
struct ReviewDecision<'a> {
    client_request_id: &'a str,
    action: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    feedback: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    feedback_scope: Option<&'a str>,
}

#[derive(Clone)]
pub(crate) struct PromptOptimizationApi {
    client: ApiClient,
}

impl PromptOptimizationApi {
    pub(crate) fn new(client: ApiClient) -> Self {
        Self { client }
    }

    pub(crate) fn create(
        &self,
        request: &CreatePromptOptimization,
    ) -> Result<PromptOptimizationDetail, ApiError> {
        let body = serde_json::to_value(request).map_err(protocol_error)?;
        self.client
            .authenticated_json::<PromptOptimizationDetail>(
                Method::POST,
                "/v1/prompt-optimizations",
                Some(body),
                Some(&request.client_request_id),
            )
            .map(|response| response.data)
    }

    pub(crate) fn get(&self, id: &str) -> Result<PromptOptimizationDetail, ApiError> {
        self.client
            .authenticated_json::<PromptOptimizationDetail>(
                Method::GET,
                &format!("/v1/prompt-optimizations/{id}"),
                None,
                None,
            )
            .map(|response| response.data)
    }

    pub(crate) fn active(&self) -> Result<Vec<PromptOptimizationSummary>, ApiError> {
        self.client
            .authenticated_json::<PromptOptimizationList>(
                Method::GET,
                "/v1/prompt-optimizations?limit=1&status=active",
                None,
                None,
            )
            .map(|response| response.data.items)
    }

    fn action(&self, id: &str, action: &str) -> Result<PromptOptimizationDetail, ApiError> {
        self.client
            .authenticated_json::<PromptOptimizationDetail>(
                Method::POST,
                &format!("/v1/prompt-optimizations/{id}/{action}"),
                None,
                None,
            )
            .map(|response| response.data)
    }

    pub(crate) fn pause(&self, id: &str) -> Result<PromptOptimizationDetail, ApiError> {
        self.action(id, "pause")
    }

    pub(crate) fn resume(&self, id: &str) -> Result<PromptOptimizationDetail, ApiError> {
        self.action(id, "resume")
    }

    pub(crate) fn cancel(&self, id: &str) -> Result<PromptOptimizationDetail, ApiError> {
        self.action(id, "cancel")
    }

    pub(crate) fn retry(&self, id: &str) -> Result<PromptOptimizationDetail, ApiError> {
        self.action(id, "retry")
    }

    pub(crate) fn review(
        &self,
        id: &str,
        client_request_id: &str,
        action: &str,
        feedback: Option<&str>,
        feedback_scope: Option<&str>,
    ) -> Result<PromptOptimizationDetail, ApiError> {
        let body = serde_json::to_value(ReviewDecision {
            client_request_id,
            action,
            feedback,
            feedback_scope,
        })
        .map_err(protocol_error)?;
        self.client
            .authenticated_json::<PromptOptimizationDetail>(
                Method::POST,
                &format!("/v1/prompt-optimizations/{id}/review-decision"),
                Some(body),
                Some(client_request_id),
            )
            .map(|response| response.data)
    }
}

fn protocol_error(error: serde_json::Error) -> ApiError {
    ApiError::Protocol {
        message: error.to_string(),
        request_id: None,
    }
}
