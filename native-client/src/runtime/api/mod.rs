// Wire DTOs intentionally mirror complete server envelopes even when the current UI
// only renders a subset of their fields.
#![allow(dead_code)]

mod account;
mod auth;
mod billing_context;
mod client;
mod device;
mod error;
mod generation;
mod membership;
mod notifications;
mod payment;
mod prompt_optimization;
mod session;
mod team;
mod types;

#[cfg(test)]
mod cross_stack_tests;

pub(crate) use account::*;
pub(crate) use auth::*;
pub(crate) use billing_context::*;
// Payer selection enters through ApiClient::billing_json_scoped; identity transport never adds it.
pub(crate) use client::*;
pub(crate) use device::*;
pub(crate) use error::*;
pub(crate) use generation::*;
pub(crate) use membership::*;
pub(crate) use notifications::*;
pub(crate) use payment::*;
pub(crate) use prompt_optimization::*;
pub(crate) use session::*;
// Team, owner-admin, finance-summary, and reauthentication routes are identity scoped.
#[allow(unused_imports)]
pub(crate) use team::*;
pub(crate) use types::*;

use std::path::Path;
use std::sync::Arc;

#[derive(Clone)]
pub(crate) struct BackendRuntime {
    pub(crate) api: ApiClient,
}

impl BackendRuntime {
    pub(crate) fn new(data_dir: &Path) -> Result<Self, ApiError> {
        let device = DeviceIdentity::load_or_create(data_dir)?;
        let session = Arc::new(SessionManager::with_file_store(data_dir));
        let api = ApiClient::new(ApiClientConfig::from_environment()?, device, session)?;
        Ok(Self { api })
    }
}
