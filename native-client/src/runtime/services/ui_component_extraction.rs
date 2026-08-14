use super::*;

const MAX_ANALYSIS_EDGE: u32 = 1_024;
const MAX_EXTRACTED_COMPONENTS: usize = 24;

pub(super) fn canvas_ui_extraction_prompt() -> String {
    "严格以参考图为唯一内容来源，提取其中全部可复用的游戏 UI 组件。先生成一张组件汇总图：保持原图的视觉风格、颜色、材质、描边和细节，不重新设计，不生成完整游戏场景。把头像框、血条、按钮、图标、摇杆、物品格、技能、货币、设置、小地图、宝箱、面板等独立元素完整分离，正面展示，互不重叠，按从左到右、从上到下排列在纯白或透明背景上。元素之间保留宽阔且一致的空白间距；不要文字说明、序号、水印、装饰性背景、阴影连接或裁切元素。".to_string()
}

#[derive(Debug)]
pub(super) struct ExtractedUiComponent {
    pub(super) image: image::RgbaImage,
}

#[derive(Clone, Copy, Debug)]
struct ComponentBounds {
    left: u32,
    top: u32,
    right: u32,
    bottom: u32,
    pixels: u32,
}

pub(super) fn extract_ui_components(
    source: &image::RgbaImage,
) -> Result<Vec<ExtractedUiComponent>> {
    if source.width() < 32 || source.height() < 32 {
        return Err(anyhow!("组件汇总图尺寸过小"));
    }

    let scale = (MAX_ANALYSIS_EDGE as f32 / source.width().max(source.height()) as f32).min(1.0);
    let analysis_width = ((source.width() as f32 * scale).round() as u32).max(1);
    let analysis_height = ((source.height() as f32 * scale).round() as u32).max(1);
    let analysis = if analysis_width == source.width() && analysis_height == source.height() {
        source.clone()
    } else {
        image::imageops::resize(
            source,
            analysis_width,
            analysis_height,
            image::imageops::FilterType::Triangle,
        )
    };
    let background = estimated_border_color(&analysis);
    let mut mask = analysis
        .pixels()
        .map(|pixel| is_foreground(*pixel, background))
        .collect::<Vec<_>>();
    let bridge_radius = (analysis_width.max(analysis_height) / 220).clamp(2, 6);
    mask = dilate_mask(&mask, analysis_width, analysis_height, bridge_radius);
    let minimum_pixels = (analysis_width * analysis_height / 3_200).max(40);
    let mut bounds =
        connected_component_bounds(&mask, analysis_width, analysis_height, minimum_pixels);
    bounds.retain(|item| {
        let width = item.right - item.left + 1;
        let height = item.bottom - item.top + 1;
        width >= 10
            && height >= 10
            && !(width > analysis_width * 94 / 100 && height > analysis_height * 94 / 100)
    });
    sort_component_bounds_reading_order(&mut bounds);
    bounds.truncate(MAX_EXTRACTED_COMPONENTS);
    if bounds.len() < 2 {
        return Err(anyhow!("未识别到可拆分的独立 UI 元素"));
    }

    let inverse_scale = 1.0 / scale;
    bounds
        .into_iter()
        .map(|bounds| {
            let padding = (10.0 * inverse_scale).round() as u32;
            let left =
                ((bounds.left as f32 * inverse_scale).floor() as u32).saturating_sub(padding);
            let top = ((bounds.top as f32 * inverse_scale).floor() as u32).saturating_sub(padding);
            let right = (((bounds.right + 1) as f32 * inverse_scale).ceil() as u32 + padding)
                .min(source.width());
            let bottom = (((bounds.bottom + 1) as f32 * inverse_scale).ceil() as u32 + padding)
                .min(source.height());
            let mut crop = image::imageops::crop_imm(
                source,
                left,
                top,
                right.saturating_sub(left).max(1),
                bottom.saturating_sub(top).max(1),
            )
            .to_image();
            clear_edge_connected_background(&mut crop, background);
            Ok(ExtractedUiComponent { image: crop })
        })
        .collect()
}

fn sort_component_bounds_reading_order(bounds: &mut Vec<ComponentBounds>) {
    bounds.sort_by_key(|item| (item.top, item.left, item.bottom, item.right));

    let mut rows = Vec::<Vec<ComponentBounds>>::new();
    for item in bounds.drain(..) {
        let belongs_to_last_row = rows.last().is_some_and(|row| {
            let anchor = row[0];
            let anchor_height = anchor.bottom.saturating_sub(anchor.top) + 1;
            let item_height = item.bottom.saturating_sub(item.top) + 1;
            let row_tolerance = anchor_height.min(item_height) / 2;
            item.top.abs_diff(anchor.top) <= row_tolerance.max(1)
        });

        if belongs_to_last_row {
            rows.last_mut().expect("row exists").push(item);
        } else {
            rows.push(vec![item]);
        }
    }

    for mut row in rows {
        row.sort_by_key(|item| (item.left, item.top, item.bottom, item.right));
        bounds.extend(row);
    }
}

fn estimated_border_color(image: &image::RgbaImage) -> image::Rgba<u8> {
    let mut samples = Vec::new();
    let steps = 48_u32;
    for index in 0..steps {
        let x = index * image.width().saturating_sub(1) / steps.saturating_sub(1);
        let y = index * image.height().saturating_sub(1) / steps.saturating_sub(1);
        samples.push(*image.get_pixel(x, 0));
        samples.push(*image.get_pixel(x, image.height() - 1));
        samples.push(*image.get_pixel(0, y));
        samples.push(*image.get_pixel(image.width() - 1, y));
    }
    let mut result = [0_u8; 4];
    for channel in 0..4 {
        let mut values = samples
            .iter()
            .map(|pixel| pixel[channel])
            .collect::<Vec<_>>();
        values.sort_unstable();
        result[channel] = values[values.len() / 2];
    }
    image::Rgba(result)
}

fn is_foreground(pixel: image::Rgba<u8>, background: image::Rgba<u8>) -> bool {
    if pixel[3] <= 18 {
        return false;
    }
    if background[3] <= 40 {
        return true;
    }
    let color_distance = (0..3)
        .map(|channel| {
            let delta = pixel[channel] as i32 - background[channel] as i32;
            delta * delta
        })
        .sum::<i32>();
    color_distance > 24 * 24 || pixel[3].abs_diff(background[3]) > 28
}

fn dilate_mask(mask: &[bool], width: u32, height: u32, radius: u32) -> Vec<bool> {
    let mut horizontal = vec![false; mask.len()];
    for y in 0..height {
        let mut prefix = vec![0_u32; width as usize + 1];
        for x in 0..width {
            prefix[x as usize + 1] =
                prefix[x as usize] + u32::from(mask[(y * width + x) as usize]);
        }
        for x in 0..width {
            let start = x.saturating_sub(radius) as usize;
            let end = (x + radius + 1).min(width) as usize;
            horizontal[(y * width + x) as usize] = prefix[end] > prefix[start];
        }
    }
    let mut result = vec![false; mask.len()];
    for x in 0..width {
        let mut prefix = vec![0_u32; height as usize + 1];
        for y in 0..height {
            prefix[y as usize + 1] =
                prefix[y as usize] + u32::from(horizontal[(y * width + x) as usize]);
        }
        for y in 0..height {
            let start = y.saturating_sub(radius) as usize;
            let end = (y + radius + 1).min(height) as usize;
            result[(y * width + x) as usize] = prefix[end] > prefix[start];
        }
    }
    result
}

fn connected_component_bounds(
    mask: &[bool],
    width: u32,
    height: u32,
    minimum_pixels: u32,
) -> Vec<ComponentBounds> {
    let mut visited = vec![false; mask.len()];
    let mut results = Vec::new();
    for start in 0..mask.len() {
        if !mask[start] || visited[start] {
            continue;
        }
        visited[start] = true;
        let mut queue = std::collections::VecDeque::from([start]);
        let start_x = start as u32 % width;
        let start_y = start as u32 / width;
        let mut bounds = ComponentBounds {
            left: start_x,
            top: start_y,
            right: start_x,
            bottom: start_y,
            pixels: 0,
        };
        while let Some(index) = queue.pop_front() {
            let x = index as u32 % width;
            let y = index as u32 / width;
            bounds.left = bounds.left.min(x);
            bounds.top = bounds.top.min(y);
            bounds.right = bounds.right.max(x);
            bounds.bottom = bounds.bottom.max(y);
            bounds.pixels += 1;
            for (next_x, next_y) in [
                (x.wrapping_sub(1), y),
                (x + 1, y),
                (x, y.wrapping_sub(1)),
                (x, y + 1),
            ] {
                if next_x >= width || next_y >= height {
                    continue;
                }
                let next = (next_y * width + next_x) as usize;
                if mask[next] && !visited[next] {
                    visited[next] = true;
                    queue.push_back(next);
                }
            }
        }
        if bounds.pixels >= minimum_pixels {
            results.push(bounds);
        }
    }
    results
}

fn clear_edge_connected_background(crop: &mut image::RgbaImage, background: image::Rgba<u8>) {
    let width = crop.width();
    let height = crop.height();
    let mut visited = vec![false; (width * height) as usize];
    let mut queue = std::collections::VecDeque::new();
    for x in 0..width {
        queue.push_back((x, 0));
        queue.push_back((x, height - 1));
    }
    for y in 0..height {
        queue.push_back((0, y));
        queue.push_back((width - 1, y));
    }
    while let Some((x, y)) = queue.pop_front() {
        let index = (y * width + x) as usize;
        if visited[index] || is_foreground(*crop.get_pixel(x, y), background) {
            continue;
        }
        visited[index] = true;
        crop.get_pixel_mut(x, y)[3] = 0;
        for (next_x, next_y) in [
            (x.wrapping_sub(1), y),
            (x + 1, y),
            (x, y.wrapping_sub(1)),
            (x, y + 1),
        ] {
            if next_x < width && next_y < height {
                queue.push_back((next_x, next_y));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn separates_isolated_ui_components_from_a_flat_background() {
        let mut atlas = image::RgbaImage::from_pixel(480, 360, image::Rgba([248, 246, 240, 255]));
        for (left, top, right, bottom, color) in [
            (30, 35, 155, 130, [32, 74, 180, 255]),
            (280, 30, 430, 120, [204, 62, 72, 255]),
            (45, 225, 175, 325, [56, 164, 92, 255]),
            (290, 210, 440, 330, [128, 68, 184, 255]),
        ] {
            for y in top..bottom {
                for x in left..right {
                    atlas.put_pixel(x, y, image::Rgba(color));
                }
            }
        }

        let components = extract_ui_components(&atlas).expect("extract components");

        assert_eq!(components.len(), 4);
        assert!(components
            .iter()
            .all(|component| { component.image.width() >= 120 && component.image.height() >= 85 }));
        assert!(components
            .iter()
            .all(|component| { component.image.pixels().any(|pixel| pixel[3] == 0) }));
    }

    #[test]
    fn reading_order_sort_handles_overlapping_row_tolerances() {
        let mut bounds = vec![
            ComponentBounds {
                left: 0,
                top: 10,
                right: 9,
                bottom: 19,
                pixels: 100,
            },
            ComponentBounds {
                left: 10,
                top: 5,
                right: 19,
                bottom: 14,
                pixels: 100,
            },
            ComponentBounds {
                left: 20,
                top: 0,
                right: 29,
                bottom: 9,
                pixels: 100,
            },
        ];

        sort_component_bounds_reading_order(&mut bounds);

        assert_eq!(
            bounds
                .iter()
                .map(|item| (item.top, item.left))
                .collect::<Vec<_>>(),
            vec![(5, 10), (0, 20), (10, 0)]
        );
    }
}
