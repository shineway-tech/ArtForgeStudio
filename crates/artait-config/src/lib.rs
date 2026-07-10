//! ArtAIT 配置加载、TOML 读写、密钥存储。

use std::path::{Path, PathBuf};

use artait_model::{AppConfig, PathConfig};
use chrono::Utc;
use tracing::warn;

pub mod error;
pub mod legacy;
pub mod secret_store;

pub use error::{ConfigError, ConfigResult};
pub use legacy::{migrate_from_legacy_json, MigrationReport};

/// 加载结果。区分"新建默认"和"从损坏配置恢复"。
#[derive(Debug)]
pub enum LoadOutcome {
    /// 配置文件存在且解析成功。
    Loaded(AppConfig),
    /// 配置文件不存在；返回默认配置。调用方决定是否走首启引导。
    Missing(AppConfig),
    /// 配置文件存在但解析失败。已备份原文件到 `backup`，返回默认配置。
    Recovered { config: AppConfig, backup: PathBuf },
}

impl LoadOutcome {
    pub fn into_config(self) -> AppConfig {
        match self {
            LoadOutcome::Loaded(c) | LoadOutcome::Missing(c) => c,
            LoadOutcome::Recovered { config, .. } => config,
        }
    }
}

/// 返回绿色版 `data/config/` 目录。不存在时不创建。
pub fn config_dir() -> ConfigResult<PathBuf> {
    Ok(artait_model::portable_data_dir().join("config"))
}

/// 返回 `app_config.toml` 路径。
pub fn app_config_path() -> ConfigResult<PathBuf> {
    Ok(config_dir()?.join("app_config.toml"))
}

/// 兼容旧版 `load`：返回 `Option<AppConfig>` 简易语义。
///
/// 解析失败时落入恢复模式（备份 + 默认值），不向调用方报错。
pub fn load() -> ConfigResult<Option<AppConfig>> {
    match load_with_outcome()? {
        LoadOutcome::Missing(_) => Ok(None),
        outcome => Ok(Some(outcome.into_config())),
    }
}

/// 详细加载流程：可区分缺失 / 已加载 / 已恢复。
pub fn load_with_outcome() -> ConfigResult<LoadOutcome> {
    let path = app_config_path()?;
    let legacy_path = legacy_appdata_config_path();
    load_with_paths(&path, legacy_path.as_deref())
}

fn load_with_paths(path: &Path, legacy_path: Option<&Path>) -> ConfigResult<LoadOutcome> {
    if !path.exists() {
        if let Some(legacy_path) = legacy_path.filter(|p| p.exists()) {
            return migrate_legacy_app_config(path, legacy_path);
        }
        return Ok(LoadOutcome::Missing(AppConfig::default()));
    }

    load_existing_config(path)
}

fn load_existing_config(path: &Path) -> ConfigResult<LoadOutcome> {
    let raw = std::fs::read_to_string(&path).map_err(|e| ConfigError::Read {
        path: path.to_path_buf(),
        source: e,
    })?;

    match toml::from_str::<AppConfig>(&raw) {
        Ok(cfg) => Ok(LoadOutcome::Loaded(cfg)),
        Err(parse_err) => {
            warn!(error = %parse_err, "app_config.toml 解析失败，进入恢复模式");
            let backup = backup_corrupt(path, &raw)?;
            Ok(LoadOutcome::Recovered {
                config: AppConfig::default(),
                backup,
            })
        }
    }
}

fn migrate_legacy_app_config(path: &Path, legacy_path: &Path) -> ConfigResult<LoadOutcome> {
    let raw = std::fs::read_to_string(legacy_path).map_err(|e| ConfigError::Read {
        path: legacy_path.to_path_buf(),
        source: e,
    })?;
    let mut cfg = toml::from_str::<AppConfig>(&raw).map_err(|e| ConfigError::Parse {
        path: legacy_path.to_path_buf(),
        source: e,
    })?;
    rewrite_legacy_default_paths(&mut cfg);
    save_to(path, &cfg)?;
    Ok(LoadOutcome::Loaded(cfg))
}

fn legacy_appdata_config_path() -> Option<PathBuf> {
    let proj = directories::ProjectDirs::from("", "", "ArtAIT")?;
    Some(proj.config_dir().join("app_config.toml"))
}

fn rewrite_legacy_default_paths(cfg: &mut AppConfig) {
    let Some(old_base) = legacy_documents_base_dir() else {
        return;
    };
    let new_defaults = PathConfig::default();
    rewrite_if_legacy(
        &mut cfg.paths.input_dir,
        &old_base.join("input"),
        &new_defaults.input_dir,
    );
    rewrite_if_legacy(
        &mut cfg.paths.output_dir,
        &old_base.join("out"),
        &new_defaults.output_dir,
    );
    rewrite_if_legacy(
        &mut cfg.paths.prompt_dir,
        &old_base.join("prompt"),
        &new_defaults.prompt_dir,
    );
    rewrite_if_legacy(
        &mut cfg.paths.apply_prompt_dir,
        &old_base.join("apply_prompt"),
        &new_defaults.apply_prompt_dir,
    );
    rewrite_if_legacy(
        &mut cfg.paths.reference_action_dir,
        &old_base.join("reference_action"),
        &new_defaults.reference_action_dir,
    );
    rewrite_if_legacy(
        &mut cfg.paths.reference_prompt_dir,
        &old_base.join("reference_prompt"),
        &new_defaults.reference_prompt_dir,
    );
}

fn legacy_documents_base_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .map(|home| home.join("Documents").join("ArtAIT"))
}

fn rewrite_if_legacy(current: &mut PathBuf, legacy: &Path, replacement: &Path) {
    if path_eq(current, legacy) {
        *current = replacement.to_path_buf();
    }
}

fn path_eq(left: &Path, right: &Path) -> bool {
    if cfg!(windows) {
        left.to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy())
    } else {
        left == right
    }
}

/// 写入 `app_config.toml`。先写到 `.tmp` 再 rename，避免半写损坏。
pub fn save(cfg: &AppConfig) -> ConfigResult<()> {
    let path = app_config_path()?;
    save_to(&path, cfg)
}

pub fn save_to(path: &Path, cfg: &AppConfig) -> ConfigResult<()> {
    if let Some(parent) = path.parent() {
        ensure_dir(parent)?;
    }
    let s = toml::to_string_pretty(cfg)?;

    // 原子写入：先写临时文件，再 rename。Windows 上 rename 同盘是原子的。
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, s).map_err(|e| ConfigError::Write {
        path: tmp.clone(),
        source: e,
    })?;
    std::fs::rename(&tmp, path).map_err(|e| ConfigError::Write {
        path: path.to_path_buf(),
        source: e,
    })?;
    Ok(())
}

/// 把损坏的配置文件备份到 `app_config.toml.broken-<时间戳>`。
fn backup_corrupt(path: &Path, raw: &str) -> ConfigResult<PathBuf> {
    let stamp = Utc::now().format("%Y%m%d-%H%M%S");
    let mut backup = path.to_path_buf();
    let name = format!(
        "{}.broken-{stamp}",
        path.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("config")
    );
    backup.set_file_name(name);
    std::fs::write(&backup, raw).map_err(|e| ConfigError::Write {
        path: backup.clone(),
        source: e,
    })?;
    Ok(backup)
}

/// 确保所有 PathConfig 中声明的目录存在。
pub fn ensure_dirs(cfg: &AppConfig) -> ConfigResult<()> {
    let p = &cfg.paths;
    for d in [
        &p.input_dir,
        &p.output_dir,
        &p.prompt_dir,
        &p.apply_prompt_dir,
        &p.reference_action_dir,
        &p.reference_prompt_dir,
    ] {
        ensure_dir(d)?;
    }
    Ok(())
}

fn ensure_dir(path: &Path) -> ConfigResult<()> {
    std::fs::create_dir_all(path).map_err(|e| ConfigError::CreateDir {
        path: path.to_path_buf(),
        source: e,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::sync::{Mutex, OnceLock};

    const DATA_ENV: &str = "ARTAIT_DATA_DIR";
    const USER_ENV: &str = "USERPROFILE";

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("artait-config-test-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("app_config.toml")
    }

    struct EnvGuard {
        key: &'static str,
        previous: Option<OsString>,
    }

    impl EnvGuard {
        fn set_path(key: &'static str, value: Option<&Path>) -> Self {
            let previous = std::env::var_os(key);
            match value {
                Some(path) => std::env::set_var(key, path),
                None => std::env::remove_var(key),
            }
            Self { key, previous }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.previous {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }

    #[test]
    fn save_round_trip() {
        let path = temp_path("rt");
        let cfg = AppConfig::default();
        save_to(&path, &cfg).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        let back: AppConfig = toml::from_str(&raw).unwrap();
        assert_eq!(back.schema_version, cfg.schema_version);
    }

    #[test]
    fn ensure_dirs_creates_paths() {
        let dir = std::env::temp_dir().join("artait-config-ensure-dirs");
        let _ = std::fs::remove_dir_all(&dir);

        let mut cfg = AppConfig::default();
        cfg.paths.input_dir = dir.join("input");
        cfg.paths.output_dir = dir.join("out");
        cfg.paths.prompt_dir = dir.join("prompt");
        cfg.paths.apply_prompt_dir = dir.join("apply_prompt");
        cfg.paths.reference_action_dir = dir.join("ref_action");
        cfg.paths.reference_prompt_dir = dir.join("ref_prompt");

        ensure_dirs(&cfg).unwrap();
        assert!(cfg.paths.input_dir.exists());
        assert!(cfg.paths.output_dir.exists());
    }

    #[test]
    fn config_dir_uses_portable_data_dir() {
        let _guard = env_lock().lock().unwrap();
        let root = std::env::temp_dir().join("artait-config-portable");
        let _env = EnvGuard::set_path(DATA_ENV, Some(&root));

        assert_eq!(config_dir().unwrap(), root.join("config"));
        assert_eq!(
            app_config_path().unwrap(),
            root.join("config").join("app_config.toml")
        );
    }

    #[test]
    fn missing_config_returns_default_without_writing() {
        let _guard = env_lock().lock().unwrap();
        let root = std::env::temp_dir().join("artait-config-missing");
        let _ = std::fs::remove_dir_all(&root);
        let _env = EnvGuard::set_path(DATA_ENV, Some(&root));
        let path = app_config_path().unwrap();

        match load_with_paths(&path, None).unwrap() {
            LoadOutcome::Missing(_) => {}
            _ => panic!("expected missing config"),
        }
        assert!(!path.exists());
    }

    #[test]
    fn legacy_default_paths_are_rewritten_to_data() {
        let _guard = env_lock().lock().unwrap();
        let user = std::env::temp_dir().join("artait-legacy-user");
        let data = std::env::temp_dir().join("artait-legacy-data");
        let _user_env = EnvGuard::set_path(USER_ENV, Some(&user));
        let _data_env = EnvGuard::set_path(DATA_ENV, Some(&data));

        let mut cfg = AppConfig::default();
        let old_base = user.join("Documents").join("ArtAIT");
        cfg.paths.input_dir = old_base.join("input");
        cfg.paths.output_dir = old_base.join("out");
        cfg.paths.prompt_dir = old_base.join("prompt");
        rewrite_legacy_default_paths(&mut cfg);

        assert_eq!(cfg.paths.input_dir, data.join("input"));
        assert_eq!(cfg.paths.output_dir, data.join("out"));
        assert_eq!(cfg.paths.prompt_dir, data.join("prompt"));
    }

    #[test]
    fn custom_paths_are_preserved_during_rewrite() {
        let _guard = env_lock().lock().unwrap();
        let user = std::env::temp_dir().join("artait-legacy-user-custom");
        let custom = std::env::temp_dir().join("artait-custom-output");
        let _user_env = EnvGuard::set_path(USER_ENV, Some(&user));

        let mut cfg = AppConfig::default();
        cfg.paths.output_dir = custom.clone();
        rewrite_legacy_default_paths(&mut cfg);

        assert_eq!(cfg.paths.output_dir, custom);
    }
}
