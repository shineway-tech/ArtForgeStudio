//! 场景库回调 handler。

use crate::callbacks::CbCtx;
use crate::ui::{AppShell, AppState, SceneCardItem};
use slint::{ComponentHandle, ModelRc, VecModel};

pub(crate) fn init(ctx: &CbCtx, _app: &AppShell) {
    let state = _app.global::<AppState>();
    let app_weak = ctx.app.clone();
    let scene_store = ctx.scene_store.clone();

    state.on_scene_library_create(move || {
        if let Some(app) = app_weak.upgrade() {
            let s = app.global::<AppState>();
            let mut store = scene_store.borrow_mut();
            let id = format!("scene-{}", chrono::Utc::now().timestamp_millis());
            let sc = artait_model::scene::Scene::new(id, "新场景".into());
            if store.create_scene(sc).is_ok() {
                store.flush();
                refresh_scene_list(&s, store.all_scenes());
            }
        }
    });

    {
        let app_weak2 = ctx.app.clone();
        state.on_scene_library_select(move |id| {
            let id_str = id.to_string();
            if let Some(app) = app_weak2.upgrade() {
                let s = app.global::<AppState>();
                s.set_scene_library_selected_id(id_str.into());
            }
        });
    }
}

#[allow(dead_code)]
pub(crate) fn refresh_scene_list(state: &AppState, scenes: &[artait_model::scene::Scene]) {
    let items: Vec<SceneCardItem> = scenes
        .iter()
        .map(|s| SceneCardItem {
            id: s.id.clone().into(),
            name: s.name.clone().into(),
            location: s.location.clone().into(),
            time_of_day: s.time_of_day.clone().unwrap_or_default().into(),
            atmosphere: s.atmosphere.clone().unwrap_or_default().into(),
            viewpoint_count: s.viewpoint_count() as i32,
            has_thumb: s.thumbnail_url.is_some(),
            thumb: slint::Image::default(),
        })
        .collect();
    state.set_scene_library_items(ModelRc::new(VecModel::from(items)));
}
