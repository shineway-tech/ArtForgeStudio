//! Request leases and sensitive values for account-group billing operations.

use super::SessionScope;
use serde::{Deserialize, Deserializer};
use zeroize::Zeroize;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct GroupRequestScope {
    pub(crate) session: SessionScope,
    pub(crate) account_group_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct BillingScope {
    pub(crate) request: GroupRequestScope,
    pub(crate) context_epoch: u64,
}

pub(crate) struct SecretString(String);

impl SecretString {
    pub(crate) fn new(value: String) -> Self {
        Self(value)
    }

    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for SecretString {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SecretString([REDACTED])")
    }
}

impl Drop for SecretString {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

pub(crate) fn deserialize_secret_string<'de, D>(
    deserializer: D,
) -> Result<SecretString, D::Error>
where
    D: Deserializer<'de>,
{
    String::deserialize(deserializer).map(SecretString::new)
}

// Task 8's durable recovery-row read/write path must call this boundary before resuming work.
pub(crate) fn require_saved_group(expected: &str, actual: &str) -> Result<(), super::ApiError> {
    if !expected.is_empty() && expected == actual {
        return Ok(());
    }
    Err(super::ApiError::Protocol {
        message: "服务端资源的计费账号与本地恢复记录不一致".to_string(),
        request_id: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::api::SessionScope;

    const TEST_GROUP_ID: &str = "11111111-1111-4111-8111-111111111111";
    const OTHER_GROUP_ID: &str = "22222222-2222-4222-8222-222222222222";

    #[test]
    fn secret_string_exposes_only_explicitly_and_redacts_debug() {
        let value = SecretString::new("continuation-secret".to_string());
        assert_eq!(value.expose(), "continuation-secret");
        assert_eq!(format!("{value:?}"), "SecretString([REDACTED])");
    }

    #[test]
    fn billing_scope_keeps_authentication_and_context_epochs_distinct() {
        let session = SessionScope {
            owner_user_id: "11111111-1111-4111-8111-111111111111".to_string(),
            auth_epoch: 7,
        };
        let scope = BillingScope {
            request: GroupRequestScope {
                session,
                account_group_id: "22222222-2222-4222-8222-222222222222".to_string(),
            },
            context_epoch: 9,
        };
        assert_eq!(scope.request.session.auth_epoch, 7);
        assert_eq!(scope.context_epoch, 9);
    }

    #[test]
    fn saved_payer_rejects_empty_and_mismatched_server_groups() {
        assert!(require_saved_group(TEST_GROUP_ID, "").is_err());
        assert!(require_saved_group(TEST_GROUP_ID, OTHER_GROUP_ID).is_err());
        assert!(require_saved_group(TEST_GROUP_ID, TEST_GROUP_ID).is_ok());
    }
}
