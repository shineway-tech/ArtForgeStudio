use super::{ApiClient, ApiError};
use reqwest::Method;
use serde::Deserialize;
use serde_json::Value;

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct ServerNotification {
    pub(crate) id: String,
    #[serde(rename = "type")]
    pub(crate) notification_type: String,
    pub(crate) title: String,
    pub(crate) body: String,
    pub(crate) metadata: Value,
    pub(crate) created_at: String,
    pub(crate) read_at: Option<String>,
}

#[derive(Deserialize)]
struct NotificationList {
    items: Vec<ServerNotification>,
}

#[derive(Clone)]
pub(crate) struct NotificationsApi { client: ApiClient }

impl NotificationsApi {
    pub(crate) fn new(client: ApiClient) -> Self { Self { client } }

    pub(crate) fn list(&self) -> Result<Vec<ServerNotification>, ApiError> {
        self.client.authenticated_json::<NotificationList>(
            Method::GET, "/v1/notifications?limit=50", None, None,
        ).map(|response| response.data.items)
    }

    pub(crate) fn mark_read(&self, id: &str) -> Result<(), ApiError> {
        self.client.authenticated_json::<ServerNotification>(
            Method::POST,
            &format!("/v1/notifications/{id}/read"),
            None,
            None,
        )?;
        Ok(())
    }

    pub(crate) fn mark_all_read(&self) -> Result<(), ApiError> {
        self.client.authenticated_json::<Value>(
            Method::POST, "/v1/notifications/read_all", None, None,
        )?;
        Ok(())
    }
}
