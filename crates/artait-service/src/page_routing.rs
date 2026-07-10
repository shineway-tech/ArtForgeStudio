//! 页面路由分类与初始页决策。

use artait_model::AppConfig;

/// 判断页面路由是否为 workspace 页面（含生图和动作序列）。
pub fn is_workspace_page(page: &str) -> bool {
    matches!(
        page,
        "scene"
            | "character"
            | "ui_concept"
            | "effect"
            | "animation_scene"
            | "animation_character"
            | "character_turnaround"
            | "action_sequence"
            | "video"
    )
}

/// 判断页面路由是否为 workspace 生图页面（不含动作序列和视频）。
pub fn is_ws_gen_page(page: &str) -> bool {
    matches!(
        page,
        "scene"
            | "character"
            | "ui_concept"
            | "effect"
            | "animation_scene"
            | "animation_character"
            | "character_turnaround"
    )
}

/// 判断页面路由是否可恢复（非 workspace 页面，或已知路由）。
pub fn is_restorable_page(page: &str) -> bool {
    matches!(
        page,
        "welcome"
            | "settings"
            | "tasks"
            | "runtime_log"
            | "project"
            | "create_project"
            | "project_overview"
            | "project_script"
            | "project_characters"
            | "project_scenes"
            | "project_storyboard"
            | "project_video"
    ) || artait_model::feature_id_from_route(page).is_some()
}

/// 根据配置决定启动时的初始页面。
pub fn initial_page_from_config(cfg: &AppConfig) -> String {
    cfg.last_main_tab
        .as_deref()
        .filter(|page| is_restorable_page(page))
        .map(str::to_string)
        .or_else(|| {
            cfg.last_workspace
                .as_ref()
                .filter(|ws| is_workspace_page(&ws.page))
                .map(|ws| ws.page.clone())
        })
        .unwrap_or_else(|| "welcome".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_page_recognizes_all_modes() {
        assert!(is_workspace_page("scene"));
        assert!(is_workspace_page("character"));
        assert!(is_workspace_page("action_sequence"));
        assert!(!is_workspace_page("settings"));
        assert!(!is_workspace_page("welcome"));
    }

    #[test]
    fn ws_gen_page_excludes_action_sequence() {
        assert!(is_ws_gen_page("scene"));
        assert!(!is_ws_gen_page("action_sequence"));
    }

    #[test]
    fn restorable_includes_known_routes() {
        assert!(is_restorable_page("settings"));
        assert!(is_restorable_page("welcome"));
        assert!(is_restorable_page("scene"));
        assert!(!is_restorable_page("missing_page"));
    }

    #[test]
    fn initial_page_prefers_last_main_tab() {
        let mut cfg = AppConfig::default();
        cfg.last_main_tab = Some("settings".into());
        cfg.last_workspace = Some(artait_model::LastWorkspaceState {
            page: "scene".into(),
            prompt: String::new(),
            negative: String::new(),
            aspect: "1:1".into(),
            quality: "2K".into(),
            count: 1,
        });
        assert_eq!(initial_page_from_config(&cfg), "settings");
    }

    #[test]
    fn initial_page_falls_back_to_last_workspace() {
        let mut cfg = AppConfig::default();
        cfg.last_workspace = Some(artait_model::LastWorkspaceState {
            page: "character".into(),
            prompt: String::new(),
            negative: String::new(),
            aspect: "1:1".into(),
            quality: "2K".into(),
            count: 1,
        });
        assert_eq!(initial_page_from_config(&cfg), "character");
    }

    #[test]
    fn initial_page_defaults_to_welcome() {
        let cfg = AppConfig::default();
        assert_eq!(initial_page_from_config(&cfg), "welcome");
    }
}
