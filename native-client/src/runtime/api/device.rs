use super::ApiError;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct DeviceIdentity {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) platform: String,
}

impl DeviceIdentity {
    pub(crate) fn load_or_create(data_dir: &Path) -> Result<Self, ApiError> {
        let path = device_path(data_dir);
        if path.exists() {
            let text = fs::read_to_string(&path).map_err(|error| ApiError::LocalState {
                message: format!("无法读取设备标识：{error}"),
            })?;
            let identity = serde_json::from_str::<Self>(&text).map_err(|error| {
                ApiError::LocalState {
                    message: format!("设备标识文件已损坏：{error}"),
                }
            })?;
            identity.validate()?;
            return Ok(identity);
        }

        fs::create_dir_all(data_dir).map_err(|error| ApiError::LocalState {
            message: format!("无法创建设备数据目录：{error}"),
        })?;
        let identity = Self {
            id: Uuid::new_v4().to_string(),
            name: device_name(),
            platform: platform_name().to_string(),
        };
        identity.write_atomic(&path)?;
        Ok(identity)
    }

    fn validate(&self) -> Result<(), ApiError> {
        if Uuid::parse_str(&self.id).is_err() || self.name.trim().is_empty() {
            return Err(ApiError::LocalState {
                message: "设备标识内容无效".to_string(),
            });
        }
        Ok(())
    }

    fn write_atomic(&self, path: &Path) -> Result<(), ApiError> {
        let temporary = path.with_extension("json.tmp");
        let payload = serde_json::to_vec_pretty(self).map_err(|error| ApiError::LocalState {
            message: format!("无法序列化设备标识：{error}"),
        })?;
        fs::write(&temporary, payload).map_err(|error| ApiError::LocalState {
            message: format!("无法写入设备标识：{error}"),
        })?;
        fs::rename(&temporary, path).map_err(|error| ApiError::LocalState {
            message: format!("无法保存设备标识：{error}"),
        })
    }
}

fn device_path(data_dir: &Path) -> PathBuf {
    data_dir.join("device.json")
}

fn device_name() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "ArtForge Studio".to_string())
        .chars()
        .take(128)
        .collect()
}

fn platform_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "windows"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_identity_is_stable_after_first_write() {
        let dir = std::env::temp_dir().join(format!("artforge-device-test-{}", Uuid::new_v4()));
        let first = DeviceIdentity::load_or_create(&dir).unwrap();
        let second = DeviceIdentity::load_or_create(&dir).unwrap();

        assert_eq!(first.id, second.id);
        assert_eq!(first.platform, second.platform);
        let _ = fs::remove_dir_all(dir);
    }
}
