use super::*;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use image::ImageDecoder as _;
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
    let (mut image, source_format) = decode_image_file(path)?;
    if is_persisted_reference_path(path)
        && !reference_requires_optimization(file_size, image.width(), image.height())
        && !reference_requires_conversion(source_format)
    {
        return Ok(PreparedReferenceUpload {
            path: path.to_path_buf(),
            temporary: false,
        });
    }

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
        let size_ratio = (REFERENCE_UPLOAD_TARGET_BYTES as f64 / bytes.len() as f64).sqrt() * 0.95;
        let scale = size_ratio.clamp(0.5, 0.9);
        let width = ((image.width() as f64 * scale).round() as u32).max(1);
        let height = ((image.height() as f64 * scale).round() as u32).max(1);
        image = image.resize_exact(width, height, image::imageops::FilterType::Lanczos3);
    };

    let directory = std::env::temp_dir()
        .join("ElunviCanvas")
        .join("reference-uploads");
    fs::create_dir_all(&directory)?;
    let destination = directory.join(format!("reference-{}.{}", Uuid::new_v4(), extension));
    atomic_write_file(&destination, &bytes)?;
    Ok(PreparedReferenceUpload {
        path: destination,
        temporary: true,
    })
}

fn is_persisted_reference_path(path: &Path) -> bool {
    path.parent()
        .is_some_and(|parent| parent == app_data_dir().join("references").join("library"))
}

fn reference_requires_optimization(file_size: u64, width: u32, height: u32) -> bool {
    file_size > REFERENCE_UPLOAD_TARGET_BYTES
        || width > REFERENCE_UPLOAD_MAX_EDGE
        || height > REFERENCE_UPLOAD_MAX_EDGE
}

fn reference_requires_conversion(format: Option<image::ImageFormat>) -> bool {
    !matches!(
        format,
        Some(image::ImageFormat::Png | image::ImageFormat::Jpeg)
    )
}

fn encode_reference_upload(
    image: &image::DynamicImage,
    preserve_alpha: bool,
) -> Result<(Vec<u8>, &'static str)> {
    let mut bytes = Vec::new();
    if preserve_alpha {
        let rgba = image.to_rgba8();
        let encoder = image::codecs::png::PngEncoder::new(Cursor::new(&mut bytes));
        image::ImageEncoder::write_image(
            encoder,
            rgba.as_raw(),
            rgba.width(),
            rgba.height(),
            image::ExtendedColorType::Rgba8,
        )?;
        Ok((bytes, "png"))
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
    let mut rgba =
        image::RgbaImage::from_pixel(size as u32, size as u32, image::Rgba([255, 255, 255, 255]));
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

pub(super) fn generated_image_dimensions(bytes: &[u8]) -> Result<(i32, i32)> {
    let decoded = image::load_from_memory(bytes)?;
    Ok((decoded.width() as i32, decoded.height() as i32))
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

pub(super) fn conversion_format_extension(target_format: &str) -> Option<&'static str> {
    match target_format {
        "jpeg" => Some("jpg"),
        "png" => Some("png"),
        "webp" => Some("webp"),
        "bmp" => Some("bmp"),
        "avif" => Some("avif"),
        _ => None,
    }
}

pub(super) fn convert_image_file(
    source_path: &Path,
    target_format: &str,
) -> Result<(Vec<u8>, &'static str)> {
    let (image, _) = decode_image_file(source_path)?;
    let extension = conversion_format_extension(target_format)
        .ok_or_else(|| anyhow!("unsupported conversion target format"))?;
    let mut bytes = Vec::new();

    match target_format {
        "jpeg" => {
            let rgb = flatten_image_on_white(&image);
            let encoder =
                image::codecs::jpeg::JpegEncoder::new_with_quality(Cursor::new(&mut bytes), 92);
            image::ImageEncoder::write_image(
                encoder,
                rgb.as_raw(),
                rgb.width(),
                rgb.height(),
                image::ExtendedColorType::Rgb8,
            )?;
        }
        "png" => {
            let rgba = image.to_rgba8();
            let encoder = image::codecs::png::PngEncoder::new(Cursor::new(&mut bytes));
            image::ImageEncoder::write_image(
                encoder,
                rgba.as_raw(),
                rgba.width(),
                rgba.height(),
                image::ExtendedColorType::Rgba8,
            )?;
        }
        "webp" => {
            let rgba = image.to_rgba8();
            let encoder = image::codecs::webp::WebPEncoder::new_lossless(Cursor::new(&mut bytes));
            image::ImageEncoder::write_image(
                encoder,
                rgba.as_raw(),
                rgba.width(),
                rgba.height(),
                image::ExtendedColorType::Rgba8,
            )?;
        }
        "bmp" => {
            let rgba = image.to_rgba8();
            let mut encoder = image::codecs::bmp::BmpEncoder::new(&mut bytes);
            encoder.encode(
                rgba.as_raw(),
                rgba.width(),
                rgba.height(),
                image::ExtendedColorType::Rgba8,
            )?;
        }
        "avif" => {
            let rgba = image.to_rgba8();
            let encoder = image::codecs::avif::AvifEncoder::new_with_speed_quality(
                Cursor::new(&mut bytes),
                8,
                85,
            );
            image::ImageEncoder::write_image(
                encoder,
                rgba.as_raw(),
                rgba.width(),
                rgba.height(),
                image::ExtendedColorType::Rgba8,
            )?;
        }
        _ => unreachable!("target format was validated above"),
    }

    Ok((bytes, extension))
}

#[derive(Clone, Copy, Debug)]
pub(super) enum ImageCompressionMode {
    Quality(u8),
    TargetBytes(u64),
}

#[derive(Debug)]
#[allow(dead_code)]
pub(super) struct CompressedImage {
    pub(super) bytes: Vec<u8>,
    pub(super) extension: &'static str,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) downsampled: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CompressionFormat {
    Jpeg,
    Png,
    WebP,
    Bmp,
}

impl CompressionFormat {
    fn from_detected(format: Option<image::ImageFormat>) -> Result<Self> {
        match format {
            Some(image::ImageFormat::Jpeg) => Ok(Self::Jpeg),
            Some(image::ImageFormat::Png) => Ok(Self::Png),
            Some(image::ImageFormat::WebP) => Ok(Self::WebP),
            Some(image::ImageFormat::Bmp) => Ok(Self::Bmp),
            _ => Err(anyhow!("图片压缩仅支持 JPEG、PNG、WebP 和 BMP 格式")),
        }
    }

    fn extension(self) -> &'static str {
        match self {
            Self::Jpeg => "jpg",
            Self::Png => "png",
            Self::WebP => "webp",
            Self::Bmp => "bmp",
        }
    }

    fn target_quality_candidates(self) -> &'static [u8] {
        match self {
            Self::Bmp => &[100],
            Self::Jpeg | Self::Png | Self::WebP => &[100, 95, 80, 60, 40, 20, 1],
        }
    }
}

pub(super) fn compression_source_extension(source_path: &Path) -> Result<&'static str> {
    let (_, detected_format) = decode_image_file(source_path)?;
    Ok(CompressionFormat::from_detected(detected_format)?.extension())
}

pub(super) fn compress_image_file(
    source_path: &Path,
    mode: ImageCompressionMode,
) -> Result<CompressedImage> {
    let source_bytes = fs::read(source_path)
        .with_context(|| format!("无法读取待压缩图片 {}", source_path.display()))?;
    let (image, detected_format) = decode_image_file(source_path)?;
    let format = CompressionFormat::from_detected(detected_format)?;
    let original_width = image.width();
    let original_height = image.height();

    match mode {
        ImageCompressionMode::Quality(quality) => {
            if !(1..=100).contains(&quality) {
                return Err(anyhow!("图片压缩质量必须在 1 到 100 之间"));
            }
            if quality == 100 {
                return Ok(CompressedImage {
                    bytes: source_bytes,
                    extension: format.extension(),
                    width: original_width,
                    height: original_height,
                    downsampled: false,
                });
            }
            compress_image_to_quality(image, format, quality, source_bytes)
        }
        ImageCompressionMode::TargetBytes(target_bytes) => {
            if target_bytes == 0 {
                return Err(anyhow!("图片压缩目标大小必须大于 0 字节"));
            }
            if source_bytes.len() as u64 <= target_bytes {
                return Ok(CompressedImage {
                    bytes: source_bytes,
                    extension: format.extension(),
                    width: original_width,
                    height: original_height,
                    downsampled: false,
                });
            }
            compress_image_to_target(image, format, target_bytes, original_width, original_height)
        }
    }
}

fn compress_image_to_quality(
    image: image::DynamicImage,
    format: CompressionFormat,
    quality: u8,
    source_bytes: Vec<u8>,
) -> Result<CompressedImage> {
    let original_width = image.width();
    let original_height = image.height();
    let mut working = if format == CompressionFormat::Bmp {
        let scale = (f64::from(quality) / 100.0).sqrt();
        resize_image_by_scale(&image, scale)
    } else {
        image
    };
    let mut bytes = encode_compressed_image(&working, format, quality)?;

    for _ in 0..20 {
        let should_downsample = if format == CompressionFormat::Bmp {
            bytes.len() >= source_bytes.len()
        } else {
            bytes.len() > source_bytes.len()
        };
        if !should_downsample || (working.width() == 1 && working.height() == 1) {
            break;
        }

        let size_ratio = (source_bytes.len() as f64 / bytes.len() as f64).sqrt() * 0.96;
        let scale = size_ratio.clamp(0.5, 0.95);
        let resized = resize_image_by_scale(&working, scale);
        if resized.width() == working.width() && resized.height() == working.height() {
            break;
        }
        working = resized;
        bytes = encode_compressed_image(&working, format, quality)?;
    }

    if bytes.len() > source_bytes.len()
        || (format == CompressionFormat::Bmp && bytes.len() == source_bytes.len())
    {
        return Ok(CompressedImage {
            bytes: source_bytes,
            extension: format.extension(),
            width: original_width,
            height: original_height,
            downsampled: false,
        });
    }

    Ok(CompressedImage {
        bytes,
        extension: format.extension(),
        width: working.width(),
        height: working.height(),
        downsampled: working.width() != original_width || working.height() != original_height,
    })
}

fn compress_image_to_target(
    mut image: image::DynamicImage,
    format: CompressionFormat,
    target_bytes: u64,
    original_width: u32,
    original_height: u32,
) -> Result<CompressedImage> {
    let mut smallest_generated_size = u64::MAX;

    for _ in 0..24 {
        let (candidate, generated_minimum) =
            encode_best_target_candidate(&image, format, target_bytes)?;
        smallest_generated_size = smallest_generated_size.min(generated_minimum);
        if let Some(bytes) = candidate {
            return Ok(CompressedImage {
                bytes,
                extension: format.extension(),
                width: image.width(),
                height: image.height(),
                downsampled: image.width() != original_width || image.height() != original_height,
            });
        }

        if image.width() == 1 && image.height() == 1 {
            break;
        }

        let size_ratio = (target_bytes as f64 / smallest_generated_size as f64).sqrt() * 0.92;
        let scale = size_ratio.clamp(0.1, 0.9);
        let resized = resize_image_by_scale(&image, scale);
        if resized.width() == image.width() && resized.height() == image.height() {
            break;
        }
        image = resized;
    }

    Err(anyhow!(
        "无法将图片压缩到指定大小：目标为 {} 字节，可生成的最小文件为 {} 字节",
        target_bytes,
        smallest_generated_size
    ))
}

fn encode_best_target_candidate(
    image: &image::DynamicImage,
    format: CompressionFormat,
    target_bytes: u64,
) -> Result<(Option<Vec<u8>>, u64)> {
    if format == CompressionFormat::Jpeg {
        let mut low = 1u8;
        let mut high = 100u8;
        let mut best = None;
        let mut smallest_generated_size = u64::MAX;
        while low <= high {
            let quality = low + (high - low) / 2;
            let bytes = encode_compressed_image(image, format, quality)?;
            let generated_size = bytes.len() as u64;
            smallest_generated_size = smallest_generated_size.min(generated_size);
            if generated_size <= target_bytes {
                best = Some(bytes);
                if quality == 100 {
                    break;
                }
                low = quality + 1;
            } else {
                if quality == 1 {
                    break;
                }
                high = quality - 1;
            }
        }
        return Ok((best, smallest_generated_size));
    }

    let mut smallest_generated_size = u64::MAX;
    for &quality in format.target_quality_candidates() {
        let bytes = encode_compressed_image(image, format, quality)?;
        let generated_size = bytes.len() as u64;
        smallest_generated_size = smallest_generated_size.min(generated_size);
        if generated_size <= target_bytes {
            return Ok((Some(bytes), smallest_generated_size));
        }
    }
    Ok((None, smallest_generated_size))
}

fn encode_compressed_image(
    image: &image::DynamicImage,
    format: CompressionFormat,
    quality: u8,
) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    match format {
        CompressionFormat::Jpeg => {
            let rgb = flatten_image_on_white(image);
            let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(
                Cursor::new(&mut bytes),
                quality,
            );
            image::ImageEncoder::write_image(
                encoder,
                rgb.as_raw(),
                rgb.width(),
                rgb.height(),
                image::ExtendedColorType::Rgb8,
            )?;
        }
        CompressionFormat::Png => {
            let mut rgba = image.to_rgba8();
            quantize_rgba_for_quality(&mut rgba, quality);
            let encoder = image::codecs::png::PngEncoder::new(Cursor::new(&mut bytes));
            image::ImageEncoder::write_image(
                encoder,
                rgba.as_raw(),
                rgba.width(),
                rgba.height(),
                image::ExtendedColorType::Rgba8,
            )?;
        }
        CompressionFormat::WebP => {
            let mut rgba = image.to_rgba8();
            quantize_rgba_for_quality(&mut rgba, quality);
            let encoder = image::codecs::webp::WebPEncoder::new_lossless(Cursor::new(&mut bytes));
            image::ImageEncoder::write_image(
                encoder,
                rgba.as_raw(),
                rgba.width(),
                rgba.height(),
                image::ExtendedColorType::Rgba8,
            )?;
        }
        CompressionFormat::Bmp => {
            let mut rgba = image.to_rgba8();
            quantize_rgba_for_quality(&mut rgba, quality);
            if rgba.pixels().all(|pixel| pixel[3] == 255) {
                let rgb = image::DynamicImage::ImageRgba8(rgba).to_rgb8();
                let mut encoder = image::codecs::bmp::BmpEncoder::new(&mut bytes);
                encoder.encode(
                    rgb.as_raw(),
                    rgb.width(),
                    rgb.height(),
                    image::ExtendedColorType::Rgb8,
                )?;
            } else {
                let mut encoder = image::codecs::bmp::BmpEncoder::new(&mut bytes);
                encoder.encode(
                    rgba.as_raw(),
                    rgba.width(),
                    rgba.height(),
                    image::ExtendedColorType::Rgba8,
                )?;
            }
        }
    }
    Ok(bytes)
}

fn quantize_rgba_for_quality(rgba: &mut image::RgbaImage, quality: u8) {
    let step = match quality {
        100 => 1,
        90..=99 => 2,
        75..=89 => 4,
        55..=74 => 8,
        35..=54 => 16,
        15..=34 => 32,
        _ => 64,
    };
    if step == 1 {
        return;
    }

    for pixel in rgba.pixels_mut() {
        for channel in &mut pixel.0[..3] {
            let quantized = ((u16::from(*channel) + step / 2) / step) * step;
            *channel = quantized.min(255) as u8;
        }
    }
}

fn resize_image_by_scale(image: &image::DynamicImage, scale: f64) -> image::DynamicImage {
    let width = ((image.width() as f64 * scale).floor() as u32)
        .max(1)
        .min(image.width());
    let height = ((image.height() as f64 * scale).floor() as u32)
        .max(1)
        .min(image.height());
    image.resize_exact(width, height, image::imageops::FilterType::Lanczos3)
}

fn flatten_image_on_white(image: &image::DynamicImage) -> image::RgbImage {
    let rgba = image.to_rgba8();
    let mut rgb = image::RgbImage::new(rgba.width(), rgba.height());
    for (x, y, pixel) in rgba.enumerate_pixels() {
        let alpha = u32::from(pixel[3]);
        let blend = |channel: u8| {
            (((u32::from(channel) * alpha) + (255 * (255 - alpha)) + 127) / 255) as u8
        };
        rgb.put_pixel(
            x,
            y,
            image::Rgb([blend(pixel[0]), blend(pixel[1]), blend(pixel[2])]),
        );
    }
    rgb
}

pub(super) fn slint_image_from_rgba(rgba: &image::RgbaImage, width: u32, height: u32) -> Image {
    let buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
        rgba.as_raw(),
        width,
        height,
    );
    Image::from_rgba8(buffer)
}

pub(super) fn persist_reference_source(path: &Path) -> Result<PathBuf> {
    let (decoded, _) = decode_image_file(path)?;
    persist_reference_image(&decoded)
}

pub(super) fn persist_colorization_source(path: &Path) -> Result<PathBuf> {
    let (decoded, _) = decode_image_file(path)?;
    persist_reference_image(&flatten_colorization_image(&decoded))
}

fn flatten_colorization_image(image: &image::DynamicImage) -> image::DynamicImage {
    let rgba = image.to_rgba8();
    let mut rgb = image::RgbImage::new(rgba.width(), rgba.height());
    for (x, y, pixel) in rgba.enumerate_pixels() {
        let alpha = u32::from(pixel[3]);
        let blend = |channel: u8| {
            (((u32::from(channel) * alpha) + (255 * (255 - alpha)) + 127) / 255) as u8
        };
        rgb.put_pixel(
            x,
            y,
            image::Rgb([blend(pixel[0]), blend(pixel[1]), blend(pixel[2])]),
        );
    }
    image::DynamicImage::ImageRgb8(rgb)
}

pub(super) fn persist_clipboard_reference(img: &arboard::ImageData<'_>) -> Result<PathBuf> {
    let rgba = image::RgbaImage::from_raw(
        img.width as u32,
        img.height as u32,
        img.bytes.as_ref().to_vec(),
    )
    .ok_or_else(|| anyhow!("剪贴板图片数据无效"))?;
    persist_reference_image(&image::DynamicImage::ImageRgba8(rgba))
}

pub(super) fn persist_slint_reference(image: &Image) -> Result<PathBuf> {
    let buffer = image
        .to_rgba8()
        .ok_or_else(|| anyhow!("参考图像素数据不可用"))?;
    let rgba =
        image::RgbaImage::from_raw(buffer.width(), buffer.height(), buffer.as_bytes().to_vec())
            .ok_or_else(|| anyhow!("参考图像素数据无效"))?;
    persist_reference_image(&image::DynamicImage::ImageRgba8(rgba))
}

fn persist_reference_image(image: &image::DynamicImage) -> Result<PathBuf> {
    use sha2::{Digest, Sha256};

    let preserve_alpha = image.color().has_alpha();
    let (bytes, extension) = encode_reference_upload(image, preserve_alpha)?;
    let digest = format!("{:x}", Sha256::digest(&bytes));
    let directory = app_data_dir().join("references").join("library");
    if !ensure_managed_subdirectory(&directory) {
        return Err(anyhow!("无法创建安全的参考图目录"));
    }
    let destination = directory.join(format!("{digest}.{extension}"));
    if !destination.is_file() {
        atomic_write_file(&destination, &bytes)?;
    }
    Ok(destination)
}

pub(super) fn decode_image_file(
    path: &Path,
) -> Result<(image::DynamicImage, Option<image::ImageFormat>)> {
    let decoded = (|| -> image::ImageResult<_> {
        let reader = image::ImageReader::open(path)?.with_guessed_format()?;
        let format = reader.format();
        let mut decoder = reader.into_decoder()?;
        let orientation = decoder.orientation()?;
        let mut image = image::DynamicImage::from_decoder(decoder)?;
        image.apply_orientation(orientation);
        Ok((image, format))
    })();

    match decoded {
        Ok(image) => Ok(image),
        Err(error) => {
            #[cfg(target_os = "macos")]
            if is_macos_native_image(path) {
                return decode_macos_native_image(path)
                    .map(|image| (image, None))
                    .with_context(|| format!("无法读取图片 {}", path.display()));
            }
            Err(error).with_context(|| format!("无法读取图片 {}", path.display()))
        }
    }
}

#[cfg(target_os = "macos")]
fn is_macos_native_image(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| matches!(extension.to_ascii_lowercase().as_str(), "heic" | "heif"))
}

#[cfg(target_os = "macos")]
fn decode_macos_native_image(path: &Path) -> Result<image::DynamicImage> {
    use objc2::AllocAnyThread;
    use objc2_app_kit::NSImage;
    use objc2_foundation::NSString;

    let file_name = path
        .to_str()
        .map(NSString::from_str)
        .ok_or_else(|| anyhow!("图片路径不是有效文本"))?;
    let native_image = NSImage::initWithContentsOfFile(NSImage::alloc(), &file_name)
        .ok_or_else(|| anyhow!("macOS 无法解码该图片"))?;
    let tiff_data = native_image
        .TIFFRepresentation()
        .ok_or_else(|| anyhow!("macOS 无法转换该图片"))?;
    let bytes = unsafe { tiff_data.as_bytes_unchecked() };
    image::load_from_memory_with_format(bytes, image::ImageFormat::Tiff).map_err(Into::into)
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
    fn external_png_is_reencoded_before_upload_even_when_it_is_small() {
        let source = std::env::temp_dir().join(format!(
            "artforge-external-reference-{}.png",
            Uuid::new_v4()
        ));
        image::RgbaImage::from_pixel(4, 3, image::Rgba([20, 80, 160, 200]))
            .save_with_format(&source, image::ImageFormat::Png)
            .expect("write external png");

        let prepared = prepare_reference_for_upload(&source).expect("prepare external reference");
        let prepared_path = prepared.path().to_path_buf();
        assert!(prepared.is_temporary());
        assert_ne!(prepared_path, source);
        drop(prepared);
        assert!(!prepared_path.exists());
        let _ = fs::remove_file(source);
    }

    #[test]
    fn exif_orientation_is_applied_before_reference_metadata_is_removed() {
        let source = std::env::temp_dir().join(format!(
            "artforge-oriented-reference-{}.jpg",
            Uuid::new_v4()
        ));
        let mut encoded = Vec::new();
        let mut encoder =
            image::codecs::jpeg::JpegEncoder::new_with_quality(Cursor::new(&mut encoded), 95);
        image::ImageEncoder::set_exif_metadata(
            &mut encoder,
            vec![
                0x49, 0x49, 0x2a, 0x00, 0x08, 0x00, 0x00, 0x00, 0x01, 0x00, 0x12, 0x01, 0x03, 0x00,
                0x01, 0x00, 0x00, 0x00, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            ],
        )
        .expect("attach exif orientation");
        encoder
            .encode(
                &[255, 0, 0, 0, 0, 255],
                2,
                1,
                image::ExtendedColorType::Rgb8,
            )
            .expect("encode oriented jpeg");
        fs::write(&source, encoded).expect("write oriented jpeg");

        let (decoded, _) = decode_image_file(&source).expect("decode oriented jpeg");
        assert_eq!((decoded.width(), decoded.height()), (1, 2));

        let prepared = prepare_reference_for_upload(&source).expect("prepare oriented reference");
        let prepared_path = prepared.path().to_path_buf();
        let (prepared_image, _) =
            decode_image_file(&prepared_path).expect("decode prepared reference");
        assert_eq!((prepared_image.width(), prepared_image.height()), (1, 2));
        drop(prepared);
        assert!(!prepared_path.exists());
        let _ = fs::remove_file(source);
    }

    #[test]
    fn small_non_upload_format_is_converted_before_upload() {
        let source =
            std::env::temp_dir().join(format!("artforge-reference-source-{}.bmp", Uuid::new_v4()));
        image::RgbaImage::from_pixel(4, 4, image::Rgba([20, 80, 160, 255]))
            .save_with_format(&source, image::ImageFormat::Bmp)
            .expect("write bmp");

        let prepared = prepare_reference_for_upload(&source).expect("prepare reference");
        let prepared_path = prepared.path().to_path_buf();

        assert!(prepared.is_temporary());
        assert_ne!(prepared_path, source);
        assert!(matches!(
            image::ImageReader::open(&prepared_path)
                .expect("open prepared image")
                .with_guessed_format()
                .expect("guess prepared format")
                .format(),
            Some(image::ImageFormat::Jpeg | image::ImageFormat::Png)
        ));
        drop(prepared);
        assert!(!prepared_path.exists());
        let _ = fs::remove_file(source);
    }

    #[test]
    fn webp_references_are_reencoded_as_png_or_jpeg() {
        for (has_alpha, expected_format, expected_extension) in [
            (true, image::ImageFormat::Png, "png"),
            (false, image::ImageFormat::Jpeg, "jpg"),
        ] {
            let source = std::env::temp_dir()
                .join(format!("artforge-reference-source-{}.webp", Uuid::new_v4()));
            let mut source_bytes = Vec::new();
            let encoder =
                image::codecs::webp::WebPEncoder::new_lossless(Cursor::new(&mut source_bytes));
            if has_alpha {
                image::ImageEncoder::write_image(
                    encoder,
                    &[20, 80, 160, 128],
                    1,
                    1,
                    image::ExtendedColorType::Rgba8,
                )
                .expect("encode alpha webp");
            } else {
                image::ImageEncoder::write_image(
                    encoder,
                    &[20, 80, 160],
                    1,
                    1,
                    image::ExtendedColorType::Rgb8,
                )
                .expect("encode opaque webp");
            }
            fs::write(&source, source_bytes).expect("write webp source");

            let prepared = prepare_reference_for_upload(&source).expect("prepare reference");
            let prepared_path = prepared.path().to_path_buf();
            assert!(prepared.is_temporary());
            assert_eq!(
                prepared_path.extension().and_then(|value| value.to_str()),
                Some(expected_extension)
            );
            assert_eq!(
                image::ImageReader::open(&prepared_path)
                    .expect("open prepared image")
                    .with_guessed_format()
                    .expect("guess prepared format")
                    .format(),
                Some(expected_format)
            );
            assert!(
                fs::metadata(&prepared_path)
                    .expect("prepared metadata")
                    .len()
                    <= REFERENCE_UPLOAD_TARGET_BYTES
            );
            drop(prepared);
            assert!(!prepared_path.exists());
            let _ = fs::remove_file(source);
        }
    }

    #[test]
    fn colorization_source_flattens_transparency_and_uses_jpeg() {
        let source = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            2,
            2,
            image::Rgba([20, 80, 160, 0]),
        ));
        let flattened = flatten_colorization_image(&source);
        let (bytes, extension) = encode_reference_upload(&flattened, flattened.color().has_alpha())
            .expect("encode colorization reference");

        assert_eq!(extension, "jpg");
        assert!(!flattened.color().has_alpha());
        assert_eq!(
            image::load_from_memory(&bytes)
                .expect("decode colorization reference")
                .to_rgb8()
                .get_pixel(0, 0)
                .0,
            [255, 255, 255]
        );
    }

    #[test]
    fn common_image_formats_decode_for_preview() {
        for (extension, format) in [
            ("bmp", image::ImageFormat::Bmp),
            ("gif", image::ImageFormat::Gif),
            ("tiff", image::ImageFormat::Tiff),
        ] {
            let source = std::env::temp_dir().join(format!(
                "artforge-reference-source-{}.{}",
                Uuid::new_v4(),
                extension
            ));
            image::RgbaImage::from_pixel(3, 2, image::Rgba([40, 120, 210, 180]))
                .save_with_format(&source, format)
                .expect("write common image format");

            let (decoded, detected_format) =
                decode_image_file(&source).expect("decode common image format");
            assert_eq!(decoded.width(), 3);
            assert_eq!(decoded.height(), 2);
            assert_eq!(detected_format, Some(format));
            let _ = fs::remove_file(source);
        }
    }

    #[test]
    fn decode_uses_file_content_when_extension_is_wrong() {
        let source =
            std::env::temp_dir().join(format!("artforge-reference-source-{}.jpg", Uuid::new_v4()));
        let rgba = image::RgbaImage::from_pixel(3, 2, image::Rgba([40, 120, 210, 255]));
        let bytes = encode_png_rgba(&rgba, rgba.width(), rgba.height()).expect("encode png");
        fs::write(&source, bytes).expect("write png with jpeg extension");

        let (preview, detected_format) =
            decode_image_file(&source).expect("decode image from its content");
        let preview = preview.to_rgba8();

        assert_eq!(preview.width(), 3);
        assert_eq!(preview.height(), 2);
        assert_eq!(detected_format, Some(image::ImageFormat::Png));
        let _ = fs::remove_file(source);
    }

    #[test]
    fn local_conversion_encodes_every_supported_target_format() {
        let source =
            std::env::temp_dir().join(format!("artforge-conversion-source-{}.jpg", Uuid::new_v4()));
        let rgba = image::RgbaImage::from_pixel(8, 6, image::Rgba([40, 120, 210, 180]));
        let source_bytes =
            encode_png_rgba(&rgba, rgba.width(), rgba.height()).expect("encode source png");
        fs::write(&source, source_bytes).expect("write png with jpeg extension");

        for (target, extension, expected_format) in [
            ("jpeg", "jpg", image::ImageFormat::Jpeg),
            ("png", "png", image::ImageFormat::Png),
            ("webp", "webp", image::ImageFormat::WebP),
            ("bmp", "bmp", image::ImageFormat::Bmp),
            ("avif", "avif", image::ImageFormat::Avif),
        ] {
            let (bytes, actual_extension) =
                convert_image_file(&source, target).expect("convert local image");
            assert_eq!(actual_extension, extension);
            assert_eq!(
                image::guess_format(&bytes).expect("detect converted format"),
                expected_format
            );
            if expected_format != image::ImageFormat::Avif {
                let converted = image::load_from_memory_with_format(&bytes, expected_format)
                    .expect("decode converted image");
                assert_eq!(converted.width(), 8);
                assert_eq!(converted.height(), 6);
            }
        }

        let _ = fs::remove_file(source);
    }

    #[test]
    fn jpeg_conversion_flattens_transparency_onto_white() {
        let source = std::env::temp_dir().join(format!(
            "artforge-transparent-source-{}.png",
            Uuid::new_v4()
        ));
        let rgba = image::RgbaImage::from_pixel(8, 8, image::Rgba([0, 0, 0, 0]));
        let source_bytes =
            encode_png_rgba(&rgba, rgba.width(), rgba.height()).expect("encode source png");
        fs::write(&source, source_bytes).expect("write transparent source");

        let (bytes, _) = convert_image_file(&source, "jpeg").expect("convert transparent image");
        let pixel = image::load_from_memory_with_format(&bytes, image::ImageFormat::Jpeg)
            .expect("decode converted jpeg")
            .to_rgb8()
            .get_pixel(4, 4)
            .0;

        assert!(pixel.into_iter().all(|channel| channel >= 250));
        let _ = fs::remove_file(source);
    }

    #[test]
    fn compression_detects_content_instead_of_trusting_the_extension() {
        let source = std::env::temp_dir().join(format!(
            "artforge-compression-source-{}.jpg",
            Uuid::new_v4()
        ));
        let rgba = image::RgbaImage::from_pixel(5, 4, image::Rgba([40, 120, 210, 255]));
        let original =
            encode_png_rgba(&rgba, rgba.width(), rgba.height()).expect("encode source png");
        fs::write(&source, &original).expect("write png with jpeg extension");

        assert_eq!(
            compression_source_extension(&source).expect("detect compression source"),
            "png"
        );
        let compressed = compress_image_file(&source, ImageCompressionMode::Quality(100))
            .expect("preserve lossless source");
        assert_eq!(compressed.bytes, original);
        assert_eq!(compressed.extension, "png");
        assert_eq!((compressed.width, compressed.height), (5, 4));
        assert!(!compressed.downsampled);

        let _ = fs::remove_file(source);
    }

    #[test]
    fn compression_rejects_formats_the_tool_does_not_support() {
        let source = std::env::temp_dir().join(format!(
            "artforge-compression-source-{}.gif",
            Uuid::new_v4()
        ));
        image::RgbaImage::from_pixel(2, 2, image::Rgba([40, 120, 210, 255]))
            .save_with_format(&source, image::ImageFormat::Gif)
            .expect("write gif");

        let error = compression_source_extension(&source).expect_err("reject gif");
        assert!(error.to_string().contains("JPEG、PNG、WebP 和 BMP"));
        let _ = fs::remove_file(source);
    }

    #[test]
    fn quality_compression_reencodes_jpeg_without_growing_the_file() {
        let source = std::env::temp_dir().join(format!(
            "artforge-compression-source-{}.jpg",
            Uuid::new_v4()
        ));
        let rgba = image::RgbaImage::from_fn(128, 96, |x, y| {
            image::Rgba([
                ((x * 17 + y * 5) % 256) as u8,
                ((x * 7 + y * 19) % 256) as u8,
                ((x * 13 + y * 11) % 256) as u8,
                255,
            ])
        });
        let image = image::DynamicImage::ImageRgba8(rgba);
        let original =
            encode_compressed_image(&image, CompressionFormat::Jpeg, 100).expect("encode jpeg");
        fs::write(&source, &original).expect("write jpeg");

        let compressed =
            compress_image_file(&source, ImageCompressionMode::Quality(45)).expect("compress jpeg");
        assert_eq!(compressed.extension, "jpg");
        assert!(compressed.bytes.len() < original.len());
        assert_eq!(
            image::guess_format(&compressed.bytes).expect("detect compressed jpeg"),
            image::ImageFormat::Jpeg
        );
        assert_eq!((compressed.width, compressed.height), (128, 96));
        assert!(!compressed.downsampled);

        let _ = fs::remove_file(source);
    }

    #[test]
    fn png_and_webp_quality_quantization_preserves_alpha() {
        let mut rgba = image::RgbaImage::from_pixel(1, 1, image::Rgba([123, 78, 211, 137]));
        quantize_rgba_for_quality(&mut rgba, 40);
        let pixel = rgba.get_pixel(0, 0).0;

        assert_ne!(&pixel[..3], &[123, 78, 211]);
        assert_eq!(pixel[3], 137);
    }

    #[test]
    fn target_size_compression_downsamples_bmp_until_it_fits() {
        let source = std::env::temp_dir().join(format!(
            "artforge-compression-source-{}.bmp",
            Uuid::new_v4()
        ));
        let rgba = image::RgbaImage::from_fn(160, 120, |x, y| {
            image::Rgba([
                ((x * 17 + y * 5) % 256) as u8,
                ((x * 7 + y * 19) % 256) as u8,
                ((x * 13 + y * 11) % 256) as u8,
                255,
            ])
        });
        let image = image::DynamicImage::ImageRgba8(rgba);
        let original =
            encode_compressed_image(&image, CompressionFormat::Bmp, 100).expect("encode bmp");
        fs::write(&source, &original).expect("write bmp");
        let target_bytes = (original.len() / 5) as u64;

        let compressed =
            compress_image_file(&source, ImageCompressionMode::TargetBytes(target_bytes))
                .expect("compress bmp to target size");
        assert!(compressed.bytes.len() as u64 <= target_bytes);
        assert_eq!(compressed.extension, "bmp");
        assert!(compressed.width < 160);
        assert!(compressed.height < 120);
        assert!(compressed.downsampled);
        assert_eq!(
            image::guess_format(&compressed.bytes).expect("detect compressed bmp"),
            image::ImageFormat::Bmp
        );

        let _ = fs::remove_file(source);
    }

    #[test]
    fn target_size_compression_reports_an_impossible_limit() {
        let source = std::env::temp_dir().join(format!(
            "artforge-compression-source-{}.png",
            Uuid::new_v4()
        ));
        let rgba = image::RgbaImage::from_pixel(1, 1, image::Rgba([40, 120, 210, 255]));
        let original =
            encode_png_rgba(&rgba, rgba.width(), rgba.height()).expect("encode source png");
        fs::write(&source, original).expect("write png");

        let error = compress_image_file(&source, ImageCompressionMode::TargetBytes(1))
            .expect_err("one byte target must fail");
        assert!(error.to_string().contains("无法将图片压缩到指定大小"));

        let _ = fs::remove_file(source);
    }

    #[test]
    fn compression_rejects_invalid_quality_values() {
        let source = std::env::temp_dir().join(format!(
            "artforge-compression-source-{}.png",
            Uuid::new_v4()
        ));
        let rgba = image::RgbaImage::from_pixel(1, 1, image::Rgba([40, 120, 210, 255]));
        let original =
            encode_png_rgba(&rgba, rgba.width(), rgba.height()).expect("encode source png");
        fs::write(&source, original).expect("write png");

        let error = compress_image_file(&source, ImageCompressionMode::Quality(0))
            .expect_err("zero quality must fail");
        assert!(error.to_string().contains("1 到 100"));

        let _ = fs::remove_file(source);
    }

    #[test]
    fn oversized_reference_is_resized_without_touching_the_original() {
        let source =
            std::env::temp_dir().join(format!("artforge-reference-source-{}.png", Uuid::new_v4()));
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
