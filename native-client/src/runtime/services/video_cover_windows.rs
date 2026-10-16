use super::*;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};
use windows::Win32::UI::Shell::SHCreateMemStream;

struct DecoderSession;
impl Drop for DecoderSession {
    fn drop(&mut self) {
        unsafe {
            let _ = MFShutdown();
            CoUninitialize();
        }
    }
}

pub(super) fn extract_video_cover_frame(
    bytes: &[u8],
    duration_secs: i32,
    should_continue: &impl Fn() -> bool,
) -> Result<image::DynamicImage> {
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
        if let Err(error) = MFStartup(MF_VERSION, MFSTARTUP_FULL) {
            CoUninitialize();
            return Err(error.into());
        }
        let _session = DecoderSession;
        let stream =
            SHCreateMemStream(Some(bytes)).ok_or_else(|| anyhow!("video stream unavailable"))?;
        let byte_stream = MFCreateMFByteStreamOnStream(&stream)?;
        let mut attributes = None;
        MFCreateAttributes(&mut attributes, 1)?;
        let attributes = attributes.unwrap();
        attributes.SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1)?;
        let reader = MFCreateSourceReaderFromByteStream(&byte_stream, &attributes)?;
        let video_stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
        reader.SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)?;
        reader.SetStreamSelection(video_stream, true)?;
        let format = MFCreateMediaType()?;
        format.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        format.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
        reader.SetCurrentMediaType(video_stream, None, &format)?;
        let duration = reader
            .GetPresentationAttribute(MF_SOURCE_READER_MEDIASOURCE.0 as u32, &MF_PD_DURATION)
            .ok()
            .and_then(|value| u64::try_from(&value).ok())
            .filter(|value| *value > 0)
            .map(|value| value as f64 / 10_000_000.0)
            .unwrap_or(f64::from(duration_secs.max(1)));
        let positions = [
            0.0,
            0.5,
            duration * 0.25,
            duration * 0.5,
            duration * 0.75,
            duration * 0.9,
        ];
        let deadline = Instant::now() + Duration::from_secs(20);
        for position in positions {
            anyhow::ensure!(
                should_continue() && Instant::now() < deadline,
                "video cover cancelled"
            );
            let target = (position * 10_000_000.0) as i64;
            let position = PROPVARIANT::from(target);
            if reader
                .SetCurrentPosition(&windows::core::GUID::zeroed(), &position)
                .is_err()
            {
                continue;
            }
            for _attempt in 0..600 {
                anyhow::ensure!(
                    should_continue() && Instant::now() < deadline,
                    "video cover cancelled"
                );
                let mut flags = 0;
                let mut sample = None;
                let mut timestamp = 0;
                reader.ReadSample(
                    video_stream,
                    0,
                    None,
                    Some(&mut flags),
                    Some(&mut timestamp),
                    Some(&mut sample),
                )?;
                if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                    break;
                }
                let Some(sample) = sample else {
                    continue;
                };
                if timestamp < target {
                    continue;
                }
                let format = reader.GetCurrentMediaType(video_stream)?;
                let size = format.GetUINT64(&MF_MT_FRAME_SIZE)?;
                let (width, height) = ((size >> 32) as u32, size as u32);
                anyhow::ensure!(
                    width > 0 && height > 0 && u64::from(width) * u64::from(height) <= 50_000_000,
                    "video cover dimensions exceed policy"
                );
                let stride = format
                    .GetUINT32(&MF_MT_DEFAULT_STRIDE)
                    .map(|value| value as i32)
                    .unwrap_or(width as i32 * 4);
                let buffer = sample.ConvertToContiguousBuffer()?;
                let mut pointer = std::ptr::null_mut();
                let mut length = 0;
                buffer.Lock(&mut pointer, None, Some(&mut length))?;
                let frame = if pointer.is_null() {
                    Err(anyhow!("video frame buffer unavailable"))
                } else {
                    decode_bgra_frame(
                        std::slice::from_raw_parts(pointer, length as usize),
                        width,
                        height,
                        stride,
                    )
                };
                buffer.Unlock()?;
                let frame = frame?;
                if frame_has_picture(&frame) {
                    return Ok(frame);
                }
                break;
            }
        }
        anyhow::bail!("video has no usable cover frame")
    }
}

fn decode_bgra_frame(
    bytes: &[u8],
    width: u32,
    height: u32,
    stride: i32,
) -> Result<image::DynamicImage> {
    let pitch = stride.unsigned_abs() as usize;
    anyhow::ensure!(
        pitch >= width as usize * 4
            && pitch
                .checked_mul(height as usize)
                .is_some_and(|size| size <= bytes.len()),
        "video frame buffer is truncated"
    );
    let image = image::RgbImage::from_fn(width, height, |column, row| {
        let source_row = if stride < 0 { height - 1 - row } else { row };
        let offset = source_row as usize * pitch + column as usize * 4;
        image::Rgb([bytes[offset + 2], bytes[offset + 1], bytes[offset]])
    });
    Ok(image::DynamicImage::ImageRgb8(image))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_cover_decodes_bgra_with_padding_and_bottom_up_rows() {
        let bytes = [0, 0, 255, 0, 9, 9, 9, 9, 255, 0, 0, 0, 9, 9, 9, 9];
        let frame = decode_bgra_frame(&bytes, 1, 2, -8).unwrap().to_rgb8();
        assert_eq!(frame.get_pixel(0, 0).0, [0, 0, 255]);
        assert_eq!(frame.get_pixel(0, 1).0, [255, 0, 0]);
        assert!(decode_bgra_frame(&bytes[..7], 1, 2, 8).is_err());
    }

    #[test]
    fn video_cover_extracts_picture_after_black_intro() {
        let bytes = include_bytes!("../../../tests/fixtures/video-cover-black-intro.mp4");
        let frame = extract_video_cover_frame(bytes, 3, &|| true).unwrap();
        assert_eq!((frame.width(), frame.height()), (64, 48));
        assert!(frame_has_picture(&frame));
        assert!(extract_video_cover_frame(bytes, 3, &|| false).is_err());
        assert!(extract_video_cover_frame(b"invalid video", 3, &|| true).is_err());
    }
}
