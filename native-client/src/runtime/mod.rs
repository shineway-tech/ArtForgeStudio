use anyhow::{anyhow, Context, Result};
use chrono::{Datelike, Duration as ChronoDuration, Local, NaiveDate};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use slint::{Image, Model, ModelRc, SharedString, VecModel, Weak};
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::sync::{
    mpsc::{self, TryRecvError},
    Arc, Mutex,
};
use std::time::{Duration, Instant};
use uuid::Uuid;

use crate::drag_preview;

slint::include_modules!();

include!("model.rs");

mod api;
use api::*;
mod app;
#[path = "callbacks/auth.rs"]
mod auth_callbacks;
use auth_callbacks::*;
#[path = "callbacks/payment.rs"]
mod payment_callbacks;
use payment_callbacks::*;
#[path = "callbacks/credits.rs"]
mod credit_callbacks;
use credit_callbacks::*;
#[path = "callbacks/custom_prompt.rs"]
mod custom_prompt_callbacks;
use custom_prompt_callbacks::*;
#[path = "callbacks/infinite_canvas.rs"]
mod infinite_canvas_callbacks;
use infinite_canvas_callbacks::*;
mod payment_window;
use payment_window::*;
mod agreement_window;
use agreement_window::*;
#[path = "callbacks/generation.rs"]
mod generation_callbacks;
use generation_callbacks::*;
#[path = "callbacks/notification.rs"]
mod notification_callbacks;
use notification_callbacks::*;
#[path = "callbacks/model_catalog.rs"]
mod model_catalog_callbacks;
use model_catalog_callbacks::*;
#[path = "callbacks/reference.rs"]
mod reference_callbacks;
use reference_callbacks::*;
#[path = "callbacks/viewer.rs"]
mod viewer_callbacks;
use viewer_callbacks::*;
mod configuration;
use configuration::*;
#[path = "features/inspiration.rs"]
mod inspiration;
use inspiration::*;
#[path = "features/viewer.rs"]
mod viewer;
use viewer::*;
#[path = "generation/controller.rs"]
mod generation_controller;
use generation_controller::*;
#[path = "generation/backend.rs"]
mod backend_generation;
use backend_generation::*;
#[path = "generation/poll.rs"]
mod generation_poll;
use generation_poll::*;
#[path = "generation/state.rs"]
mod generation_state;
use generation_state::*;
#[path = "presentation/sync.rs"]
mod sync;
use sync::*;
#[path = "presentation/theme.rs"]
mod theme;
use theme::*;
mod prompt;
use prompt::*;
#[path = "services/image_processing.rs"]
mod image_processing;
use image_processing::*;
#[path = "storage/local_store.rs"]
mod local_store;
use local_store::*;
#[path = "storage/paths.rs"]
mod paths;
use paths::*;
#[path = "storage/recovery.rs"]
mod recovery;
use recovery::*;
mod utilities;
use utilities::*;

pub(crate) fn run() -> Result<()> {
    app::run()
}

include!("tests.rs");
