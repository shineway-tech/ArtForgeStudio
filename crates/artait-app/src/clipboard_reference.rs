use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use image::{ImageBuffer, Rgba};

pub(crate) fn save_clipboard_image_png(sequence_hint: u64) -> Result<Option<PathBuf>> {
    let mut clipboard = match arboard::Clipboard::new() {
        Ok(clipboard) => clipboard,
        Err(arboard::Error::ContentNotAvailable) => return Ok(None),
        Err(e) => return Err(e).context("打开剪贴板失败"),
    };
    let image = match clipboard.get_image() {
        Ok(image) => image,
        Err(arboard::Error::ContentNotAvailable) => return Ok(None),
        Err(e) => return Err(e).context("读取剪贴板图片失败"),
    };

    if image.width == 0 || image.height == 0 || image.bytes.is_empty() {
        return Ok(None);
    }

    let bytes = image.bytes.into_owned();
    let expected = image.width * image.height * 4;
    if bytes.len() != expected {
        bail!("剪贴板图片数据大小异常");
    }

    let buffer =
        ImageBuffer::<Rgba<u8>, Vec<u8>>::from_raw(image.width as u32, image.height as u32, bytes)
            .context("转换剪贴板图片失败")?;

    let dir = artait_model::portable_data_dir().join("reference_clipboard");
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("创建剪贴板参考图目录失败: {}", dir.display()))?;
    let path = dir.join(format!(
        "clipboard-{}-{}.png",
        chrono::Local::now().format("%Y%m%d-%H%M%S"),
        sequence_hint
    ));
    buffer
        .save(&path)
        .with_context(|| format!("保存剪贴板参考图失败: {}", path.display()))?;
    Ok(Some(path))
}

#[cfg(windows)]
pub(crate) fn clipboard_sequence_number() -> u64 {
    extern "system" {
        fn GetClipboardSequenceNumber() -> u32;
    }

    unsafe { GetClipboardSequenceNumber() as u64 }
}

#[cfg(not(windows))]
pub(crate) fn clipboard_sequence_number() -> u64 {
    0
}
