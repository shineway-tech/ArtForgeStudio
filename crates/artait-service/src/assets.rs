//! 资产元数据服务：从 SQLite 存储读取元数据。

use artait_model::{
    AssetPurpose, ColorMoodPreset, DirectorControls, GameViewPreset, LightingPreset,
    TimeOfDayPreset, WeatherPreset,
};

/// 资产元数据（从 SQLite 存储读取，主数据源）。
/// AppState 里的 AssetItem 是显示缓存，不应作为元数据来源。
pub struct AssetMetadataInfo {
    pub file_name: String,
    pub prompt: String,
    pub quality: String,
    pub aspect_ratio: String,
    pub model: String,
    pub width: i32,
    pub height: i32,
    pub bytes: i32,
    pub domain: String,
    pub director_summary: String,
}

/// 从 SQLite 元数据存储读取资产元数据（主数据源）。
/// fallback_* 仅在 SQLite 不可用时作为显示缓存降级使用。
pub fn read_asset_metadata(
    path: &std::path::Path,
    fallback_name: Option<&str>,
    fallback_bytes: Option<i32>,
    fallback_domain: Option<&str>,
) -> AssetMetadataInfo {
    // 主数据源：SQLite
    let stored = artait_asset::AssetMetadataStore::default()
        .and_then(|store| store.find_by_path(path))
        .ok()
        .flatten();

    let file_name = fallback_name
        .map(str::to_owned)
        .or_else(|| path.file_name().and_then(|s| s.to_str()).map(str::to_owned))
        .unwrap_or_default();

    let prompt = stored
        .as_ref()
        .and_then(|m| m.prompt.clone())
        .unwrap_or_default();
    let quality = stored
        .as_ref()
        .and_then(|m| m.quality.clone())
        .unwrap_or_default();
    let aspect_ratio = stored
        .as_ref()
        .and_then(|m| m.aspect_ratio.clone())
        .unwrap_or_default();
    let model = stored
        .as_ref()
        .and_then(|m| m.model.clone().or_else(|| m.provider_id.clone()))
        .unwrap_or_default();

    let width = stored
        .as_ref()
        .and_then(|m| m.width)
        .and_then(|v| i32::try_from(v).ok())
        .unwrap_or(0);

    let height = stored
        .as_ref()
        .and_then(|m| m.height)
        .and_then(|v| i32::try_from(v).ok())
        .unwrap_or(0);

    // bytes：SQLite 没有此字段，用 fallback（AssetItem 缓存的文件大小）
    let bytes = fallback_bytes.unwrap_or(0);

    let domain = fallback_domain.map(str::to_owned).unwrap_or_default();
    let director_summary = stored
        .as_ref()
        .and_then(|m| m.request_metadata_json.as_deref())
        .map(director_summary_from_request_metadata)
        .unwrap_or_default();

    AssetMetadataInfo {
        file_name,
        prompt,
        quality,
        aspect_ratio,
        model,
        width,
        height,
        bytes,
        domain,
        director_summary,
    }
}

pub fn director_summary_from_request_metadata(raw: &str) -> String {
    let value = match serde_json::from_str::<serde_json::Value>(raw) {
        Ok(value) => value,
        Err(_) => return String::new(),
    };
    let Some(controls_value) = value.get("director_controls") else {
        return String::new();
    };
    let Ok(controls) = serde_json::from_value::<DirectorControls>(controls_value.clone()) else {
        return String::new();
    };
    director_summary(&controls)
}

pub fn director_summary(controls: &DirectorControls) -> String {
    let mut parts = Vec::new();
    push_part(
        &mut parts,
        "用途",
        controls.purpose.map(AssetPurpose::label),
    );
    push_part(
        &mut parts,
        "视角",
        controls.game_view.map(GameViewPreset::label),
    );
    push_part(
        &mut parts,
        "色彩",
        controls.color_mood.map(ColorMoodPreset::label),
    );
    push_part(
        &mut parts,
        "天气",
        controls.weather.map(WeatherPreset::label),
    );
    push_part(
        &mut parts,
        "时间",
        controls.time_of_day.map(TimeOfDayPreset::label),
    );
    push_part(
        &mut parts,
        "光照",
        controls.lighting.map(LightingPreset::label),
    );
    parts.join(" · ")
}

fn push_part(parts: &mut Vec<String>, label: &str, value: Option<&str>) {
    if let Some(value) = value.filter(|v| !v.trim().is_empty()) {
        parts.push(format!("{label}: {value}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn director_summary_reads_request_metadata() {
        let raw = r#"{
            "director_controls": {
                "purpose": "tileset",
                "game_view": "2_5d",
                "color_mood": "japanese_fantasy",
                "weather": "rainy",
                "time_of_day": "night",
                "lighting": "cinematic"
            }
        }"#;

        let summary = director_summary_from_request_metadata(raw);

        assert!(summary.contains("用途: TileSet"));
        assert!(summary.contains("视角: 2.5D"));
        assert!(summary.contains("天气: 雨天"));
        assert!(summary.contains("时间: 深夜"));
        assert!(summary.contains("光照: 电影感"));
    }

    #[test]
    fn director_summary_ignores_missing_metadata() {
        assert_eq!("", director_summary_from_request_metadata("{}"));
        assert_eq!("", director_summary_from_request_metadata("not json"));
    }
}
