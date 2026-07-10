//! 路径配置。
//!
//! 业务层禁止直接拼路径字符串，统一通过 PathConfig 取值。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

const PORTABLE_DATA_ENV: &str = "ARTAIT_DATA_DIR";
const DATA_DIR_NAME: &str = "data";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathConfig {
    pub input_dir: PathBuf,
    pub output_dir: PathBuf,
    pub prompt_dir: PathBuf,
    pub apply_prompt_dir: PathBuf,
    pub reference_action_dir: PathBuf,
    pub reference_prompt_dir: PathBuf,
}

impl Default for PathConfig {
    fn default() -> Self {
        let base = portable_data_dir();
        Self {
            input_dir: base.join("input"),
            output_dir: base.join("out"),
            prompt_dir: base.join("prompt"),
            apply_prompt_dir: base.join("apply_prompt"),
            reference_action_dir: base.join("reference_action"),
            reference_prompt_dir: base.join("reference_prompt"),
        }
    }
}

pub fn portable_data_dir() -> PathBuf {
    if let Some(raw) = std::env::var_os(PORTABLE_DATA_ENV) {
        return PathBuf::from(raw);
    }

    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|dir| dir.join(DATA_DIR_NAME)))
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .map(|dir| dir.join(DATA_DIR_NAME))
        })
        .unwrap_or_else(|| PathBuf::from(DATA_DIR_NAME))
}

impl PathConfig {
    pub fn output_subdir(&self, sub: &str) -> PathBuf {
        self.output_dir.join(sub)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    struct EnvGuard {
        previous: Option<std::ffi::OsString>,
    }

    impl EnvGuard {
        fn set(value: Option<&PathBuf>) -> Self {
            let previous = std::env::var_os(PORTABLE_DATA_ENV);
            match value {
                Some(path) => std::env::set_var(PORTABLE_DATA_ENV, path),
                None => std::env::remove_var(PORTABLE_DATA_ENV),
            }
            Self { previous }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.previous {
                Some(value) => std::env::set_var(PORTABLE_DATA_ENV, value),
                None => std::env::remove_var(PORTABLE_DATA_ENV),
            }
        }
    }

    #[test]
    fn portable_data_dir_prefers_env_override() {
        let _guard = env_lock().lock().unwrap();
        let dir = std::env::temp_dir().join("artait-portable-data-env");
        let _env = EnvGuard::set(Some(&dir));

        assert_eq!(portable_data_dir(), dir);
    }

    #[test]
    fn portable_data_dir_defaults_to_exe_sibling_data() {
        let _guard = env_lock().lock().unwrap();
        let _env = EnvGuard::set(None);

        assert_eq!(portable_data_dir().file_name().unwrap(), DATA_DIR_NAME);
    }

    #[test]
    fn path_config_defaults_live_under_data_dir() {
        let _guard = env_lock().lock().unwrap();
        let base = std::env::temp_dir().join("artait-path-config-data");
        let _env = EnvGuard::set(Some(&base));
        let cfg = PathConfig::default();

        assert_eq!(cfg.input_dir, base.join("input"));
        assert_eq!(cfg.output_dir, base.join("out"));
        assert_eq!(cfg.prompt_dir, base.join("prompt"));
        assert_eq!(cfg.apply_prompt_dir, base.join("apply_prompt"));
        assert_eq!(cfg.reference_action_dir, base.join("reference_action"));
        assert_eq!(cfg.reference_prompt_dir, base.join("reference_prompt"));
    }
}
