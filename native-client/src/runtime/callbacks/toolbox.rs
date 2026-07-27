use super::*;

pub(super) fn wire_toolbox_callbacks(app: &AppWindow) {
    let state = app.global::<AppState>();

    {
        let app_weak = app.as_weak();
        state.on_choose_enhance_source(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(path) = rfd::FileDialog::new()
                .add_filter("Images", &["png", "jpg", "jpeg", "webp"])
                .pick_file()
            else {
                return;
            };
            let state = app.global::<AppState>();
            match load_image(&path) {
                Ok(image) => {
                    let name = path
                        .file_name()
                        .and_then(|value| value.to_str())
                        .unwrap_or_default()
                        .to_string();
                    state.set_enhance_source_path(path.display().to_string().into());
                    state.set_enhance_source_name(name.into());
                    state.set_enhance_source_image(image);
                    state.set_enhance_result_path("".into());
                    state.set_enhance_result_name("".into());
                    state.set_enhance_result_image(Image::default());
                    state.set_enhance_processing(false);
                    state.set_enhance_message("".into());
                }
                Err(_) => state.set_enhance_message(
                    if state.get_language().as_str() == "en" {
                        "The selected file is not a supported image"
                    } else {
                        "所选文件不是受支持的图片"
                    }
                    .into(),
                ),
            }
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_start_enhance(move |quality| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            if state.get_enhance_source_path().trim().is_empty() {
                state.set_enhance_message(
                    if state.get_language().as_str() == "en" {
                        "Upload an image first"
                    } else {
                        "请先上传图片"
                    }
                    .into(),
                );
                return;
            }
            let quality = quality.to_ascii_uppercase();
            if !matches!(quality.as_str(), "1K" | "2K" | "4K") {
                state.set_enhance_message(
                    if state.get_language().as_str() == "en" {
                        "Choose a valid quality"
                    } else {
                        "请选择有效的清晰度"
                    }
                    .into(),
                );
                return;
            }
            state.set_enhance_quality(quality.into());
            state.set_enhance_processing(false);
            state.set_enhance_message(
                if state.get_language().as_str() == "en" {
                    "Image enhancement is waiting for backend configuration"
                } else {
                    "图片变清晰能力等待后端配置"
                }
                .into(),
            );
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_reveal_enhance_result(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            let path = PathBuf::from(state.get_enhance_result_path().to_string());
            if !path.is_file() {
                state.set_enhance_message(
                    if state.get_language().as_str() == "en" {
                        "No enhanced image is available yet"
                    } else {
                        "暂无可查看的清晰处理结果"
                    }
                    .into(),
                );
                return;
            }
            match reveal_path_in_file_manager(&path) {
                Ok(_) => state.set_enhance_message(
                    if state.get_language().as_str() == "en" {
                        "Opened the image folder"
                    } else {
                        "已打开图片所在文件夹"
                    }
                    .into(),
                ),
                Err(error) => state.set_enhance_message(
                    if state.get_language().as_str() == "en" {
                        format!("Failed to open the image folder: {error}")
                    } else {
                        format!("打开图片所在文件夹失败：{error}")
                    }
                    .into(),
                ),
            }
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_choose_watermark_source(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(path) = rfd::FileDialog::new()
                .add_filter("Images", &["png", "jpg", "jpeg", "webp"])
                .pick_file()
            else {
                return;
            };
            let state = app.global::<AppState>();
            match load_image(&path) {
                Ok(image) => {
                    let name = path
                        .file_name()
                        .and_then(|value| value.to_str())
                        .unwrap_or_default()
                        .to_string();
                    state.set_watermark_source_path(path.display().to_string().into());
                    state.set_watermark_source_name(name.into());
                    state.set_watermark_source_image(image);
                    state.set_watermark_result_path("".into());
                    state.set_watermark_result_name("".into());
                    state.set_watermark_result_image(Image::default());
                    state.set_watermark_processing(false);
                    state.set_watermark_progress(0);
                    state.set_watermark_message("".into());
                }
                Err(_) => state.set_watermark_message(
                    if state.get_language().as_str() == "en" {
                        "The selected file is not a supported image"
                    } else {
                        "所选文件不是受支持的图片"
                    }
                    .into(),
                ),
            }
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_start_watermark_removal(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            if state.get_watermark_source_path().trim().is_empty() {
                state.set_watermark_message(
                    if state.get_language().as_str() == "en" {
                        "Upload an image first"
                    } else {
                        "请先上传图片"
                    }
                    .into(),
                );
                return;
            }
            state.set_watermark_processing(false);
            state.set_watermark_progress(0);
            state.set_watermark_message(
                if state.get_language().as_str() == "en" {
                    "The watermark-removal service is waiting for backend configuration"
                } else {
                    "去水印服务等待后端配置，接口开放后将在右侧显示处理结果"
                }
                .into(),
            );
        });
    }

    {
        let app_weak = app.as_weak();
        state.on_reveal_watermark_result(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let state = app.global::<AppState>();
            let path = PathBuf::from(state.get_watermark_result_path().to_string());
            if !path.is_file() {
                state.set_watermark_message(
                    if state.get_language().as_str() == "en" {
                        "No processed image is available yet"
                    } else {
                        "暂无可查看的处理结果"
                    }
                    .into(),
                );
                return;
            }
            match reveal_path_in_file_manager(&path) {
                Ok(_) => state.set_watermark_message(
                    if state.get_language().as_str() == "en" {
                        "Opened the image folder"
                    } else {
                        "已打开图片所在文件夹"
                    }
                    .into(),
                ),
                Err(error) => state.set_watermark_message(
                    if state.get_language().as_str() == "en" {
                        format!("Failed to open the image folder: {error}")
                    } else {
                        format!("打开图片所在文件夹失败：{error}")
                    }
                    .into(),
                ),
            }
        });
    }
}
