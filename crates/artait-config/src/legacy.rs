//! 旧 Python 版本 config.json 兼容解析。
//!
//! 只读取结构，不读取密钥。生成新的 AppConfig 草稿与一份迁移报告。

use std::path::{Path, PathBuf};

use artait_model::{
    AppConfig, FeatureConfig, FeaturePreset, ProviderFamily, ProviderInstance, ProviderScope,
    ThemeId,
};
use serde::Deserialize;

use crate::error::{ConfigError, ConfigResult};

#[derive(Debug, Clone, Default)]
pub struct MigrationReport {
    pub source_path: PathBuf,
    pub providers_found: usize,
    pub providers_imported: usize,
    pub paths_imported: bool,
    pub warnings: Vec<String>,
    pub secret_keys_seen: Vec<String>,
}

/// 反序列化旧 config.json 的一个最小子集。未知字段全部忽略。
#[derive(Debug, Default, Deserialize)]
struct LegacyRoot {
    #[serde(default)]
    input_dir: Option<String>,
    #[serde(default)]
    output_dir: Option<String>,
    #[serde(default)]
    prompt_dir: Option<String>,
    #[serde(default)]
    apply_prompt_dir: Option<String>,
    #[serde(default)]
    theme: Option<String>,
    #[serde(default)]
    general: LegacyGeneral,
    #[serde(default)]
    ui_font: Option<String>,
    #[serde(default)]
    provider_instances: Vec<LegacyProviderInstance>,
    #[serde(default)]
    providers: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Default, Deserialize)]
struct LegacyGeneral {
    #[serde(default)]
    theme: Option<String>,
    #[serde(default)]
    ui_font: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct LegacyProviderInstance {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    provider_id: Option<String>,
    #[serde(default)]
    scope: Option<Vec<String>>,
    #[serde(default)]
    endpoint: Option<String>,
    #[serde(default)]
    generation_model: Option<String>,
    #[serde(default)]
    analysis_model: Option<String>,
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default)]
    show_in_main_ui: Option<bool>,
}

/// 读取旧 config.json，返回 (新 AppConfig 草稿, 迁移报告)。
///
/// 不会写入任何凭据。`secret_keys_seen` 仅记录字段名，**不包含密钥值**。
pub fn migrate_from_legacy_json(
    legacy_path: impl AsRef<Path>,
) -> ConfigResult<(AppConfig, MigrationReport)> {
    let path = legacy_path.as_ref().to_path_buf();
    let raw = std::fs::read_to_string(&path).map_err(|e| ConfigError::Read {
        path: path.clone(),
        source: e,
    })?;

    let legacy: LegacyRoot = serde_json::from_str(&raw)
        .map_err(|e| ConfigError::Legacy(format!("旧 config.json 解析失败: {e}")))?;

    let mut report = MigrationReport {
        source_path: path.clone(),
        ..Default::default()
    };
    let mut cfg = AppConfig::default();
    cfg.migrated_from = Some(path.clone());

    if let Some(d) = legacy.input_dir {
        cfg.paths.input_dir = PathBuf::from(d);
        report.paths_imported = true;
    }
    if let Some(d) = legacy.output_dir {
        cfg.paths.output_dir = PathBuf::from(d);
        report.paths_imported = true;
    }
    if let Some(d) = legacy.prompt_dir {
        cfg.paths.prompt_dir = PathBuf::from(d);
        report.paths_imported = true;
    }
    if let Some(d) = legacy.apply_prompt_dir {
        cfg.paths.apply_prompt_dir = PathBuf::from(d);
        report.paths_imported = true;
    }

    if let Some(theme) = legacy.general.theme.or(legacy.theme) {
        cfg.ui.theme = legacy_theme_id(&theme);
    }
    if let Some(font) = legacy.general.ui_font.or(legacy.ui_font) {
        cfg.ui.font_family = font;
    }

    cfg.features = FeatureConfig {
        preset: FeaturePreset::Custom,
        enabled: artait_model::FeaturePreset::Full.enabled_features(),
        sidebar_hidden: Vec::new(),
    };

    report.providers_found = legacy.provider_instances.len();
    for inst in legacy.provider_instances {
        let id = match inst.id {
            Some(s) if !s.is_empty() => s,
            _ => {
                report.warnings.push("跳过没有 id 的 provider 实例".into());
                continue;
            }
        };

        let provider_id = inst.provider_id.unwrap_or_else(|| "unknown".into());
        let family = guess_family(&provider_id);
        let scopes = inst
            .scope
            .unwrap_or_default()
            .into_iter()
            .filter_map(|s| match s.as_str() {
                "generation" | "image" | "generate" => Some(ProviderScope::Generation),
                "analysis" | "analyze" | "inference" => Some(ProviderScope::Analysis),
                "video" | "generate_video" => Some(ProviderScope::Video),
                _ => None,
            })
            .collect::<Vec<_>>();

        let mut models = artait_model::ProviderModelConfig::default();
        models.generation_model = inst.generation_model;
        models.analysis_model = inst.analysis_model;

        if inst.api_key.is_some() {
            report
                .secret_keys_seen
                .push(crate::secret_store::ref_key(&id, "api_key"));
        }

        let new_inst = ProviderInstance {
            id: id.clone(),
            name: inst.name.unwrap_or_else(|| id.clone()),
            provider_id,
            family,
            scopes,
            show_in_main_ui: inst.show_in_main_ui.unwrap_or(true),
            models,
            endpoint: inst.endpoint,
            secret_ref: if report.secret_keys_seen.last().is_some() {
                Some(crate::secret_store::ref_key(&id, "api_key"))
            } else {
                None
            },
            api_key: inst.api_key,
            extra: serde_json::Value::Object(serde_json::Map::new()),
        };

        cfg.providers.push(new_inst);
        report.providers_imported += 1;
    }

    if !legacy.providers.is_empty() {
        report.warnings.push(format!(
            "旧 providers 表里还有 {} 个未导入的协议族条目（无 id），需手动处理",
            legacy.providers.len()
        ));
    }

    Ok((cfg, report))
}

fn legacy_theme_id(theme: &str) -> ThemeId {
    match theme.to_lowercase().as_str() {
        "dark" | "indigo" => ThemeId::Dark,
        "light" => ThemeId::Light,
        "ocean" => ThemeId::Ocean,
        "warm" => ThemeId::Warm,
        "forest" => ThemeId::Forest,
        "rose" => ThemeId::Rose,
        "cyber" => ThemeId::Cyber,
        "oled" => ThemeId::Oled,
        "cream" => ThemeId::Cream,
        "system" | "auto" => ThemeId::System,
        "user" => ThemeId::User,
        other if other.contains("light") => ThemeId::Light,
        other if other.contains("system") || other.contains("auto") => ThemeId::System,
        _ => ThemeId::Dark,
    }
}

fn guess_family(provider_id: &str) -> ProviderFamily {
    let s = provider_id.to_lowercase();
    if s.contains("openai") {
        ProviderFamily::OpenAiCompatible
    } else if s.contains("gemini") {
        ProviderFamily::GeminiCompatible
    } else if s.contains("wavespeed") {
        ProviderFamily::WavespeedCompatible
    } else if s.contains("seedance") || s.contains("volcengine") {
        ProviderFamily::VolcengineSeedance
    } else if s.contains("deepseek") {
        ProviderFamily::DeepSeek
    } else if s.contains("ikun") {
        ProviderFamily::Ikuncode
    } else if s.contains("rembg") {
        ProviderFamily::Rembg
    } else if s.contains("photoroom") {
        ProviderFamily::PhotoRoom
    } else {
        ProviderFamily::Custom
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_basic_legacy() {
        let raw = r#"
        {
            "input_dir": "C:/old/input",
            "output_dir": "C:/old/out",
            "theme": "light",
            "provider_instances": [
                {
                    "id": "openai-1",
                    "name": "OpenAI",
                    "provider_id": "openai",
                    "scope": ["generation", "analysis"],
                    "endpoint": "https://api.openai.com/v1",
                    "api_key": "sk-REDACTED",
                    "generation_model": "gpt-image-1"
                }
            ]
        }
        "#;
        let dir = std::env::temp_dir().join("artait-config-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("legacy.json");
        std::fs::write(&path, raw).unwrap();

        let (cfg, report) = migrate_from_legacy_json(&path).unwrap();
        assert_eq!(report.providers_imported, 1);
        assert_eq!(cfg.providers[0].family, ProviderFamily::OpenAiCompatible);
        assert_eq!(cfg.providers[0].scopes.len(), 2);
        assert_eq!(cfg.ui.theme, artait_model::ThemeId::Light);
        assert_eq!(report.secret_keys_seen, vec!["openai-1/api_key"]);
        // 关键：迁移报告不包含密钥值
        let report_dbg = format!("{report:?}");
        assert!(!report_dbg.contains("sk-REDACTED"));
    }

    #[test]
    fn skips_legacy_without_id() {
        let raw = r#"{ "provider_instances": [{ "name": "no id" }] }"#;
        let dir = std::env::temp_dir().join("artait-config-test-skip");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("legacy.json");
        std::fs::write(&path, raw).unwrap();

        let (cfg, report) = migrate_from_legacy_json(&path).unwrap();
        assert_eq!(report.providers_imported, 0);
        assert_eq!(cfg.providers.len(), 0);
        assert!(!report.warnings.is_empty());
    }
}
