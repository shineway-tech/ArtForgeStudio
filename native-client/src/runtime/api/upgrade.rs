//! Process-shared one-way ordinary-work fence. The updater has separate transport.
use super::ApiError;
use std::sync::{Arc, Mutex, Condvar};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RequiredUpgrade { pub(crate) minimum_version: Option<String> }
impl RequiredUpgrade {
    pub(crate) fn from_error(error: &ApiError) -> Option<Self> {
        if !error.is_client_update_required() { return None; }
        let candidate = match error {
            ApiError::Http { details: Some(details), .. } => details.get("minimum_version").and_then(|value| value.as_str()),
            _ => None,
        };
        let minimum_version = candidate.filter(|value| {
            let parts: Vec<_> = value.split('.').collect();
            parts.len() == 3 && parts.iter().all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit())
                && (part.len() == 1 || !part.starts_with('0')) && part.parse::<u64>().is_ok())
        }).map(str::to_owned);
        Some(Self { minimum_version })
    }
    pub(crate) fn as_error(&self) -> ApiError {
        ApiError::Http { status: 426, code: "client_upgrade_required".into(),
            message: "当前客户端版本过旧，必须更新后继续使用".into(), request_id: None,
            details: self.minimum_version.as_ref().map(|version| serde_json::json!({"minimum_version": version})),
        }
    }
}
#[derive(Clone, Default)]
pub(crate) struct UpgradeLatch { inner: Arc<UpgradeLatchInner> }
#[derive(Default)]
struct UpgradeLatchInner { state: Mutex<UpgradeLatchState>, drained: Condvar }
#[derive(Default)]
struct UpgradeLatchState { required: Option<RequiredUpgrade>, counts: [usize; 4] }
struct OrdinaryPermit { inner: Arc<UpgradeLatchInner>, kind: usize, active: bool }
pub(crate) struct OrdinaryTransferPermit(OrdinaryPermit);
pub(crate) struct OrdinaryDurableCommitPermit(OrdinaryPermit);
pub(crate) struct OrdinaryBlockingEffectPermit(OrdinaryPermit);
pub(crate) struct OrdinaryExternalWorkerPermit(OrdinaryPermit);
pub(crate) struct DeferredExternalEffectPermit { inner: Arc<UpgradeLatchInner> }
impl UpgradeLatch {
    fn lock(&self) -> std::sync::MutexGuard<'_, UpgradeLatchState> {
        self.inner.state.lock().unwrap_or_else(|poison| {
            let mut state = poison.into_inner();
            if state.required.is_none() { state.required = Some(RequiredUpgrade { minimum_version: None }); }
            state
        })
    }
    fn wait_drained(&self) {
        let mut state = self.lock();
        while state.counts.iter().any(|count| *count != 0) {
            state = self.inner.drained.wait(state).unwrap_or_else(|poison| poison.into_inner());
        }
    }
    pub(crate) fn snapshot(&self) -> Option<RequiredUpgrade> { self.lock().required.clone() }
    pub(crate) fn is_tripped(&self) -> bool { self.snapshot().is_some() }
    pub(crate) fn trip(&self, required: RequiredUpgrade) {
        { let mut state = self.lock(); if state.required.is_none() { state.required = Some(required); } }
        self.wait_drained();
    }
    pub(crate) fn trip_from_ordinary_transfer<R>(&self, mut permit: OrdinaryTransferPermit, required: RequiredUpgrade, cleanup: impl FnOnce() -> R) -> R {
        {
            let mut state = self.lock();
            assert!(permit.0.active && Arc::ptr_eq(&self.inner, &permit.0.inner));
            if state.required.is_none() { state.required = Some(required); }
            state.counts[0] -= 1; permit.0.active = false;
            self.inner.drained.notify_all();
        }
        let result = cleanup();
        self.wait_drained();
        result
    }
    pub(crate) fn apply_if_open<R>(&self, apply: impl FnOnce() -> R) -> Result<R, RequiredUpgrade> {
        let state = self.lock();
        if let Some(required) = &state.required { return Err(required.clone()); }
        let result = apply();
        drop(state);
        Ok(result)
    }
    fn begin(&self, kind: usize) -> Result<OrdinaryPermit, RequiredUpgrade> {
        let mut state = self.lock();
        if let Some(required) = &state.required { return Err(required.clone()); }
        let Some(count) = state.counts[kind].checked_add(1) else {
            let required = RequiredUpgrade { minimum_version: None };
            state.required = Some(required.clone());
            return Err(required);
        };
        state.counts[kind] = count;
        Ok(OrdinaryPermit { inner: self.inner.clone(), kind, active: true })
    }
    pub(crate) fn begin_ordinary_transfer(&self) -> Result<OrdinaryTransferPermit, RequiredUpgrade> { self.begin(0).map(OrdinaryTransferPermit) }
    pub(crate) fn begin_ordinary_durable_commit(&self) -> Result<OrdinaryDurableCommitPermit, RequiredUpgrade> { self.begin(1).map(OrdinaryDurableCommitPermit) }
    pub(crate) fn begin_ordinary_blocking_effect(&self) -> Result<OrdinaryBlockingEffectPermit, RequiredUpgrade> { self.begin(2).map(OrdinaryBlockingEffectPermit) }
    pub(crate) fn begin_ordinary_external_worker(&self) -> Result<OrdinaryExternalWorkerPermit, RequiredUpgrade> { self.begin(3).map(OrdinaryExternalWorkerPermit) }
    pub(crate) fn defer_external_effect(&self) -> Result<DeferredExternalEffectPermit, RequiredUpgrade> {
        self.apply_if_open(|| DeferredExternalEffectPermit { inner: self.inner.clone() })
    }
    pub(crate) fn commit_deferred_external_effect_if_open<R>(&self, permit: DeferredExternalEffectPermit, commit: impl FnOnce() -> R) -> Result<R, RequiredUpgrade> {
        assert!(Arc::ptr_eq(&self.inner, &permit.inner));
        self.apply_if_open(commit)
    }
    pub(crate) fn commit_transfer_if_open<R>(&self, permit: &OrdinaryTransferPermit, commit: impl FnOnce() -> R) -> Result<R, RequiredUpgrade> {
        assert!(permit.0.active && Arc::ptr_eq(&self.inner, &permit.0.inner));
        self.apply_if_open(commit)
    }
}
impl OrdinaryPermit {
    fn is_cancelled(&self) -> bool {
        self.inner.state.lock().map(|state| state.required.is_some()).unwrap_or(true)
    }
}
impl Drop for OrdinaryPermit {
    fn drop(&mut self) {
        if !self.active { return; }
        let mut state = self.inner.state.lock().unwrap_or_else(|poison| poison.into_inner());
        state.counts[self.kind] -= 1; self.active = false; self.inner.drained.notify_all();
    }
}
impl OrdinaryTransferPermit { pub(crate) fn is_cancelled(&self) -> bool { self.0.is_cancelled() } }
impl OrdinaryExternalWorkerPermit { pub(crate) fn is_cancelled(&self) -> bool { self.0.is_cancelled() } }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn core_upgrade_self_retirement_closes_admission_and_waits_for_real_work() {
        let latch = UpgradeLatch::default();
        let transfer = latch.begin_ordinary_transfer().unwrap();
        let durable = latch.begin_ordinary_durable_commit().unwrap();
        std::thread::scope(|workers| {
            let (sender, receiver) = std::sync::mpsc::channel();
            let worker_latch = latch.clone();
            let worker = workers.spawn(move || {
                worker_latch.trip_from_ordinary_transfer(transfer, RequiredUpgrade { minimum_version: Some("1.2.3".into()) }, || ());
                sender.send(()).unwrap();
            });
            while !latch.is_tripped() { std::thread::yield_now(); }
            assert!(latch.apply_if_open(|| panic!("late ordinary application")).is_err());
            assert!(latch.begin_ordinary_transfer().is_err());
            assert!(receiver.try_recv().is_err());
            drop(durable);
            receiver.recv().unwrap();
            worker.join().unwrap();
        });
        assert_eq!(latch.snapshot().unwrap().minimum_version.as_deref(), Some("1.2.3"));
    }
}
