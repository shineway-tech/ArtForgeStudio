use super::*;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use qrcode::{Color as QrColor, QrCode};

pub(super) fn qr_image(data: &str) -> Result<Image> {
    let code = QrCode::new(data.as_bytes()).map_err(|error| anyhow!(error.to_string()))?;
    let quiet_zone = 4usize;
    let scale = 6usize;
    let modules = code.width();
    let size = (modules + quiet_zone * 2) * scale;
    let mut rgba = image::RgbaImage::from_pixel(
        size as u32,
        size as u32,
        image::Rgba([255, 255, 255, 255]),
    );
    let colors = code.to_colors();
    for y in 0..modules {
        for x in 0..modules {
            if colors[y * modules + x] != QrColor::Dark {
                continue;
            }
            let left = (x + quiet_zone) * scale;
            let top = (y + quiet_zone) * scale;
            for py in top..top + scale {
                for px in left..left + scale {
                    rgba.put_pixel(px as u32, py as u32, image::Rgba([0, 0, 0, 255]));
                }
            }
        }
    }
    Ok(slint_image_from_rgba(&rgba, size as u32, size as u32))
}

pub(super) fn encoded_image(data: &str) -> Result<Image> {
    let encoded = data.split_once(',').map_or(data, |(_, payload)| payload);
    let bytes = STANDARD.decode(encoded.trim())?;
    let rgba = image::load_from_memory(&bytes)?.to_rgba8();
    let (width, height) = rgba.dimensions();
    Ok(slint_image_from_rgba(&rgba, width, height))
}

pub(super) fn generated_image_from_bytes(bytes: &[u8]) -> Result<(Vec<u8>, Image, i32, i32)> {
    let rgba = image::load_from_memory(bytes)?.to_rgba8();
    let (width, height) = rgba.dimensions();
    let image = slint_image_from_rgba(&rgba, width, height);
    Ok((bytes.to_vec(), image, width as i32, height as i32))
}

pub(super) fn encode_png_rgba(rgba: &image::RgbaImage, width: u32, height: u32) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let encoder = image::codecs::png::PngEncoder::new(Cursor::new(&mut bytes));
    image::ImageEncoder::write_image(
        encoder,
        rgba.as_raw(),
        width,
        height,
        image::ExtendedColorType::Rgba8,
    )?;
    Ok(bytes)
}

pub(super) fn slint_image_from_rgba(rgba: &image::RgbaImage, width: u32, height: u32) -> Image {
    let buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
        rgba.as_raw(),
        width,
        height,
    );
    Image::from_rgba8(buffer)
}

pub(super) fn image_from_clipboard(img: arboard::ImageData<'_>) -> Image {
    let buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
        img.bytes.as_ref(),
        img.width as u32,
        img.height as u32,
    );
    Image::from_rgba8(buffer)
}

pub(super) fn load_image(path: &Path) -> Result<Image> {
    Image::load_from_path(path).map_err(|_| anyhow!("无法读取图片"))
}
