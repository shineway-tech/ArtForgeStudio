//! 将游戏资产导演级控制编译为生成提示词。

use artait_model::{CreationMode, DirectorControls};

pub fn compile_director_prompt(mode: CreationMode, controls: &DirectorControls) -> String {
    let mut parts = Vec::new();

    let _ = mode;
    if let Some(purpose) = controls.purpose {
        parts.push(format!("Asset purpose: {}", purpose.prompt_label()));
    }

    if let Some(view) = controls.game_view {
        parts.push(format!("Game camera/view: {}", view.prompt_label()));
    }
    if let Some(weather) = controls.weather {
        parts.push(format!("Weather: {}", weather.prompt_label()));
    }
    if let Some(time) = controls.time_of_day {
        parts.push(format!("Time of day: {}", time.prompt_label()));
    }
    if let Some(lighting) = controls.lighting {
        parts.push(format!("Lighting: {}", lighting.prompt_label()));
    }
    if let Some(mood) = controls.color_mood {
        parts.push(format!("Color mood: {}", mood.prompt_label()));
    }

    parts.join("\n")
}

pub fn append_director_prompt(
    base_prompt: &str,
    mode: CreationMode,
    controls: &DirectorControls,
) -> String {
    let director_prompt = compile_director_prompt(mode, controls);
    let base = base_prompt.trim();
    if director_prompt.is_empty() {
        base.to_string()
    } else if base.is_empty() {
        director_prompt
    } else {
        format!("{base}\n\n{director_prompt}")
    }
}

#[cfg(test)]
mod tests {
    use artait_model::{
        AssetPurpose, ColorMoodPreset, CreationMode, DirectorControls, GameViewPreset,
        LightingPreset, TimeOfDayPreset, WeatherPreset,
    };

    use super::*;

    #[test]
    fn compile_director_prompt_includes_p0_controls() {
        let controls = DirectorControls {
            purpose: Some(AssetPurpose::TileSet),
            color_mood: Some(ColorMoodPreset::JapaneseFantasy),
            game_view: Some(GameViewPreset::TwoPointFiveD),
            weather: Some(WeatherPreset::Rainy),
            time_of_day: Some(TimeOfDayPreset::Night),
            lighting: Some(LightingPreset::Cinematic),
        };

        let prompt = compile_director_prompt(CreationMode::Scene, &controls);

        assert!(prompt.contains("tileset"));
        assert!(prompt.contains("2.5D"));
        assert!(prompt.contains("rainy"));
        assert!(prompt.contains("deep night"));
        assert!(prompt.contains("dramatic lighting"));
        assert!(prompt.contains("Japanese fantasy"));
    }

    #[test]
    fn compile_director_prompt_stays_empty_without_selected_controls() {
        let prompt = compile_director_prompt(CreationMode::Effect, &DirectorControls::default());

        assert!(prompt.is_empty());
    }

    #[test]
    fn append_director_prompt_keeps_manual_prompt_first() {
        let controls = DirectorControls {
            purpose: Some(AssetPurpose::Hud),
            ..Default::default()
        };

        let prompt = append_director_prompt("森林村庄", CreationMode::Ui, &controls);

        assert!(prompt.starts_with("森林村庄\n\nAsset purpose: game HUD"));
    }
}
