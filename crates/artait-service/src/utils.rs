//! 字符串与文件路径工具函数。

use std::path::Path;

/// 按字符截断，超出部分用 `…` 省略。
pub fn short(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        s.to_string()
    } else {
        let head: String = chars.iter().take(max).collect();
        format!("{head}…")
    }
}

/// 文件名安全的截断：剔除非 ASCII 字母数字，长度上限。
pub fn short_safe(s: &str, max: usize) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(max)
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

/// 根据文件扩展名推断 MIME 类型。
pub fn mime_for_path(path: &Path) -> String {
    match path
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase())
        .as_deref()
    {
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("gif") => "image/gif",
        Some("bmp") => "image/bmp",
        _ => "image/png",
    }
    .into()
}

/// 判断文件路径是否为支持的图片格式。
pub fn is_image_path(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|s| s.to_str())
            .map(|s| s.to_ascii_lowercase())
            .as_deref(),
        Some("png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_truncates_long_strings() {
        assert_eq!(short("hello world", 5), "hello…");
        assert_eq!(short("hi", 5), "hi");
    }

    #[test]
    fn short_safe_strips_unsafe_chars() {
        assert_eq!(short_safe("hello world!测试", 20), "helloworld");
        assert_eq!(short_safe("a-b_c", 10), "a-b_c");
        assert_eq!(short_safe("--trim--", 10), "trim");
    }

    #[test]
    fn mime_for_path_recognizes_extensions() {
        assert_eq!(mime_for_path(Path::new("a.png")), "image/png");
        assert_eq!(mime_for_path(Path::new("b.jpg")), "image/jpeg");
        assert_eq!(mime_for_path(Path::new("c.xyz")), "image/png");
    }

    #[test]
    fn is_image_path_matches_supported() {
        assert!(is_image_path(Path::new("img.png")));
        assert!(is_image_path(Path::new("img.JPG")));
        assert!(!is_image_path(Path::new("doc.txt")));
    }
}
