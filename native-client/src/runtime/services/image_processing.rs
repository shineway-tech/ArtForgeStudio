use super::*;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use qrcode::{Color as QrColor, QrCode};

const REFERENCE_UPLOAD_TARGET_BYTES: u64 = 8 * 1024 * 1024;
const REFERENCE_UPLOAD_MAX_EDGE: u32 = 4096;
const REFERENCE_UPLOAD_MIN_EDGE: u32 = 1024;

pub(super) struct PreparedReferenceUpload {
    path: PathBuf,
    temporary: bool,
}

impl PreparedReferenceUpload {
    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    #[cfg(test)]
    fn is_temporary(&self) -> bool {
        self.temporary
    }
}

impl Drop for PreparedReferenceUpload {
    fn drop(&mut self) {
        if self.temporary {
            let _ = fs::remove_file(&self.path);
        }
    }
}

pub(super) fn prepare_reference_for_upload(path: &Path) -> Result<PreparedReferenceUpload> {
    let file_size = fs::metadata(path)?.len();
    let reader = image::ImageReader::open(path)?.with_guessed_format()?;
    let (width, height) = reader.into_dimensions()?;
    if !reference_requires_optimization(file_size, width, height) {
        return Ok(PreparedReferenceUpload {
            path: path.to_path_buf(),
            temporary: false,
        });
    }

    let mut image = image::ImageReader::open(path)?
        .with_guessed_format()?
        .decode()?;
    let preserve_alpha = image.color().has_alpha();
    if image.width().max(image.height()) > REFERENCE_UPLOAD_MAX_EDGE {
        image = image.resize(
            REFERENCE_UPLOAD_MAX_EDGE,
            REFERENCE_UPLOAD_MAX_EDGE,
            image::imageops::FilterType::Lanczos3,
        );
    }

    let (bytes, extension) = loop {
        let (bytes, extension) = encode_reference_upload(&image, preserve_alpha)?;
        if bytes.len() as u64 <= REFERENCE_UPLOAD_TARGET_BYTES
            || image.width().max(image.height()) <= REFERENCE_UPLOAD_MIN_EDGE
        {
            break (bytes, extension);
        }
        let size_ratio =
            (REFERENCE_UPLOAD_TARGET_BYTES as f64 / bytes.len() as f64).sqrt() * 0.95;
        let scale = size_ratio.clamp(0.5, 0.9);
        let width = ((image.width() as f64 * scale).round() as u32).max(1);
        let height = ((image.height() as f64 * scale).round() as u32).max(1);
        image = image.resize_exact(width, height, image::imageops::FilterType::Lanczos3);
    };

    let directory = std::env::temp_dir()
        .join("ArtForgeStudio")
        .join("reference-uploads");
    fs::create_dir_all(&directory)?;
    let destination = directory.join(format!("reference-{}.{}", Uuid::new_v4(), extension));
    atomic_write_file(&destination, &bytes)?;
    Ok(PreparedReferenceUpload {
        path: destination,
        temporary: true,
    })
}

fn reference_requires_optimization(file_size: u64, width: u32, height: u32) -> bool {
    file_size > REFERENCE_UPLOAD_TARGET_BYTES
        || width > REFERENCE_UPLOAD_MAX_EDGE
        || height > REFERENCE_UPLOAD_MAX_EDGE
}

fn encode_reference_upload(
    image: &image::DynamicImage,
    preserve_alpha: bool,
) -> Result<(Vec<u8>, &'static str)> {
    let mut bytes = Vec::new();
    if preserve_alpha {
        let rgba = image.to_rgba8();
        let encoder = image::codecs::webp::WebPEncoder::new_lossless(Cursor::new(&mut bytes));
        image::ImageEncoder::write_image(
            encoder,
            rgba.as_raw(),
            rgba.width(),
            rgba.height(),
            image::ExtendedColorType::Rgba8,
        )?;
        Ok((bytes, "webp"))
    } else {
        let rgb = image.to_rgb8();
        let encoder =
            image::codecs::jpeg::JpegEncoder::new_with_quality(Cursor::new(&mut bytes), 92);
        image::ImageEncoder::write_image(
            encoder,
            rgb.as_raw(),
            rgb.width(),
            rgb.height(),
            image::ExtendedColorType::Rgb8,
        )?;
        Ok((bytes, "jpg"))
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_optimization_only_runs_for_large_files_or_dimensions() {
        assert!(!reference_requires_optimization(
            REFERENCE_UPLOAD_TARGET_BYTES,
            4096,
            4096
        ));
        assert!(reference_requires_optimization(
            REFERENCE_UPLOAD_TARGET_BYTES + 1,
            1024,
            1024
        ));
        assert!(reference_requires_optimization(1024, 4097, 512));
        assert!(reference_requires_optimization(1024, 512, 4097));
    }

    #[test]
    fn oversized_reference_is_resized_without_touching_the_original() {
        let source = std::env::temp_dir().join(format!(
            "artforge-reference-source-{}.png",
            Uuid::new_v4()
        ));
        let rgba = image::RgbaImage::from_pixel(
            REFERENCE_UPLOAD_MAX_EDGE + 64,
            8,
            image::Rgba([40, 120, 210, 180]),
        );
        let original = encode_png_rgba(&rgba, rgba.width(), rgba.height()).expect("encode source");
        fs::write(&source, &original).expect("write source");

        let prepared = prepare_reference_for_upload(&source).expect("prepare reference");
        let prepared_path = prepared.path().to_path_buf();
        let optimized = image::open(&prepared_path).expect("read optimized reference");

        assert!(prepared.is_temporary());
        assert_ne!(prepared_path, source);
        assert!(optimized.width() <= REFERENCE_UPLOAD_MAX_EDGE);
        assert_eq!(fs::read(&source).expect("read source"), original);
        drop(prepared);
        assert!(!prepared_path.exists());
        let _ = fs::remove_file(source);
    }
}
