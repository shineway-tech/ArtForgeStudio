use super::*;

pub(super) fn viewer_item<'a>(store: &'a Store, id: &str, source: &str) -> Option<&'a AssetData> {
    match source {
        "asset" => store.assets.iter().find(|item| item.id == id),
        "inspiration" => store.inspiration.iter().find(|item| item.id == id),
        _ => store.generations.iter().find(|item| item.id == id),
    }
}

pub(super) fn copy_viewer_image(app: &AppWindow) {
    let state = app.global::<AppState>();
    let image = state.get_viewer_image();
    let Some(buffer) = image.to_rgba8() else {
        state.set_viewer_message("图片数据不可复制".into());
        return;
    };
    let data = arboard::ImageData {
        width: buffer.width() as usize,
        height: buffer.height() as usize,
        bytes: Cow::Owned(buffer.as_bytes().to_vec()),
    };
    match arboard::Clipboard::new().and_then(|mut clipboard| clipboard.set_image(data)) {
        Ok(_) => state.set_viewer_message("已复制图片".into()),
        Err(error) => state.set_viewer_message(format!("复制失败：{error}").into()),
    }
}

pub(super) fn reveal_path_in_file_manager(path: &Path) -> Result<()> {
    if !path.exists() {
        return Err(anyhow!("图片文件不存在"));
    }
    let target = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());

    #[cfg(target_os = "windows")]
    {
        let mut command = Command::new("explorer");
        if target.is_file() {
            command.arg("/select,").arg(&target);
        } else {
            command.arg(&target);
        }
        command
            .spawn()
            .with_context(|| format!("无法打开文件夹：{}", target.display()))?;
    }

    #[cfg(target_os = "macos")]
    {
        let mut command = Command::new("open");
        if target.is_file() {
            command.arg("-R").arg(&target);
        } else {
            command.arg(&target);
        }
        command
            .spawn()
            .with_context(|| format!("无法打开文件夹：{}", target.display()))?;
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let folder = if target.is_file() {
            target.parent().unwrap_or(&target)
        } else {
            target.as_path()
        };
        Command::new("xdg-open")
            .arg(folder)
            .spawn()
            .with_context(|| format!("无法打开文件夹：{}", folder.display()))?;
    }

    Ok(())
}

pub(super) fn download_viewer_image(app: &AppWindow, store: &Store) {
    let state = app.global::<AppState>();
    let id = state.get_viewer_id().to_string();
    let source = state.get_viewer_source().to_string();
    let item = viewer_item(store, &id, &source);
    let Some(item) = item else {
        state.set_viewer_message("没有可打开位置的原始文件".into());
        return;
    };
    let source_path = item.source_path.trim();
    if source_path.is_empty() || source_path == "failed" || source_path == "asset" {
        state.set_viewer_message("没有可打开位置的原始文件".into());
        return;
    }
    let source = PathBuf::from(source_path);
    match reveal_path_in_file_manager(&source) {
        Ok(_) => state.set_viewer_message("已打开图片所在文件夹".into()),
        Err(error) => state.set_viewer_message(format!("打开文件夹失败：{error}").into()),
    }
}

pub(super) fn download_asset(app: &AppWindow, store: &Rc<RefCell<Store>>, id: String) {
    let item = {
        let store_ref = store.borrow();
        store_ref
            .generations
            .iter()
            .chain(store_ref.assets.iter())
            .chain(store_ref.inspiration.iter())
            .find(|item| item.id == id)
            .cloned()
    };
    let state = app.global::<AppState>();
    let Some(item) = item else {
        state.set_generation_status("未找到图片".into());
        return;
    };
    let source_path = item.source_path.trim();
    if source_path.is_empty() || source_path == "failed" || source_path == "asset" {
        state.set_generation_status("没有可打开位置的原始文件".into());
        return;
    }
    let source = PathBuf::from(source_path);
    match reveal_path_in_file_manager(&source) {
        Ok(_) => state.set_generation_status("已打开图片所在文件夹".into()),
        Err(error) => state.set_generation_status(format!("打开文件夹失败：{error}").into()),
    }
}

#[derive(Clone, Copy)]
pub(super) enum ProcessImageMode {
    Cutout,
    RemoveBlack,
    Upscale { scale: u32, target_long_edge: u32 },
}

pub(super) fn start_viewer_image_processing(
    app: &AppWindow,
    store: Rc<RefCell<Store>>,
    mode: ProcessImageMode,
) {
    let state = app.global::<AppState>();
    if state.get_viewer_processing() {
        return;
    }
    let already_done = match mode {
        ProcessImageMode::Cutout => state.get_viewer_cutout_done(),
        ProcessImageMode::RemoveBlack => state.get_viewer_remove_black_done(),
        ProcessImageMode::Upscale { .. } => state.get_viewer_upscale_done(),
    };
    if already_done {
        state.set_viewer_message(processing_done_message(app, mode).into());
        return;
    }
    state.set_viewer_processing(true);
    state.set_viewer_processing_progress(0);
    state.set_viewer_processing_label(processing_label(app, mode).into());
    let duration_ms = viewer_processing_duration_ms(mode);
    for (delay_percent, progress) in [
        (10u64, 12),
        (24, 28),
        (40, 46),
        (60, 64),
        (78, 82),
        (94, 94),
    ] {
        let delay = duration_ms.saturating_mul(delay_percent) / 100;
        schedule_viewer_processing_progress(app.as_weak(), delay, progress);
    }
    let app_weak = app.as_weak();
    slint::Timer::single_shot(Duration::from_millis(duration_ms), move || {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        if process_viewer_image(&app, &store, mode) {
            let state = app.global::<AppState>();
            state.set_viewer_processing_progress(100);
            let app_weak = app.as_weak();
            slint::Timer::single_shot(Duration::from_millis(180), move || {
                let Some(app) = app_weak.upgrade() else {
                    return;
                };
                let state = app.global::<AppState>();
                state.set_viewer_processing(false);
                state.set_upscale_open(false);
                state.set_viewer_open(false);
                navigate_to(&app, "generation");
            });
        } else {
            let state = app.global::<AppState>();
            state.set_viewer_processing(false);
        }
    });
}

pub(super) fn viewer_processing_duration_ms(mode: ProcessImageMode) -> u64 {
    match mode {
        ProcessImageMode::Cutout | ProcessImageMode::RemoveBlack => 3000,
        ProcessImageMode::Upscale { .. } => 560,
    }
}

pub(super) fn schedule_viewer_processing_progress(app: Weak<AppWindow>, delay_ms: u64, progress: i32) {
    slint::Timer::single_shot(Duration::from_millis(delay_ms), move || {
        let Some(app) = app.upgrade() else {
            return;
        };
        let state = app.global::<AppState>();
        if state.get_viewer_processing() && state.get_viewer_processing_progress() < progress {
            state.set_viewer_processing_progress(progress);
        }
    });
}

pub(super) fn processing_label(app: &AppWindow, mode: ProcessImageMode) -> &'static str {
    let en = app.global::<AppState>().get_language().as_str() == "en";
    match mode {
        ProcessImageMode::Cutout => {
            if en {
                "Cutting out"
            } else {
                "正在抠图"
            }
        }
        ProcessImageMode::RemoveBlack => {
            if en {
                "Removing black"
            } else {
                "正在去黑"
            }
        }
        ProcessImageMode::Upscale { .. } => {
            if en {
                "Upscaling"
            } else {
                "正在放大"
            }
        }
    }
}

pub(super) fn processing_done_message(app: &AppWindow, mode: ProcessImageMode) -> &'static str {
    let en = app.global::<AppState>().get_language().as_str() == "en";
    match mode {
        ProcessImageMode::Cutout => {
            if en {
                "This image has already been cut out."
            } else {
                "当前图片已抠图"
            }
        }
        ProcessImageMode::RemoveBlack => {
            if en {
                "Black has already been removed from this image."
            } else {
                "当前图片已去黑"
            }
        }
        ProcessImageMode::Upscale { .. } => {
            if en {
                "This image has already been upscaled."
            } else {
                "当前图片已清晰放大"
            }
        }
    }
}

pub(super) fn upscale_quality_long_edge(quality: &str) -> u32 {
    if quality.eq_ignore_ascii_case("4K") {
        4096
    } else {
        2048
    }
}

pub(super) fn process_viewer_image(
    app: &AppWindow,
    store: &Rc<RefCell<Store>>,
    mode: ProcessImageMode,
) -> bool {
    let state = app.global::<AppState>();
    let Some(buffer) = state.get_viewer_image().to_rgba8() else {
        state.set_viewer_message("图片数据不可处理".into());
        return false;
    };
    let mut width = buffer.width();
    let mut height = buffer.height();
    if width == 0 || height == 0 {
        state.set_viewer_message("图片数据不可处理".into());
        return false;
    }
    let mut rgba = buffer.as_bytes().to_vec();
    match mode {
        ProcessImageMode::Cutout => cutout_edge_background(&mut rgba, width, height),
        ProcessImageMode::RemoveBlack => remove_black_pixels(&mut rgba),
        ProcessImageMode::Upscale {
            scale,
            target_long_edge,
        } => {
            let Some(source) = image::RgbaImage::from_raw(width, height, rgba) else {
                state.set_viewer_message("图片数据不可处理".into());
                return false;
            };
            let (target_width, target_height) =
                upscale_dimensions(width, height, scale, target_long_edge);
            let resized = image::imageops::resize(
                &source,
                target_width,
                target_height,
                image::imageops::FilterType::Lanczos3,
            );
            width = target_width;
            height = target_height;
            rgba = resized.into_raw();
        }
    }
    if let Err(error) = save_processed_viewer_image(app, store, rgba, width, height, mode) {
        state.set_viewer_message(format!("处理失败：{error}").into());
        return false;
    }
    true
}

pub(super) fn upscale_dimensions(width: u32, height: u32, scale: u32, target_long_edge: u32) -> (u32, u32) {
    let scale = scale.clamp(2, 4) as u64;
    let scaled_width = (width as u64).saturating_mul(scale);
    let scaled_height = (height as u64).saturating_mul(scale);
    let scaled_long = scaled_width.max(scaled_height).max(1);
    let original_long = width.max(height) as u64;
    let target_long = (target_long_edge as u64).max(original_long).min(8192);
    if scaled_long <= target_long {
        return (
            scaled_width.min(8192).max(1) as u32,
            scaled_height.min(8192).max(1) as u32,
        );
    }
    let target_width = (scaled_width.saturating_mul(target_long) / scaled_long).max(1);
    let target_height = (scaled_height.saturating_mul(target_long) / scaled_long).max(1);
    (target_width as u32, target_height as u32)
}

pub(super) fn cutout_edge_background(rgba: &mut [u8], width: u32, height: u32) {
    let corners = [
        (0, 0),
        (width.saturating_sub(1), 0),
        (0, height.saturating_sub(1)),
        (width.saturating_sub(1), height.saturating_sub(1)),
    ];
    let mut bg = [0u32; 3];
    for &(x, y) in &corners {
        let idx = pixel_index(width, x, y);
        bg[0] += rgba[idx] as u32;
        bg[1] += rgba[idx + 1] as u32;
        bg[2] += rgba[idx + 2] as u32;
    }
    let bg = [
        (bg[0] / corners.len() as u32) as u8,
        (bg[1] / corners.len() as u32) as u8,
        (bg[2] / corners.len() as u32) as u8,
    ];
    let mut visited = vec![false; (width as usize).saturating_mul(height as usize)];
    let mut queue = Vec::new();
    for x in 0..width {
        enqueue_background_pixel(rgba, width, height, x, 0, bg, &mut visited, &mut queue);
        enqueue_background_pixel(
            rgba,
            width,
            height,
            x,
            height.saturating_sub(1),
            bg,
            &mut visited,
            &mut queue,
        );
    }
    for y in 0..height {
        enqueue_background_pixel(rgba, width, height, 0, y, bg, &mut visited, &mut queue);
        enqueue_background_pixel(
            rgba,
            width,
            height,
            width.saturating_sub(1),
            y,
            bg,
            &mut visited,
            &mut queue,
        );
    }

    let mut cursor = 0usize;
    while cursor < queue.len() {
        let (x, y) = queue[cursor];
        cursor += 1;
        if x > 0 {
            enqueue_background_pixel(rgba, width, height, x - 1, y, bg, &mut visited, &mut queue);
        }
        if x + 1 < width {
            enqueue_background_pixel(rgba, width, height, x + 1, y, bg, &mut visited, &mut queue);
        }
        if y > 0 {
            enqueue_background_pixel(rgba, width, height, x, y - 1, bg, &mut visited, &mut queue);
        }
        if y + 1 < height {
            enqueue_background_pixel(rgba, width, height, x, y + 1, bg, &mut visited, &mut queue);
        }
    }

    for y in 0..height {
        for x in 0..width {
            let flat = (y * width + x) as usize;
            if visited[flat] {
                rgba[pixel_index(width, x, y) + 3] = 0;
            }
        }
    }
}

pub(super) fn enqueue_background_pixel(
    rgba: &[u8],
    width: u32,
    height: u32,
    x: u32,
    y: u32,
    bg: [u8; 3],
    visited: &mut [bool],
    queue: &mut Vec<(u32, u32)>,
) {
    if x >= width || y >= height {
        return;
    }
    let flat = (y * width + x) as usize;
    if visited[flat] {
        return;
    }
    let idx = pixel_index(width, x, y);
    if color_distance_sq([rgba[idx], rgba[idx + 1], rgba[idx + 2]], bg) > 55 * 55 {
        return;
    }
    visited[flat] = true;
    queue.push((x, y));
}

pub(super) fn remove_black_pixels(rgba: &mut [u8]) {
    for pixel in rgba.chunks_exact_mut(4) {
        let luma =
            (54u32 * pixel[0] as u32 + 183u32 * pixel[1] as u32 + 19u32 * pixel[2] as u32) / 256;
        if luma <= 34 {
            pixel[3] = 0;
        } else if luma < 84 {
            let scale = (luma - 34) as f32 / 50.0;
            pixel[3] = (pixel[3] as f32 * scale) as u8;
        }
    }
}

pub(super) fn pixel_index(width: u32, x: u32, y: u32) -> usize {
    ((y * width + x) * 4) as usize
}

pub(super) fn color_distance_sq(a: [u8; 3], b: [u8; 3]) -> i32 {
    let dr = a[0] as i32 - b[0] as i32;
    let dg = a[1] as i32 - b[1] as i32;
    let db = a[2] as i32 - b[2] as i32;
    dr * dr + dg * dg + db * db
}

pub(super) fn save_processed_viewer_image(
    app: &AppWindow,
    store: &Rc<RefCell<Store>>,
    rgba: Vec<u8>,
    width: u32,
    height: u32,
    mode: ProcessImageMode,
) -> Result<()> {
    let state = app.global::<AppState>();
    let (suffix, title_suffix) = match mode {
        ProcessImageMode::Cutout => ("cutout", "抠图"),
        ProcessImageMode::RemoveBlack => ("remove-black", "去黑"),
        ProcessImageMode::Upscale { scale, .. } => {
            if scale >= 4 {
                ("upscale-4x", "清晰放大4X")
            } else if scale == 3 {
                ("upscale-3x", "清晰放大3X")
            } else {
                ("upscale-2x", "清晰放大2X")
            }
        }
    };
    let image_buffer = image::RgbaImage::from_raw(width, height, rgba.clone())
        .ok_or_else(|| anyhow!("invalid image buffer"))?;
    let bytes = encode_png_rgba(&image_buffer, width, height)?;
    let dir = output_dir_path(app);
    fs::create_dir_all(&dir)?;
    let stem = sanitize_filename(&format!("{}-{}", state.get_viewer_title(), title_suffix));
    let path = unique_path(dir.join(format!(
        "{}-{}-{}.png",
        Local::now().format("%Y%m%d%H%M%S%3f"),
        stem,
        suffix
    )));
    fs::write(&path, bytes)?;

    let buffer =
        slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&rgba, width, height);
    let image = Image::from_rgba8(buffer);
    let source = state.get_viewer_source().to_string();
    let id = state.get_viewer_id().to_string();
    let original = {
        let store_ref = store.borrow();
        viewer_item(&store_ref, &id, &source).cloned()
    };
    let category = original
        .as_ref()
        .map(|item| item.category.clone())
        .unwrap_or_else(|| resolve_category(&state.get_asset_type().to_string(), ""));
    let kind = original
        .as_ref()
        .map(|item| item.kind.clone())
        .unwrap_or_else(|| state.get_mode().to_string());
    let prompt = original
        .as_ref()
        .map(|item| item.prompt.clone())
        .unwrap_or_else(|| state.get_viewer_prompt().to_string());
    let quality = match mode {
        ProcessImageMode::Upscale {
            target_long_edge, ..
        } => {
            if target_long_edge >= 4096 {
                "4K".to_string()
            } else {
                "2K".to_string()
            }
        }
        _ => original
            .as_ref()
            .map(|item| item.quality.clone())
            .unwrap_or_else(|| state.get_viewer_quality().to_string()),
    };
    let base_cutout_done = original
        .as_ref()
        .map(|item| item.cutout_done)
        .unwrap_or_else(|| state.get_viewer_cutout_done());
    let base_remove_black_done = original
        .as_ref()
        .map(|item| item.remove_black_done)
        .unwrap_or_else(|| state.get_viewer_remove_black_done());
    let base_upscale_done = original
        .as_ref()
        .map(|item| item.upscale_done)
        .unwrap_or_else(|| state.get_viewer_upscale_done());
    let item = AssetData {
        id: Uuid::new_v4().to_string(),
        conversation_id: original
            .as_ref()
            .map(|item| item.conversation_id.clone())
            .unwrap_or_default(),
        title: format!(
            "{} {}",
            original
                .as_ref()
                .map(|item| item.title.clone())
                .unwrap_or_else(|| state.get_viewer_title().to_string()),
            title_suffix
        ),
        category,
        kind,
        time: Local::now().format("%Y-%m-%d %H:%M").to_string(),
        prompt,
        ratio: ratio_from_actual_dimensions(width as i32, height as i32),
        quality,
        model: match mode {
            ProcessImageMode::Upscale { .. } => "本地清晰放大".to_string(),
            _ => "本地处理".to_string(),
        },
        width: width as i32,
        height: height as i32,
        image,
        source_path: path.display().to_string(),
        cutout_done: base_cutout_done || matches!(mode, ProcessImageMode::Cutout),
        remove_black_done: base_remove_black_done || matches!(mode, ProcessImageMode::RemoveBlack),
        upscale_done: base_upscale_done || matches!(mode, ProcessImageMode::Upscale { .. }),
    };
    {
        let mut store_mut = store.borrow_mut();
        store_mut.assets.insert(0, item.clone());
        store_mut.generations.insert(0, item.clone());
        save_local_store(app, &store_mut);
        push_all(app, &store_mut);
    }
    Ok(())
}
