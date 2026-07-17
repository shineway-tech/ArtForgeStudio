use super::*;

pub(super) fn generated_image_from_bytes(bytes: &[u8], quality: &str) -> Result<(Vec<u8>, Image, i32, i32)> {
    let mut img = image::load_from_memory(bytes)?.to_rgba8();
    let (mut width, mut height) = img.dimensions();
    let max_edge = max_edge_for_quality(quality) as u32;
    let mut output_bytes = bytes.to_vec();
    if width.max(height) > max_edge {
        let (target_width, target_height) = fit_dimensions_to_max_edge(width, height, max_edge);
        img = image::imageops::resize(
            &img,
            target_width,
            target_height,
            image::imageops::FilterType::Lanczos3,
        );
        width = target_width;
        height = target_height;
        output_bytes = encode_png_rgba(&img, width, height)?;
    }
    let image = slint_image_from_rgba(&img, width, height);
    Ok((output_bytes, image, width as i32, height as i32))
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

pub(super) fn max_edge_for_quality(quality: &str) -> i32 {
    match normalized_quality(quality) {
        "4K" => 4096,
        "2K" => 2048,
        _ => 1024,
    }
}

pub(super) fn fit_dimensions_to_max_edge(width: u32, height: u32, max_edge: u32) -> (u32, u32) {
    if width == 0 || height == 0 {
        return (max_edge.max(1), max_edge.max(1));
    }
    if width >= height {
        let target_height =
            ((height as f64 * max_edge as f64 / width as f64).round() as u32).clamp(1, max_edge);
        (max_edge, target_height)
    } else {
        let target_width =
            ((width as f64 * max_edge as f64 / height as f64).round() as u32).clamp(1, max_edge);
        (target_width, max_edge)
    }
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
