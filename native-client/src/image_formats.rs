/// Raster formats that the desktop client can decode directly.
///
/// Keep this list shared by all file pickers so selecting a file and dropping
/// the same file have identical behavior.
#[cfg(target_os = "macos")]
pub(crate) const PICKER_IMAGE_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "webp", "gif", "bmp", "tif", "tiff", "ico", "avif", "tga", "dds", "hdr",
    "exr", "qoi", "heic", "heif",
];

#[cfg(not(target_os = "macos"))]
pub(crate) const PICKER_IMAGE_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "webp", "gif", "bmp", "tif", "tiff", "ico", "avif", "tga", "dds", "hdr",
    "exr", "qoi",
];

pub(crate) fn picker_image_extensions() -> &'static [&'static str] {
    PICKER_IMAGE_EXTENSIONS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picker_includes_common_and_game_image_formats() {
        for extension in [
            "png", "jpg", "webp", "gif", "bmp", "tiff", "avif", "tga", "dds", "exr",
        ] {
            assert!(picker_image_extensions().contains(&extension));
        }
    }
}
