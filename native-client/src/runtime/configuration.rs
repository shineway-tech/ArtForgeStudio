use super::*;

pub(super) fn supported_ratios() -> &'static [(&'static str, i32, i32)] {
    &[
        ("1:1", 1, 1),
        ("3:2", 3, 2),
        ("2:3", 2, 3),
        ("4:3", 4, 3),
        ("3:4", 3, 4),
        ("5:4", 5, 4),
        ("4:5", 4, 5),
        ("16:9", 16, 9),
        ("9:16", 9, 16),
        ("2:1", 2, 1),
        ("1:2", 1, 2),
        ("21:9", 21, 9),
        ("9:21", 9, 21),
    ]
}

pub(super) fn api_aspect_ratio(ratio: &str) -> String {
    supported_ratios()
        .iter()
        .find(|(label, _, _)| *label == ratio)
        .map(|(label, _, _)| (*label).to_string())
        .unwrap_or_else(|| "1:1".to_string())
}

pub(super) fn client_ratio_from_api(ratio: &str) -> String {
    match ratio {
        "square" => "1:1".to_string(),
        "landscape" => "3:2".to_string(),
        "portrait" => "2:3".to_string(),
        _ => api_aspect_ratio(ratio),
    }
}

pub(super) fn supported_ratios_for_category(_category: &str) -> &'static [(&'static str, i32, i32)] {
    supported_ratios()
}

pub(super) fn max_reference_images_for_category(_category: &str) -> usize {
    MAX_REFERENCE_IMAGES
}

pub(super) fn reference_limit_message(_max_references: usize) -> &'static str {
    "最多上传 8 张参考图"
}

pub(super) fn normalize_creation_mode_for_category(_category: &str, creation: &str) -> String {
    creation.to_string()
}

pub(super) fn normalized_quality(quality: &str) -> &'static str {
    match quality.trim().to_ascii_uppercase().as_str() {
        "4K" => "4K",
        "2K" => "2K",
        _ => "1K",
    }
}

pub(super) fn pixel_dimensions_for(ratio: &str, quality: &str) -> (i32, i32) {
    let max_edge = match normalized_quality(quality) {
        "4K" => 4096,
        "2K" => 2048,
        _ => 1024,
    };
    let (w, h) = ratio_dimensions(ratio);
    if w <= 0 || h <= 0 {
        return (max_edge, max_edge);
    }
    if w >= h {
        (
            max_edge,
            round_dimension(max_edge as f64 * h as f64 / w as f64),
        )
    } else {
        (
            round_dimension(max_edge as f64 * w as f64 / h as f64),
            max_edge,
        )
    }
}

pub(super) fn round_dimension(value: f64) -> i32 {
    (((value.max(64.0) / 8.0).round() as i32) * 8).max(64)
}

pub(super) fn ratio_dimensions(ratio: &str) -> (i32, i32) {
    supported_ratios()
        .iter()
        .find(|(label, _, _)| *label == ratio)
        .map(|(_, w, h)| (*w, *h))
        .unwrap_or((1, 1))
}

pub(super) fn ratio_from_actual_dimensions(width: i32, height: i32) -> String {
    if width <= 0 || height <= 0 {
        return "1:1".to_string();
    }
    let actual = width as f64 / height as f64;
    supported_ratios()
        .iter()
        .min_by(|left, right| {
            let left_ratio = left.1 as f64 / left.2 as f64;
            let right_ratio = right.1 as f64 / right.2 as f64;
            (actual - left_ratio)
                .abs()
                .partial_cmp(&(actual - right_ratio).abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(label, _, _)| (*label).to_string())
        .unwrap_or_else(|| "1:1".to_string())
}

pub(super) fn quality_from_actual_dimensions(width: i32, height: i32) -> String {
    let longest = width.max(height);
    if longest > 2048 {
        "4K".to_string()
    } else if longest > 1024 {
        "2K".to_string()
    } else {
        "1K".to_string()
    }
}
