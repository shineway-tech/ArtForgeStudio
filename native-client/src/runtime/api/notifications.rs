use super::{ApiClient, ApiError, SessionScope};
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
    #[serde(default)]
    next_cursor: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct NotificationPage {
    pub(crate) items: Vec<ServerNotification>,
    pub(crate) next_cursor: Option<String>,
}

#[derive(Clone)]
pub(crate) struct NotificationsApi {
    client: ApiClient,
}

impl NotificationsApi {
    pub(crate) fn new(client: ApiClient) -> Self {
        Self { client }
    }

    pub(crate) fn list(&self) -> Result<Vec<ServerNotification>, ApiError> {
        self.client
            .authenticated_json::<NotificationList>(
                Method::GET,
                "/v1/notifications?limit=50",
                None,
                None,
            )
            .map(|response| response.data.items)
    }

    pub(crate) fn list_page_scoped(
        &self,
        cursor: Option<&str>,
        scope: &SessionScope,
    ) -> Result<NotificationPage, ApiError> {
        let path = notification_list_path(cursor);
        self.client
            .authenticated_json_scoped::<NotificationList>(
                Method::GET,
                &path,
                None,
                None,
                scope,
            )
            .map(|response| NotificationPage {
                items: response.data.items,
                next_cursor: response.data.next_cursor,
            })
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

    pub(crate) fn mark_read_scoped(
        &self,
        id: &str,
        scope: &SessionScope,
    ) -> Result<(), ApiError> {
        self.client
            .authenticated_json_scoped::<ServerNotification>(
                Method::POST,
                &format!("/v1/notifications/{id}/read"),
                None,
                None,
                scope,
            )?;
        Ok(())
    }

    pub(crate) fn mark_all_read(&self) -> Result<(), ApiError> {
        self.client.authenticated_json::<Value>(
            Method::POST,
            "/v1/notifications/read_all",
            None,
            None,
        )?;
        Ok(())
    }

    pub(crate) fn mark_all_read_scoped(&self, scope: &SessionScope) -> Result<(), ApiError> {
        self.client.authenticated_json_scoped::<Value>(
            Method::POST,
            "/v1/notifications/read_all",
            None,
            None,
            scope,
        )?;
        Ok(())
    }

    pub(crate) fn delete(&self, id: &str) -> Result<(), ApiError> {
        self.client.authenticated_json::<Value>(
            Method::DELETE,
            &format!("/v1/notifications/{id}"),
            None,
            None,
        )?;
        Ok(())
    }

    pub(crate) fn delete_scoped(
        &self,
        id: &str,
        scope: &SessionScope,
    ) -> Result<(), ApiError> {
        self.client.authenticated_json_scoped::<Value>(
            Method::DELETE,
            &format!("/v1/notifications/{id}"),
            None,
            None,
            scope,
        )?;
        Ok(())
    }

    pub(crate) fn delete_all(&self) -> Result<(), ApiError> {
        self.client
            .authenticated_json::<Value>(Method::DELETE, "/v1/notifications", None, None)?;
        Ok(())
    }

    pub(crate) fn delete_all_scoped(&self, scope: &SessionScope) -> Result<(), ApiError> {
        self.client.authenticated_json_scoped::<Value>(
            Method::DELETE,
            "/v1/notifications",
            None,
            None,
            scope,
        )?;
        Ok(())
    }
}

fn notification_list_path(cursor: Option<&str>) -> String {
    match cursor.filter(|value| !value.is_empty()) {
        Some(cursor) => format!("/v1/notifications?limit=50&cursor={cursor}"),
        None => "/v1/notifications?limit=50".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notification_page_path_only_adds_a_non_empty_cursor() {
        assert_eq!(notification_list_path(None), "/v1/notifications?limit=50");
        assert_eq!(notification_list_path(Some("")), "/v1/notifications?limit=50");
        assert_eq!(
            notification_list_path(Some("42")),
            "/v1/notifications?limit=50&cursor=42",
        );
    }
}
