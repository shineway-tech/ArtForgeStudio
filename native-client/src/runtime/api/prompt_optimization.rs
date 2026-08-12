use super::{ApiClient, ApiError, SessionScope};
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
    pub(crate) candidate_score: Option<i32>,
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

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
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

    pub(crate) fn create_scoped(
        &self,
        request: &CreatePromptOptimization,
        scope: &SessionScope,
    ) -> Result<PromptOptimizationDetail, ApiError> {
        let body = serde_json::to_value(request).map_err(protocol_error)?;
        self.client
            .authenticated_json_scoped::<PromptOptimizationDetail>(
                Method::POST,
                "/v1/prompt-optimizations",
                Some(body),
                Some(&request.client_request_id),
                scope,
            )
            .map(|response| response.data)
    }

    pub(crate) fn get_scoped(
        &self,
        id: &str,
        scope: &SessionScope,
    ) -> Result<PromptOptimizationDetail, ApiError> {
        self.client
            .authenticated_json_scoped::<PromptOptimizationDetail>(
                Method::GET,
                &format!("/v1/prompt-optimizations/{id}"),
                None,
                None,
                scope,
            )
            .map(|response| response.data)
    }

    pub(crate) fn active_scoped(
        &self,
        scope: &SessionScope,
    ) -> Result<Vec<PromptOptimizationSummary>, ApiError> {
        self.client
            .authenticated_json_scoped::<PromptOptimizationList>(
                Method::GET,
                "/v1/prompt-optimizations?limit=1&status=active",
                None,
                None,
                scope,
            )
            .map(|response| response.data.items)
    }

    fn action_scoped(
        &self,
        id: &str,
        action: &str,
        scope: &SessionScope,
    ) -> Result<PromptOptimizationDetail, ApiError> {
        self.client
            .authenticated_json_scoped::<PromptOptimizationDetail>(
                Method::POST,
                &format!("/v1/prompt-optimizations/{id}/{action}"),
                None,
                None,
                scope,
            )
            .map(|response| response.data)
    }

    pub(crate) fn pause_scoped(
        &self,
        id: &str,
        scope: &SessionScope,
    ) -> Result<PromptOptimizationDetail, ApiError> {
        self.action_scoped(id, "pause", scope)
    }

    pub(crate) fn resume_scoped(
        &self,
        id: &str,
        scope: &SessionScope,
    ) -> Result<PromptOptimizationDetail, ApiError> {
        self.action_scoped(id, "resume", scope)
    }

    pub(crate) fn cancel_scoped(
        &self,
        id: &str,
        scope: &SessionScope,
    ) -> Result<PromptOptimizationDetail, ApiError> {
        self.action_scoped(id, "cancel", scope)
    }

    pub(crate) fn retry_scoped(
        &self,
        id: &str,
        scope: &SessionScope,
    ) -> Result<PromptOptimizationDetail, ApiError> {
        self.action_scoped(id, "retry", scope)
    }

    pub(crate) fn review_scoped(
        &self,
        id: &str,
        client_request_id: &str,
        action: &str,
        feedback: Option<&str>,
        feedback_scope: Option<&str>,
        scope: &SessionScope,
    ) -> Result<PromptOptimizationDetail, ApiError> {
        let body = serde_json::to_value(ReviewDecision {
            client_request_id,
            action,
            feedback,
            feedback_scope,
        })
        .map_err(protocol_error)?;
        self.client
            .authenticated_json_scoped::<PromptOptimizationDetail>(
                Method::POST,
                &format!("/v1/prompt-optimizations/{id}/review-decision"),
                Some(body),
                Some(client_request_id),
                scope,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::api::session::test_support::MemoryRefreshTokenStore;
    use crate::runtime::api::{ApiClientConfig, DeviceIdentity, SessionManager, TokenSet};
    use reqwest::Url;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{mpsc, Arc};
    use std::thread;
    use std::time::Duration;
    use uuid::Uuid;

    const DETAIL_RESPONSE: &str = r#"{"request_id":"detail","data":{"id":"job-a","status":"paused","phase":"paused","run_mode":"automatic","focus_mode":"balanced","max_rounds":2,"current_round":1,"completed_rounds":0,"target_score":90,"baseline_score":null,"best_score":null,"best_round_no":null,"progress_percent":10,"result_score":null,"result_round_no":null},"error":null,"meta":null}"#;
    const TERMINAL_RESPONSE: &str = r#"{"request_id":"terminal","data":null,"error":{"code":"session_invalid","message":"revoked","details":null},"meta":null}"#;

    fn tokens(access: &str, refresh: &str) -> TokenSet {
        TokenSet {
            access_token: access.to_string(),
            access_expires_in_seconds: 1800,
            refresh_token: refresh.to_string(),
            refresh_expires_at: "2099-01-01T00:00:00Z".to_string(),
            token_type: "X-Token".to_string(),
        }
    }

    fn client_for(base_url: String) -> ApiClient {
        ApiClient::new(
            ApiClientConfig {
                base_url: Url::parse(&base_url).unwrap(),
                app_version: "1.0.18".to_string(),
                timeout: Duration::from_secs(2),
            },
            DeviceIdentity {
                id: Uuid::new_v4().to_string(),
                name: "prompt-optimization-test".to_string(),
                platform: "macos".to_string(),
            },
            Arc::new(SessionManager::new(Arc::new(
                MemoryRefreshTokenStore::default(),
            ))),
        )
        .unwrap()
    }

    fn blocked_response(
        status: &'static str,
        body: &'static str,
    ) -> (String, mpsc::Receiver<String>, mpsc::Sender<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (request_sender, request_receiver) = mpsc::channel();
        let (release_sender, release_receiver) = mpsc::channel();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut chunk = [0_u8; 1024];
            while !request.windows(4).any(|value| value == b"\r\n\r\n") {
                let count = stream.read(&mut chunk).unwrap();
                if count == 0 {
                    break;
                }
                request.extend_from_slice(&chunk[..count]);
            }
            request_sender
                .send(String::from_utf8_lossy(&request).to_string())
                .unwrap();
            release_receiver.recv().unwrap();
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len(),
            );
            stream.write_all(response.as_bytes()).unwrap();
        });
        (
            format!("http://{address}/"),
            request_receiver,
            release_sender,
        )
    }

    fn immediate_response(status: &'static str, body: &'static str) -> String {
        let (url, request_receiver, release_sender) = blocked_response(status, body);
        thread::spawn(move || {
            request_receiver.recv().unwrap();
            release_sender.send(()).unwrap();
        });
        url
    }

    #[test]
    fn blocked_account_a_action_keeps_a_token_after_account_b_is_installed() {
        let (url, request_receiver, release_sender) =
            blocked_response("200 OK", DETAIL_RESPONSE);
        let client = client_for(url);
        let scope_a = client
            .session()
            .install_tokens_for_user(&tokens("access-a", "refresh-a"), "user-a")
            .unwrap();
        let worker_api = PromptOptimizationApi::new(client.clone());
        let worker_scope = scope_a.clone();
        let worker =
            thread::spawn(move || worker_api.pause_scoped("job-a", &worker_scope));

        let request = request_receiver.recv().unwrap().to_ascii_lowercase();
        let scope_b = client
            .session()
            .install_tokens_for_user(&tokens("access-b", "refresh-b"), "user-b")
            .unwrap();
        release_sender.send(()).unwrap();

        assert_eq!(worker.join().unwrap().unwrap().id, "job-a");
        assert!(request.contains("x-token: access-a"));
        assert!(!request.contains("x-token: access-b"));
        assert_eq!(
            client.session().access_token_for_scope(&scope_b).unwrap(),
            "access-b",
        );
    }

    #[test]
    fn terminal_get_and_action_clear_only_their_captured_session() {
        for action in ["get", "pause"] {
            let client = client_for(immediate_response(
                "401 Unauthorized",
                TERMINAL_RESPONSE,
            ));
            let scope = client
                .session()
                .install_tokens_for_user(&tokens("access-a", "refresh-a"), "user-a")
                .unwrap();
            let api = PromptOptimizationApi::new(client.clone());
            let error = if action == "get" {
                api.get_scoped("job-a", &scope).unwrap_err()
            } else {
                api.pause_scoped("job-a", &scope).unwrap_err()
            };

            assert!(error.is_terminal_session_error());
            assert!(client.session().access().is_none());
            assert!(!client.session().has_refresh_token().unwrap());
        }
    }

    #[test]
    fn late_terminal_action_from_a_cannot_clear_account_b() {
        let (url, request_receiver, release_sender) =
            blocked_response("401 Unauthorized", TERMINAL_RESPONSE);
        let client = client_for(url);
        let scope_a = client
            .session()
            .install_tokens_for_user(&tokens("access-a", "refresh-a"), "user-a")
            .unwrap();
        let worker_api = PromptOptimizationApi::new(client.clone());
        let worker = thread::spawn(move || worker_api.cancel_scoped("job-a", &scope_a));
        request_receiver.recv().unwrap();

        let scope_b = client
            .session()
            .install_tokens_for_user(&tokens("access-b", "refresh-b"), "user-b")
            .unwrap();
        release_sender.send(()).unwrap();
        let error = worker.join().unwrap().unwrap_err();

        assert!(error.is_terminal_session_error());
        assert_eq!(
            client.session().access_token_for_scope(&scope_b).unwrap(),
            "access-b",
        );
    }
}
