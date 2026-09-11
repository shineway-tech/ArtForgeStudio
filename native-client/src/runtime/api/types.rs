use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct ApiProblem {
    pub(crate) code: String,
    pub(crate) message: String,
    #[serde(default)]
    pub(crate) details: Option<Value>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ApiMeta {
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub(crate) next_cursor: Option<String>,
}

fn deserialize_required_nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct ApiEnvelope<T> {
    pub(crate) request_id: String,
    pub(crate) data: Option<T>,
    pub(crate) error: Option<ApiProblem>,
    pub(crate) meta: Option<ApiMeta>,
}

#[derive(Clone, Debug)]
pub(crate) struct ApiResponse<T> {
    pub(crate) request_id: String,
    pub(crate) data: T,
    pub(crate) meta: Option<ApiMeta>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct TokenSet {
    pub(crate) access_token: String,
    pub(crate) access_expires_in_seconds: u64,
    pub(crate) refresh_token: String,
    pub(crate) refresh_expires_at: String,
    pub(crate) token_type: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct RefreshRequest<'a> {
    pub(crate) refresh_token: &'a str,
    pub(crate) device_id: &'a str,
    pub(crate) app_version: &'a str,
}
