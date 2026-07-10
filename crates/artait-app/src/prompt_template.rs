//! 提示词模板（re-export from artait-service）+ UI 联动函数。
pub use artait_service::prompt_template::*;

use artait_model::AppConfig;
use slint::{Model, ModelRc, VecModel};

use crate::ui::{AppState, PromptTemplateGroup};

// ── UI 模型构建（依赖 Slint，保留在 app 层）──────────────────────────────

pub fn refresh_template_model(s: &AppState, cfg: &AppConfig, page: &str) {
    let groups = list_template_groups(cfg, page);
    let files: Vec<slint::SharedString> = groups
        .iter()
        .flat_map(|group| (0..group.files.row_count()).filter_map(|i| group.files.row_data(i)))
        .collect();
    let active = s.get_ws_template_active_category().to_string();
    let has_active = groups.iter().any(|group| group.name.as_str() == active);
    s.set_ws_template_files(ModelRc::new(VecModel::from(files)));
    s.set_ws_template_groups(ModelRc::new(VecModel::from(groups)));
    if !has_active {
        s.set_ws_template_active_category(default_template_category().into());
    }
}

pub fn list_template_groups(cfg: &AppConfig, page: &str) -> Vec<PromptTemplateGroup> {
    let root = prompt_template_dir(cfg, page);
    let Ok(entries) = std::fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut default_files = Vec::new();
    let mut groups: Vec<(String, Vec<String>)> = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let Some(category) = path
                .file_name()
                .and_then(|s| s.to_str())
                .map(|s| s.to_string())
            else {
                continue;
            };
            let mut files = template_files_in_dir(&path, Some(&category));
            if !files.is_empty() {
                files.sort_by_key(|name| name.to_ascii_lowercase());
                groups.push((category, files));
            }
        } else if is_template_file(&path) {
            if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
                default_files.push(name.to_string());
            }
        }
    }

    let mut out = Vec::new();
    default_files.sort_by_key(|name| name.to_ascii_lowercase());
    out.push(PromptTemplateGroup {
        name: default_template_category().into(),
        files: ModelRc::new(VecModel::from(
            default_files
                .into_iter()
                .map(|name| name.into())
                .collect::<Vec<slint::SharedString>>(),
        )),
    });

    groups.sort_by_key(|(name, _)| name.to_ascii_lowercase());
    out.extend(groups.into_iter().map(|(name, files)| {
        PromptTemplateGroup {
            name: name.into(),
            files: ModelRc::new(VecModel::from(
                files
                    .into_iter()
                    .map(|name| name.into())
                    .collect::<Vec<slint::SharedString>>(),
            )),
        }
    }));
    out
}

fn template_files_in_dir(dir: &std::path::Path, category: Option<&str>) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if !is_template_file(&path) {
                return None;
            }
            let file_name = path.file_name()?.to_str()?;
            Some(match category {
                Some(category) => format!("{category}/{file_name}"),
                None => file_name.to_string(),
            })
        })
        .collect()
}

fn is_template_file(path: &std::path::Path) -> bool {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase());
    matches!(ext.as_deref(), Some("txt" | "json"))
}
