//! 首启引导（re-export from artait-service）+ UI 推送到 Slint。
pub use artait_service::onboarding::*;

use slint::{ComponentHandle, ModelRc, VecModel};

use crate::ui::{AppShell, AppState, OnboardingState};

/// 把 OnboardingDraft 推到 Slint AppState。
pub fn push_to_ui(app: &AppShell, draft: &OnboardingDraft) {
    let state = app.global::<AppState>();
    let s = OnboardingState {
        step: draft.step,
        preset: preset_id(draft.preset).into(),
        feature_flags: ModelRc::new(VecModel::from(draft.features.clone())),
        input_dir: draft.input_dir.display().to_string().into(),
        output_dir: draft.output_dir.display().to_string().into(),
        prompt_dir: draft.prompt_dir.display().to_string().into(),
        theme_id: theme_id_str(draft.theme_id).into(),
        legacy_detected: draft.legacy_path.is_some(),
        legacy_hint: draft.legacy_hint.clone().into(),
        last_error: draft.last_error.clone().into(),
    };
    state.set_onboarding(s);
}
