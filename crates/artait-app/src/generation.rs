//! 生图任务构建、元数据持久化、模式配置映射（re-export from artait-service）。
#[allow(unused_imports)]
pub use artait_service::generation::*;

use crate::ui::AppState;

// ── UI 联动（依赖 Slint，保留在 app 层）──────────────────────────────────

pub fn set_gallery_generating_count_for_mode(state: &AppState, mode: &str, count: i32) {
    match mode {
        "scene" => state.set_gallery_generating_scene_count(count),
        "character" => state.set_gallery_generating_character_count(count),
        "ui_concept" => state.set_gallery_generating_ui_count(count),
        "effect" => state.set_gallery_generating_effect_count(count),
        "animation_scene" => state.set_gallery_generating_animation_scene_count(count),
        "animation_character" => state.set_gallery_generating_animation_character_count(count),
        "character_turnaround" => state.set_gallery_generating_character_turnaround_count(count),
        "video" => state.set_gallery_generating_video_count(count),
        _ => {}
    }
}
