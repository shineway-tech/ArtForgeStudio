use super::*;
use sha2::{Digest, Sha256};
use std::io::SeekFrom;

const MAX_VIDEO_COVER_SOURCE_BYTES: u64 = 512 * 1024 * 1024;
const VIDEO_COVER_VERSION: u32 = 1;

pub(super) fn prepare_owned_video_cover(
    persistence: &PrivatePersistence,
    output: &SavedVideoOutput,
    should_continue: impl Fn() -> bool,
) -> Result<PreparedDeliveryPreview> {
    let activity = persistence.begin_activity()?;
    let current = || should_continue() && !activity.is_quiescing() && persistence.is_current();
    anyhow::ensure!(current(), "video cover cancelled");
    let authority = persistence.storage_authority()?;
    let relative = Path::new(&output.source_path)
        .strip_prefix(persistence.lease().namespace.path(ManagedUserArea::Videos))?;
    let source_key = ManagedFileKey::new(
        ManagedUserArea::Videos,
        relative
            .to_str()
            .ok_or_else(|| anyhow!("invalid video cover source"))?,
    )?;
    anyhow::ensure!(
        output.size_bytes > 0
            && output.size_bytes <= MAX_VIDEO_COVER_SOURCE_BYTES
            && output.sha256.len() == 64
            && output.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "invalid video cover receipt"
    );
    let mut source = authority.open_existing_regular(&source_key)?;
    let metadata = authority.inspect_regular(&source)?;
    anyhow::ensure!(
        metadata.link_count == 1 && metadata.byte_size == output.size_bytes,
        "video cover source changed"
    );
    let cache_key = ManagedFileKey::new(
        ManagedUserArea::Previews,
        &format!(
            "video-cover-v{VIDEO_COVER_VERSION}-{}.png",
            output.sha256.to_ascii_lowercase()
        ),
    )?;
    let cached = authority.open_optional_regular(&cache_key)?;
    if cached.is_some() {
        let path = persistence
            .lease()
            .namespace
            .path(ManagedUserArea::Previews)
            .join(cache_key.relative_name().as_str());
        let preview = prepare_owned_preview(persistence, &path, PreviewPurpose::Gallery);
        if current() {
            if let Ok(preview) = preview {
                return Ok(preview);
            }
        }
    }
    let bytes = authority.with_regular_reader(&mut source, |reader| {
        reader.seek(SeekFrom::Start(0))?;
        let mut bytes = Vec::with_capacity(output.size_bytes as usize);
        let mut digest = Sha256::new();
        let mut buffer = [0; 64 * 1024];
        while bytes.len() < output.size_bytes as usize {
            anyhow::ensure!(current(), "video cover cancelled");
            let count = reader
                .read(&mut buffer[..(output.size_bytes as usize - bytes.len()).min(64 * 1024)])?;
            anyhow::ensure!(count > 0, "video cover source truncated");
            digest.update(&buffer[..count]);
            bytes.extend_from_slice(&buffer[..count]);
        }
        anyhow::ensure!(
            format!("{:x}", digest.finalize()).eq_ignore_ascii_case(&output.sha256),
            "video cover receipt mismatch"
        );
        Ok(bytes)
    })?;
    let frame = extract_video_cover_frame(&bytes, output.duration_secs, &current)?;
    anyhow::ensure!(current(), "video cover cancelled");
    let frame = if frame.width().max(frame.height()) > 384 {
        frame.thumbnail(384, 384)
    } else {
        frame
    };
    let preview = prepare_video_cover_pixels(frame.clone());
    let mut encoded = Cursor::new(Vec::new());
    frame.write_to(&mut encoded, image::ImageFormat::Png)?;
    let cache_result = (|| -> Result<()> {
        let _effect = persistence.begin_effect()?;
        let _mutation = authority.begin_ordinary_mutation()?;
        anyhow::ensure!(current(), "video cover cancelled");
        let previous = authority.open_optional_regular(&cache_key)?;
        let mut temporary = authority.create_temporary_regular_for(&cache_key)?;
        let write = (|| -> Result<()> {
            authority
                .write_new_regular_from(&mut temporary, &mut Cursor::new(encoded.into_inner()))?;
            authority.sync_regular(&mut temporary)?;
            anyhow::ensure!(current(), "video cover cancelled");
            let destination = previous.as_ref().map_or(
                NamespaceManagedPublication::Absent(&cache_key),
                NamespaceManagedPublication::Replace,
            );
            authority.publish_regular(&mut temporary, destination)
        })();
        if write.is_err() {
            let _ = authority.unlink_regular(temporary);
        }
        write
    })();
    let _ = cache_result;
    anyhow::ensure!(current(), "video cover cancelled");
    Ok(preview)
}

fn frame_has_picture(frame: &image::DynamicImage) -> bool {
    let pixels = frame.thumbnail(64, 64).to_rgb8();
    if pixels.is_empty() {
        return false;
    }
    let mut darkest = 255u8;
    let mut brightest = 0u8;
    let mut channel_min = [255u8; 3];
    let mut channel_max = [0u8; 3];
    let mut visible = 0usize;
    for pixel in pixels.pixels() {
        let luminance =
            ((u32::from(pixel[0]) * 54 + u32::from(pixel[1]) * 183 + u32::from(pixel[2]) * 19)
                / 256) as u8;
        darkest = darkest.min(luminance);
        brightest = brightest.max(luminance);
        for channel in 0..3 {
            channel_min[channel] = channel_min[channel].min(pixel[channel]);
            channel_max[channel] = channel_max[channel].max(pixel[channel]);
        }
        if luminance > 12 && luminance < 244 {
            visible += 1;
        }
    }
    visible * 100 >= pixels.len() / 3
        && (brightest.saturating_sub(darkest) >= 8
            || (0..3).any(|channel| channel_max[channel] - channel_min[channel] >= 16))
}

#[cfg(windows)]
#[path = "video_cover_windows.rs"]
mod platform;
#[cfg(windows)]
use platform::extract_video_cover_frame;

#[cfg(not(windows))]
fn extract_video_cover_frame(
    _: &[u8],
    _: i32,
    _: &impl Fn() -> bool,
) -> Result<image::DynamicImage> {
    anyhow::bail!("native video cover decoder unavailable")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_cover_rejects_blank_frames_and_accepts_picture_content() {
        for color in [[0, 0, 0], [255, 255, 255], [128, 128, 128], [255, 0, 0]] {
            let frame = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
                64,
                64,
                image::Rgb(color),
            ));
            assert!(!frame_has_picture(&frame));
        }
        let frame =
            image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(64, 64, |column, row| {
                image::Rgb([column as u8 * 3, row as u8 * 3, 80])
            }));
        assert!(frame_has_picture(&frame));
        let dark = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(64, 64, |column, _| {
            image::Rgb([if column < 32 { 15 } else { 35 }; 3])
        }));
        assert!(frame_has_picture(&dark));
    }
}
