//! Request leases and sensitive values for account-group billing operations.

use super::SessionScope;
use serde::{Deserialize, Deserializer};
use zeroize::Zeroize;
use std::sync::{Arc, Mutex};
use super::{AccountGroupChoice, AccountSnapshot, ApiError, KnownCapability};

#[derive(Clone, Default)]
pub(crate) struct BillingContextManager { state: Arc<Mutex<BillingContextState>>, upgrade: super::UpgradeLatch }
#[derive(Default)]
struct BillingContextState {
    epoch: u64, closed: bool, session: Option<SessionScope>,
    pending: Option<PendingBillingSwitch>, current: Option<ConfirmedBillingContext>,
}
struct ConfirmedBillingContext { scope: BillingScope, snapshot: AccountSnapshot, usable: bool }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PreviousBillingAuthority { StillValid, Invalidated }
#[derive(Clone, PartialEq, Eq)]
struct PendingBillingSwitch { scope: BillingScope, device: String, rollback: PreviousBillingAuthority }
pub(crate) struct BillingSwitchTicket {
    manager: BillingContextManager, pending: PendingBillingSwitch, armed: bool,
}
pub(crate) struct StagedBillingConfirmation {
    pending: PendingBillingSwitch, snapshot: AccountSnapshot,
}
impl BillingSwitchTicket {
    pub(crate) fn proposed_scope(&self) -> &BillingScope { &self.pending.scope }
    pub(crate) fn device_installation_id(&self) -> &str { &self.pending.device }
    // Only the coordinator calls this after a durable result denied by the upgrade fence.
    pub(crate) fn disarm_without_publication(mut self) { self.armed = false; }
    pub(crate) fn suppress_rollback(&mut self) { self.armed = false; }
}
impl Drop for BillingSwitchTicket {
    fn drop(&mut self) {
        if self.armed {
            let _ = self.manager.upgrade.apply_if_open(|| self.manager.abort_matching(&self.pending));
        }
    }
}
impl StagedBillingConfirmation {
    pub(crate) fn scope(&self) -> &BillingScope { &self.pending.scope }
}
impl BillingContextState {
    fn advance(&mut self) -> Result<u64, ApiError> {
        if self.closed { return Err(BillingContextManager::error("计费上下文已关闭")); }
        let Some(epoch) = self.epoch.checked_add(1) else {
            self.closed = true; self.pending = None; self.current = None; self.session = None;
            return Err(BillingContextManager::error("计费上下文计数已耗尽"));
        };
        self.epoch = epoch;
        Ok(epoch)
    }
}
impl BillingContextManager {
    pub(crate) fn with_upgrade_latch(upgrade: super::UpgradeLatch) -> Self {
        Self { state: Arc::new(Mutex::new(BillingContextState::default())), upgrade }
    }
    pub(crate) fn choose_candidate<'a>(choices: &'a [AccountGroupChoice], saved: Option<&str>, suggestion: Option<&str>) -> Result<&'a AccountGroupChoice, ApiError> {
        let selectable = |choice: &&AccountGroupChoice| choice.selectable && choice.readable_context && choice.group_status == "active" && matches!(choice.role.as_str(), "owner" | "member");
        suggestion.and_then(|id| choices.iter().filter(selectable).find(|choice| choice.group_id == id))
            .or_else(|| saved.and_then(|id| choices.iter().filter(selectable).find(|choice| choice.group_id == id)))
            .or_else(|| choices.iter().filter(selectable).find(|choice| choice.role == "owner"))
            .or_else(|| choices.iter().find(|choice| choice.role == "owner" && choice.group_status == "frozen" && choice.readable_context && !choice.selectable))
            .ok_or_else(|| Self::error("没有可读取的计费账号"))
    }
    fn error(message: &str) -> ApiError { ApiError::LocalState { message: message.into() } }
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, BillingContextState>, ApiError> {
        self.state.lock().map_err(|poison| {
            let mut state = poison.into_inner(); state.closed = true; state.current = None; state.pending = None;
            Self::error("计费上下文锁不可用")
        })
    }
    pub(crate) fn bind_authenticated_session(&self, session: SessionScope) -> Result<(), ApiError> {
        let mut state = self.lock()?;
        state.advance()?;
        state.session = Some(session); state.pending = None; state.current = None;
        Ok(())
    }
    pub(crate) fn invalidate_for_auth_change(&self) {
        if let Ok(mut state) = self.lock() {
            let _ = state.advance();
            state.session = None; state.pending = None; state.current = None;
        }
    }
    pub(crate) fn begin_switch(&self, session: &SessionScope, device: &str, group: &str, rollback: PreviousBillingAuthority) -> Result<BillingSwitchTicket, ApiError> {
        let mut state = self.lock()?;
        if state.session.as_ref() != Some(session) || device.is_empty()
            || uuid::Uuid::parse_str(group).map(|id| id.to_string() != group).unwrap_or(true) {
            return Err(Self::error("计费切换会话或标识不匹配"));
        }
        let epoch = state.advance()?;
        if rollback == PreviousBillingAuthority::Invalidated {
            if let Some(current) = &mut state.current { current.usable = false; }
        }
        let pending = PendingBillingSwitch {
            scope: BillingScope { request: GroupRequestScope { session: session.clone(), account_group_id: group.into() }, context_epoch: epoch },
            device: device.into(), rollback,
        };
        state.pending = Some(pending.clone());
        Ok(BillingSwitchTicket { manager: self.clone(), pending, armed: true })
    }
    pub(crate) fn stage_confirmation(&self, ticket: &BillingSwitchTicket, choice: AccountGroupChoice, snapshot: AccountSnapshot) -> Result<StagedBillingConfirmation, ApiError> {
        {
            let state = self.lock()?;
            if state.closed || !Arc::ptr_eq(&self.state, &ticket.manager.state)
                || state.pending.as_ref() != Some(&ticket.pending)
                || state.session.as_ref() != Some(&ticket.pending.scope.request.session) {
                return Err(ApiError::Protocol { message: "计费切换票据已失效".into(), request_id: None });
            }
        }
        let same_choice = choice.group_id == snapshot.billing_group.group_id
            && choice.role == snapshot.billing_group.role
            && choice.group_status == snapshot.billing_group.group_status
            && choice.selectable == snapshot.billing_group.selectable
            && choice.readable_context == snapshot.billing_group.readable_context
            && choice.member_id == snapshot.billing_group.member_id
            && choice.relationship_status == snapshot.billing_group.relationship_status
            && choice.group_version == snapshot.billing_group.group_version
            && choice.membership_version == snapshot.billing_group.membership_version
            && choice.capabilities.iter().collect::<std::collections::BTreeSet<_>>()
                == snapshot.capabilities.iter().collect::<std::collections::BTreeSet<_>>();
        let validation = super::validate_snapshot_context(&snapshot, ticket.proposed_scope());
        if !same_choice || validation.is_err() {
            let _ = self.upgrade.apply_if_open(|| self.abort_matching(&ticket.pending));
            return Err(ApiError::Protocol { message: "账号确认与已选团队不一致".into(), request_id: None });
        }
        Ok(StagedBillingConfirmation { pending: ticket.pending.clone(), snapshot })
    }
    pub(crate) fn publish_persisted(&self, mut ticket: BillingSwitchTicket, staged: StagedBillingConfirmation) {
        if let Ok(mut state) = self.lock() {
            if !state.closed && Arc::ptr_eq(&self.state, &ticket.manager.state)
                && state.pending.as_ref() == Some(&ticket.pending) && staged.pending == ticket.pending {
                state.current = Some(ConfirmedBillingContext { scope: staged.pending.scope, snapshot: staged.snapshot, usable: true });
                state.pending = None;
            }
        }
        ticket.armed = false;
    }
    fn abort_matching(&self, pending: &PendingBillingSwitch) {
        if let Ok(mut state) = self.lock() {
            if state.pending.as_ref() != Some(pending) { return; }
            state.pending = None;
            if let Ok(epoch) = state.advance() {
                if let Some(current) = &mut state.current {
                    current.scope.context_epoch = epoch;
                    if pending.rollback == PreviousBillingAuthority::Invalidated { current.usable = false; }
                }
            }
        }
    }
    pub(crate) fn abort_switch(&self, mut ticket: BillingSwitchTicket) {
        if Arc::ptr_eq(&self.state, &ticket.manager.state) { self.abort_matching(&ticket.pending); }
        ticket.armed = false;
    }
    pub(crate) fn confirmed_scope(&self) -> Option<BillingScope> {
        let state = self.lock().ok()?;
        if state.closed || state.pending.is_some() { return None; }
        state.current.as_ref().filter(|current| current.usable).map(|current| current.scope.clone())
    }
    pub(crate) fn invalidate_current_billing(&self) {
        if let Ok(mut state) = self.lock() {
            if state.advance().is_ok() {
                state.pending = None;
                if let Some(current) = &mut state.current { current.usable = false; }
            }
        }
    }
    pub(crate) fn confirmed_snapshot(&self) -> Option<AccountSnapshot> {
        let state = self.lock().ok()?;
        if state.closed || state.pending.is_some() { return None; }
        state.current.as_ref().filter(|current| current.usable).map(|current| current.snapshot.clone())
    }
    // A balance refresh is not a payer switch or a capability reauthorization.
    // The caller's credit-sync epoch guards ordering before this short publication.
    // Called inside AppContext::apply_user_completion, which holds the upgrade
    // fence. Do not query that fence again: its mutex is not reentrant.
    pub(crate) fn refresh_financial_snapshot(&self, scope: &BillingScope, snapshot: &AccountSnapshot) -> bool {
        if super::validate_snapshot_context(snapshot, scope).is_err() {
            return false;
        }
        let Ok(mut state) = self.lock() else { return false; };
        if state.closed || state.pending.is_some() { return false; }
        let Some(current) = state.current.as_mut().filter(|current| current.usable && &current.scope == scope) else { return false; };
        if current.snapshot.billing_group.role != snapshot.billing_group.role
            || current.snapshot.billing_group.member_id != snapshot.billing_group.member_id {
            return false;
        }
        current.snapshot.credits = snapshot.credits.clone();
        current.snapshot.quota = snapshot.quota.clone();
        current.snapshot.billing_group.quota = snapshot.billing_group.quota.clone();
        true
    }
    pub(crate) fn current_scope(&self, capability: KnownCapability) -> Result<BillingScope, ApiError> {
        let state = self.lock()?;
        let current = state.current.as_ref().filter(|current| !state.closed && state.pending.is_none() && current.usable)
            .ok_or_else(|| Self::error("计费账号尚未确认"))?;
        if !current.snapshot.billing_group.has_capability(capability) {
            return Err(ApiError::Http {
                status: 403,
                code: "account_group_capability_denied".into(),
                message: "当前账号不允许此操作".into(),
                request_id: None,
                details: None,
            });
        }
        if current.snapshot.read_only && matches!(capability, KnownCapability::Bill | KnownCapability::Purchase | KnownCapability::Redeem) { return Err(Self::error("当前账号只读")); }
        Ok(current.scope.clone())
    }
    pub(crate) fn billable_scope(&self) -> Result<BillingScope, ApiError> { self.current_scope(KnownCapability::Bill) }
    pub(crate) fn is_current(&self, scope: &BillingScope) -> bool { self.confirmed_scope().as_ref() == Some(scope) }
}

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
    fn financial_refresh_completes_inside_upgrade_fence_without_relocking() {
        // Reacquiring the upgrade mutex in the refresh makes this time out.
        let latch = super::super::UpgradeLatch::default();
        let manager = BillingContextManager::with_upgrade_latch(latch.clone());
        let session = SessionScope { owner_user_id: TEST_GROUP_ID.into(), auth_epoch: 8 };
        manager.bind_authenticated_session(session.clone()).unwrap();
        let ticket = manager.begin_switch(&session, "device", TEST_GROUP_ID, PreviousBillingAuthority::StillValid).unwrap();
        let mut snapshot: AccountSnapshot = serde_json::from_value(serde_json::json!({
            "user":{"id":TEST_GROUP_ID,"email_masked":"a***@example.com","nickname":null,"status":"active","registered_at":"2026-09-07T00:00:00Z"},
            "read_only":false,"capabilities":["bill","read_group_finance"],"membership":null,"entitlement":{},"quota":null,
            "credits":{"available":"100","reserved":"0","lifetime_granted":"100","lifetime_spent":"0","version":"1"},
            "billing_group":{"group_id":TEST_GROUP_ID,"name":"owner","group_status":"active","role":"owner","member_id":null,"relationship_status":null,"readable_context":true,"selectable":true,"group_version":"1","membership_version":null,"capabilities":["bill","read_group_finance"],"quota":null}
        })).unwrap();
        let staged = manager.stage_confirmation(&ticket, snapshot.billing_group.clone(), snapshot.clone()).unwrap();
        manager.publish_persisted(ticket, staged);
        let scope = manager.confirmed_scope().unwrap();
        let credits = snapshot.credits.as_mut().unwrap();
        credits.available = "95".into();
        credits.lifetime_spent = "5".into();
        credits.version = "2".into();
        let (sender, receiver) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let refreshed = latch.apply_if_open(|| manager.refresh_financial_snapshot(&scope, &snapshot)).unwrap();
            sender.send((refreshed, manager.confirmed_snapshot().unwrap())).unwrap();
        });
        let (refreshed, current) = receiver.recv_timeout(std::time::Duration::from_secs(5))
            .expect("financial refresh must not deadlock inside the completion fence");
        worker.join().unwrap();
        assert!(refreshed);
        assert_eq!(current.credits.unwrap().available, "95");
        assert_eq!(current.billing_group.role, "owner");
    }

    #[test]
    fn core_billing_switch_hides_pending_and_invalidated_abort_never_revives_authority() {
        let manager = BillingContextManager::default();
        let session = SessionScope { owner_user_id: TEST_GROUP_ID.into(), auth_epoch: 8 };
        manager.bind_authenticated_session(session.clone()).unwrap();
        let ticket = manager.begin_switch(&session, "device", TEST_GROUP_ID, PreviousBillingAuthority::StillValid).unwrap();
        assert!(manager.confirmed_scope().is_none());
        let snapshot: AccountSnapshot = serde_json::from_value(serde_json::json!({
            "user":{"id":TEST_GROUP_ID,"email_masked":"a***@example.com","nickname":null,"status":"active","registered_at":"2026-09-07T00:00:00Z"},
            "read_only":false,"capabilities":["bill"],"membership":null,"entitlement":{},"credits":null,"quota":null,
            "billing_group":{"group_id":TEST_GROUP_ID,"name":"owner","group_status":"active","role":"owner","member_id":null,"relationship_status":null,"readable_context":true,"selectable":true,"group_version":"1","membership_version":null,"capabilities":["bill"],"quota":null}
        })).unwrap();
        let staged = manager.stage_confirmation(&ticket, snapshot.billing_group.clone(), snapshot.clone()).unwrap();
        assert!(manager.confirmed_scope().is_none());
        manager.publish_persisted(ticket, staged);
        let previous = manager.billable_scope().unwrap();
        // A missing capability must remain a permission error, not a disk failure.
        let denied = manager.current_scope(KnownCapability::Redeem).unwrap_err();
        assert_eq!(denied.code(), Some("account_group_capability_denied"));
        let stale = manager.begin_switch(&session, "device", OTHER_GROUP_ID, PreviousBillingAuthority::StillValid).unwrap();
        let newer = manager.begin_switch(&session, "device", OTHER_GROUP_ID, PreviousBillingAuthority::Invalidated).unwrap();
        manager.abort_switch(stale);
        assert!(manager.confirmed_scope().is_none());
        manager.abort_switch(newer);
        assert!(manager.confirmed_scope().is_none());
        assert!(manager.billable_scope().is_err());
        assert!(!manager.is_current(&previous));
        manager.bind_authenticated_session(SessionScope { auth_epoch: 9, ..session.clone() }).unwrap();
        assert!(manager.begin_switch(&session, "device", TEST_GROUP_ID, PreviousBillingAuthority::StillValid).is_err());
    }

    #[test]
    fn core_billing_selection_prefers_suggestion_saved_owned_then_readable_frozen() {
        fn choice(id: &str, role: &str, status: &str, selectable: bool) -> super::super::AccountGroupChoice {
            serde_json::from_value(serde_json::json!({
                "group_id":id,"name":"fixture","group_status":status,"role":role,
                "member_id":null,"relationship_status":null,"readable_context":true,
                "selectable":selectable,"group_version":"1","membership_version":null,
                "capabilities":[],"quota":null
            })).unwrap()
        }
        let choices = [choice(TEST_GROUP_ID, "owner", "active", true), choice(OTHER_GROUP_ID, "member", "active", true)];
        assert_eq!(BillingContextManager::choose_candidate(&choices, Some(TEST_GROUP_ID), Some(OTHER_GROUP_ID)).unwrap().group_id, OTHER_GROUP_ID);
        assert_eq!(BillingContextManager::choose_candidate(&choices, Some(OTHER_GROUP_ID), None).unwrap().group_id, OTHER_GROUP_ID);
        assert_eq!(BillingContextManager::choose_candidate(&choices, None, None).unwrap().group_id, TEST_GROUP_ID);
        let frozen = [choice(TEST_GROUP_ID, "owner", "frozen", false), choice(OTHER_GROUP_ID, "member", "frozen", false)];
        assert_eq!(BillingContextManager::choose_candidate(&frozen, Some(OTHER_GROUP_ID), None).unwrap().group_id, TEST_GROUP_ID);
    }

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
