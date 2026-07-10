//! 基础设置服务：配置验证、保存。

use artait_model::{AppConfig, PathConfig, ThemeId};

/// 一次基础设置的变更。
pub struct BasicSettingsChange {
    pub theme: ThemeId,
    pub font_family: String,
    pub font_size: u32,
    pub input_dir: std::path::PathBuf,
    pub output_dir: std::path::PathBuf,
    pub prompt_dir: std::path::PathBuf,
    pub upload_api_url: Option<String>,
    pub upload_api_key: Option<String>,
}

/// 验证并应用基础设置到配置。返回验证错误或变更摘要。
pub fn apply_basic_settings(
    cfg: &mut AppConfig,
    change: BasicSettingsChange,
) -> Result<SettingsSaveOutcome, String> {
    if change.font_family.trim().is_empty() {
        return Err("字体名称不能为空".into());
    }

    let old_output = cfg.paths.output_dir.clone();

    let paths = PathConfig {
        input_dir: change.input_dir,
        output_dir: change.output_dir,
        prompt_dir: change.prompt_dir,
        ..cfg.paths.clone()
    };

    cfg.ui.theme = change.theme;
    cfg.ui.font_family = change.font_family;
    cfg.ui.font_size = change.font_size;
    cfg.paths = paths;
    cfg.image_upload.api_url = change.upload_api_url.filter(|s| !s.trim().is_empty());
    cfg.image_upload.api_key = change.upload_api_key.filter(|s| !s.trim().is_empty());

    Ok(SettingsSaveOutcome {
        output_dir_changed: cfg.paths.output_dir != old_output,
    })
}

/// 保存设置后的操作决策（返回给 UI 层执行）。
pub struct SettingsSaveOutcome {
    /// 输出目录是否变更（需要重启资产监听）
    pub output_dir_changed: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_change() -> BasicSettingsChange {
        BasicSettingsChange {
            theme: ThemeId::Dark,
            font_family: "Test Font".into(),
            font_size: 14,
            input_dir: std::path::PathBuf::from("D:/test/input"),
            output_dir: std::path::PathBuf::from("D:/test/output"),
            prompt_dir: std::path::PathBuf::from("D:/test/prompt"),
            upload_api_url: None,
            upload_api_key: None,
        }
    }

    #[test]
    fn rejects_empty_font() {
        let mut cfg = AppConfig::default();
        let mut change = default_change();
        change.font_family = "  ".into();
        assert!(apply_basic_settings(&mut cfg, change).is_err());
    }

    #[test]
    fn applies_all_fields() {
        let mut cfg = AppConfig::default();
        let change = default_change();
        let _outcome = apply_basic_settings(&mut cfg, change).unwrap();
        // output_dir changed from default → checks correctly
        assert_eq!(cfg.ui.theme, ThemeId::Dark);
        assert_eq!(cfg.ui.font_family, "Test Font");
        assert_eq!(cfg.ui.font_size, 14);
        assert_eq!(
            cfg.paths.output_dir,
            std::path::PathBuf::from("D:/test/output")
        );
    }

    #[test]
    fn strips_empty_upload_url() {
        let mut cfg = AppConfig::default();
        let mut change = default_change();
        change.upload_api_url = Some("  ".into());
        apply_basic_settings(&mut cfg, change).unwrap();
        assert!(cfg.image_upload.api_url.is_none());
    }

    #[test]
    fn keeps_valid_upload_url() {
        let mut cfg = AppConfig::default();
        let mut change = default_change();
        change.upload_api_url = Some("https://api.example.com".into());
        apply_basic_settings(&mut cfg, change).unwrap();
        assert_eq!(
            cfg.image_upload.api_url,
            Some("https://api.example.com".into())
        );
    }

    #[test]
    fn config_persistence_roundtrip() {
        let tmp = std::env::temp_dir()
            .join("artait-settings-roundtrip")
            .join(format!("{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let path = tmp.join("app_config.toml");

        let mut cfg = AppConfig::default();
        let change = BasicSettingsChange {
            theme: ThemeId::Light,
            font_family: "PingFang SC".into(),
            font_size: 16,
            input_dir: tmp.join("input"),
            output_dir: tmp.join("out"),
            prompt_dir: tmp.join("prompt"),
            upload_api_url: Some("https://upload.example.com".into()),
            upload_api_key: Some("secret-123".into()),
        };
        let _outcome = apply_basic_settings(&mut cfg, change).unwrap();

        // save
        artait_config::save_to(&path, &cfg).unwrap();
        assert!(path.exists());

        // load back
        let loaded: artait_model::AppConfig =
            toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();

        assert_eq!(loaded.ui.font_family, "PingFang SC");
        assert_eq!(loaded.ui.font_size, 16);
        assert_eq!(loaded.paths.output_dir, tmp.join("out"));
        assert_eq!(
            loaded.image_upload.api_url,
            Some("https://upload.example.com".into())
        );
        assert_eq!(loaded.image_upload.api_key, Some("secret-123".into()));

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
