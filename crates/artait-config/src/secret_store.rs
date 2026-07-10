//! Windows Credential Manager 封装 + 密钥脱敏工具。

use crate::error::{ConfigError, ConfigResult};

const SERVICE: &str = "ArtAIT";

fn entry(key: &str) -> keyring::Result<keyring::Entry> {
    keyring::Entry::new_with_target(&target_name(key), SERVICE, key)
}

fn legacy_entry(key: &str) -> keyring::Result<keyring::Entry> {
    keyring::Entry::new(SERVICE, key)
}

fn target_name(key: &str) -> String {
    format!("{SERVICE}:{}", key.replace('/', ":"))
}

/// 写入凭据。
pub fn put(key: &str, secret: &str) -> ConfigResult<()> {
    let entry = entry(key)?;
    entry.set_password(secret)?;
    Ok(())
}

/// 读取凭据。不存在返回 None。
pub fn get(key: &str) -> ConfigResult<Option<String>> {
    let entry = entry(key)?;
    match entry.get_password() {
        Ok(s) => Ok(Some(s)),
        Err(keyring::Error::NoEntry) => {
            let legacy_entry = legacy_entry(key)?;
            match legacy_entry.get_password() {
                Ok(s) => Ok(Some(s)),
                Err(keyring::Error::NoEntry) => Ok(None),
                Err(e) => Err(ConfigError::Keyring(e)),
            }
        }
        Err(e) => Err(ConfigError::Keyring(e)),
    }
}

/// 删除凭据。不存在视为成功。
pub fn delete(key: &str) -> ConfigResult<()> {
    let entry = entry(key)?;
    match entry.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(ConfigError::Keyring(e)),
    }?;
    let legacy_entry = legacy_entry(key)?;
    match legacy_entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(ConfigError::Keyring(e)),
    }
}

/// 生成凭据键名约定：`<instance_id>/<field>`。
pub fn ref_key(instance_id: &str, field: &str) -> String {
    format!("{instance_id}/{field}")
}

/// 给密钥做日志脱敏：`sk-1234567890abcdef` → `sk-1***cdef`。
///
/// 规则：保留首尾各 2 字符 + `-` 前缀（如有），中间用 `***`。
/// 长度 ≤ 8 直接 mask 全部。
pub fn mask(secret: &str) -> String {
    if secret.is_empty() {
        return String::new();
    }
    if secret.len() <= 8 {
        return "***".into();
    }

    let (prefix, body) = if let Some(idx) = secret.find('-') {
        if idx <= 4 {
            (&secret[..=idx], &secret[idx + 1..])
        } else {
            ("", secret)
        }
    } else {
        ("", secret)
    };

    let body_chars: Vec<char> = body.chars().collect();
    if body_chars.len() <= 4 {
        return format!("{prefix}***");
    }
    let head: String = body_chars.iter().take(2).collect();
    let tail: String = body_chars.iter().rev().take(4).collect::<String>();
    let tail: String = tail.chars().rev().collect();
    format!("{prefix}{head}***{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ref_key_concatenates() {
        assert_eq!(ref_key("openai-1", "api_key"), "openai-1/api_key");
    }

    #[test]
    fn target_name_replaces_slashes() {
        assert_eq!(target_name("openai-1/api_key"), "ArtAIT:openai-1:api_key");
    }

    #[test]
    fn mask_short_strings_fully() {
        assert_eq!(mask(""), "");
        assert_eq!(mask("abc"), "***");
        assert_eq!(mask("12345678"), "***");
    }

    #[test]
    fn mask_keeps_prefix_and_tail() {
        // 含短前缀（"-" 位置 ≤ 4），保留前缀
        assert_eq!(mask("sk-1234567890abcdef"), "sk-12***cdef");
        // "-" 位置 > 4 时，按整体处理
        assert_eq!(mask("Bearer-abcdefghijkl"), "Be***ijkl");
        // 无前缀
        assert_eq!(mask("abcdefghijkl"), "ab***ijkl");
    }

    #[test]
    fn mask_does_not_panic_on_unicode() {
        let s = "中文密钥abcdefgh";
        let m = mask(s);
        assert!(m.contains("***"));
    }
}
