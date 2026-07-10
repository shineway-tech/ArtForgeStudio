//! 配置层错误类型。

use std::path::PathBuf;

#[derive(thiserror::Error, Debug)]
pub enum ConfigError {
    #[error("无法定位用户配置目录")]
    NoConfigDir,

    #[error("读取 {path} 失败: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("写入 {path} 失败: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("解析 {path} 失败: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    #[error("序列化失败: {0}")]
    Serialize(#[from] toml::ser::Error),

    #[error("创建目录 {path} 失败: {source}")]
    CreateDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("凭据存储错误: {0}")]
    Keyring(#[from] keyring::Error),

    #[error("旧配置迁移错误: {0}")]
    Legacy(String),
}

pub type ConfigResult<T> = std::result::Result<T, ConfigError>;
