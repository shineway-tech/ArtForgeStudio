//! 缩略图缓存：按需生成 256px 缩略图到绿色版 `data/cache/thumbnails/`。
//!
//! - 缓存键 = 源文件路径的 SHA-256 前 16 位 hex
//! - 目标尺寸：最长边 256px，保持比例
//! - 输出格式：PNG

use std::path::{Path, PathBuf};

/// 缩略图目标最长边（像素）。
const THUMB_MAX: u32 = 256;

/// 获取源文件对应的缩略图缓存路径。不保证文件存在。
fn cache_path(src: &Path) -> Option<PathBuf> {
    let dir = cache_dir()?;
    let hash = path_hash(src);
    Some(dir.join(format!("{hash}.png")))
}

/// 确保缩略图存在：已缓存则直接返回路径，否则生成并缓存。
/// 失败时返回原始路径作为 fallback。
pub fn ensure(src: &Path) -> PathBuf {
    let Some(thumb) = cache_path(src) else {
        return src.to_path_buf();
    };
    if thumb.exists() {
        return thumb;
    }
    match generate(src, &thumb) {
        Ok(()) => thumb,
        Err(e) => {
            tracing::debug!(error = %e, src = %src.display(), "缩略图生成失败，用原图");
            src.to_path_buf()
        }
    }
}

fn cache_dir() -> Option<PathBuf> {
    Some(
        artait_model::portable_data_dir()
            .join("cache")
            .join("thumbnails"),
    )
}

/// 文件路径 → 短哈希（用 std hash）。
fn path_hash(path: &Path) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.to_string_lossy().hash(&mut h);
    format!("{:016x}", h.finish())
}

/// 生成缩略图：读取 → 缩放 → 写 PNG。
fn generate(src: &Path, dest: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let img = image::open(src)?;
    let (w, h) = (img.width(), img.height());
    let (nw, nh) = if w > h {
        (THUMB_MAX, (h as f32 * THUMB_MAX as f32 / w as f32) as u32)
    } else {
        ((w as f32 * THUMB_MAX as f32 / h as f32) as u32, THUMB_MAX)
    };
    let thumb = img.resize(nw, nh, image::imageops::FilterType::Lanczos3);

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    thumb.save(dest)?;
    tracing::debug!(src = %src.display(), dest = %dest.display(), "缩略图已生成");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_path_is_deterministic() {
        let p = Path::new("D:/test/foo.png");
        let a = cache_path(p);
        let b = cache_path(p);
        assert_eq!(a, b);
    }

    #[test]
    fn ensure_falls_back_to_original_on_bad_input() {
        let p = Path::new("nonexistent_file_12345.xyz");
        let result = ensure(p);
        assert_eq!(result, p.to_path_buf());
    }

    #[test]
    fn ensure_generates_thumbnail() {
        let dir = std::env::temp_dir().join("artait-thumb-test");
        std::fs::create_dir_all(&dir).unwrap();
        // 创建一个简单的测试图片
        let src = dir.join("test.png");
        let mut img: image::RgbaImage = image::ImageBuffer::new(512, 256);
        for (_, _, p) in img.enumerate_pixels_mut() {
            *p = image::Rgba([255u8, 0, 0, 255]);
        }
        img.save(&src).unwrap();

        let thumb = ensure(&src);
        assert!(thumb.exists());
        assert_ne!(thumb, src);
        // 验证缩略图尺寸
        let thumb_img = image::open(&thumb).unwrap();
        assert_eq!(thumb_img.width(), THUMB_MAX);
    }
}
