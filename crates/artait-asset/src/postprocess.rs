//! 图片后处理：去黑（unmult）。
//!
//! Unmult 把黑底特效图转成透明背景：
//! - alpha = max(R, G, B)；
//! - 输出 RGB 保留原值（与 RGB / alpha 等价的视觉效果，
//!   因为 alpha 通道本身已经吸收了亮度）；
//! - 完全黑像素 (RGB=0) → 完全透明。
//!
//! 输入支持 PNG/JPG/WEBP，输出固定 PNG（因为需要 alpha）。

use std::path::{Path, PathBuf};

use image::{DynamicImage, ImageBuffer, Rgba};

#[derive(Debug, thiserror::Error)]
pub enum PostprocessError {
    #[error("无法读取图片: {0}")]
    Read(String),
    #[error("无法写入图片: {0}")]
    Write(String),
    #[error("解码失败: {0}")]
    Decode(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

pub type PostprocessResult<T> = std::result::Result<T, PostprocessError>;

/// 去黑后保存到 `<原文件名>.unmult.png`，返回新路径。
pub fn unmult_to_sibling(src: &Path) -> PostprocessResult<PathBuf> {
    let img = image::open(src).map_err(|e| PostprocessError::Decode(e.to_string()))?;
    let processed = unmult(&img.to_rgba8());

    let dest = sibling_with_suffix(src, ".unmult.png");
    processed
        .save(&dest)
        .map_err(|e| PostprocessError::Write(e.to_string()))?;
    Ok(dest)
}

/// 像素级 unmult。
pub fn unmult(rgba: &ImageBuffer<Rgba<u8>, Vec<u8>>) -> ImageBuffer<Rgba<u8>, Vec<u8>> {
    let (w, h) = rgba.dimensions();
    let mut out = ImageBuffer::new(w, h);
    for (x, y, p) in rgba.enumerate_pixels() {
        let r = p.0[0];
        let g = p.0[1];
        let b = p.0[2];
        // alpha = max(R, G, B)
        let a = r.max(g).max(b);
        // RGB 保持，alpha 用亮度
        out.put_pixel(x, y, Rgba([r, g, b, a]));
    }
    out
}

fn sibling_with_suffix(src: &Path, suffix: &str) -> PathBuf {
    let stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or("image");
    let parent = src.parent().unwrap_or_else(|| Path::new("."));
    let mut dest = parent.join(format!("{stem}{suffix}"));
    let mut n = 1;
    while dest.exists() {
        dest = parent.join(format!("{stem}{suffix}-{n}"));
        n += 1;
    }
    dest
}

/// 通过 HTTP 服务去背景，返回去背景后的 PNG 文件路径。
///
/// 支持两种服务：
/// - Rembg 自托管：`POST <endpoint>/api/remove`，multipart `file` 字段
/// - PhotoRoom：`POST https://sdk.photoroom.com/v1/segment`，multipart `image_file` 字段 + `x-api-key` header
pub async fn remove_background_http(
    src: &Path,
    service: RemoveBackgroundService<'_>,
) -> PostprocessResult<PathBuf> {
    let bytes = std::fs::read(src)?;
    let filename = src
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("image.png")
        .to_string();

    let result_bytes = remove_background_bytes(bytes, filename, service).await?;

    let dest = sibling_with_suffix(src, ".rembg.png");
    std::fs::write(&dest, &result_bytes)?;
    Ok(dest)
}

/// 高级去黑：先通过去背景服务得到透明前景，再与 UnMult 结果 alpha 合成。
pub async fn perfect_unmult_http(
    src: &Path,
    service: RemoveBackgroundService<'_>,
) -> PostprocessResult<PathBuf> {
    let bytes = std::fs::read(src)?;
    let filename = src
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("image.png")
        .to_string();
    let fg_bytes = remove_background_bytes(bytes, filename, service).await?;
    let original = image::open(src).map_err(|e| PostprocessError::Decode(e.to_string()))?;
    let unmult_img = DynamicImage::ImageRgba8(unmult(&original.to_rgba8()));
    let mut bg = unmult_img.to_rgba8();
    let mut fg = image::load_from_memory(&fg_bytes)
        .map_err(|e| PostprocessError::Decode(e.to_string()))?
        .to_rgba8();

    if fg.dimensions() != bg.dimensions() {
        fg = image::imageops::resize(
            &fg,
            bg.width(),
            bg.height(),
            image::imageops::FilterType::Lanczos3,
        );
    }

    image::imageops::overlay(&mut bg, &fg, 0, 0);
    let dest = sibling_with_suffix(src, ".perfect.png");
    bg.save(&dest)
        .map_err(|e| PostprocessError::Write(e.to_string()))?;
    Ok(dest)
}

async fn remove_background_bytes(
    bytes: Vec<u8>,
    filename: String,
    service: RemoveBackgroundService<'_>,
) -> PostprocessResult<Vec<u8>> {
    match service {
        RemoveBackgroundService::Rembg { endpoint } => {
            let url = format!("{}/api/remove", endpoint.trim_end_matches('/'));
            let part = reqwest::multipart::Part::bytes(bytes)
                .file_name(filename)
                .mime_str("image/png")
                .map_err(|e| PostprocessError::Write(e.to_string()))?;
            let form = reqwest::multipart::Form::new().part("file", part);
            let resp = reqwest::Client::new()
                .post(&url)
                .multipart(form)
                .send()
                .await
                .map_err(|e| PostprocessError::Write(format!("Rembg HTTP: {e}")))?;
            if !resp.status().is_success() {
                return Err(PostprocessError::Write(format!(
                    "Rembg HTTP {}: {}",
                    resp.status(),
                    resp.text()
                        .await
                        .unwrap_or_default()
                        .chars()
                        .take(200)
                        .collect::<String>()
                )));
            }
            resp.bytes()
                .await
                .map(|b| b.to_vec())
                .map_err(|e| PostprocessError::Write(e.to_string()))
        }
        RemoveBackgroundService::PhotoRoom { api_key } => {
            const URL: &str = "https://sdk.photoroom.com/v1/segment";
            let part = reqwest::multipart::Part::bytes(bytes)
                .file_name(filename)
                .mime_str("image/png")
                .map_err(|e| PostprocessError::Write(e.to_string()))?;
            let form = reqwest::multipart::Form::new().part("image_file", part);
            let resp = reqwest::Client::new()
                .post(URL)
                .header("x-api-key", api_key)
                .multipart(form)
                .send()
                .await
                .map_err(|e| PostprocessError::Write(format!("PhotoRoom HTTP: {e}")))?;
            if !resp.status().is_success() {
                return Err(PostprocessError::Write(format!(
                    "PhotoRoom HTTP {}: {}",
                    resp.status(),
                    resp.text()
                        .await
                        .unwrap_or_default()
                        .chars()
                        .take(200)
                        .collect::<String>()
                )));
            }
            resp.bytes()
                .await
                .map(|b| b.to_vec())
                .map_err(|e| PostprocessError::Write(e.to_string()))
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum RemoveBackgroundService<'a> {
    Rembg { endpoint: &'a str },
    PhotoRoom { api_key: &'a str },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unmult_black_becomes_transparent() {
        let mut buf = ImageBuffer::new(2, 1);
        buf.put_pixel(0, 0, Rgba([0, 0, 0, 255]));
        buf.put_pixel(1, 0, Rgba([255, 128, 64, 255]));
        let out = unmult(&buf);
        assert_eq!(out.get_pixel(0, 0)[3], 0); // 黑 → 透明
        assert_eq!(out.get_pixel(1, 0)[3], 255); // 高亮 → 不透明
                                                 // RGB 保留
        assert_eq!(out.get_pixel(1, 0)[0], 255);
        assert_eq!(out.get_pixel(1, 0)[1], 128);
    }

    #[test]
    fn unmult_dim_pixel_gets_partial_alpha() {
        let mut buf = ImageBuffer::new(1, 1);
        buf.put_pixel(0, 0, Rgba([20, 30, 10, 255]));
        let out = unmult(&buf);
        // alpha = max(20, 30, 10) = 30
        assert_eq!(out.get_pixel(0, 0)[3], 30);
    }
}
