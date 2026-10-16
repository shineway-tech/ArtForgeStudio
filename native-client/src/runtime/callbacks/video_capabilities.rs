use super::*;

#[derive(Debug, Default, PartialEq)]
struct VideoCapabilities {
    ratios: Vec<String>,
    durations: Vec<i32>,
    resolutions: Vec<String>,
}

fn positive_integer(value: &Value) -> Option<i32> {
    value
        .as_i64()
        .and_then(|value| i32::try_from(value).ok())
        .or_else(|| value.as_str()?.parse().ok())
        .filter(|value| *value > 0)
}

fn parse_video_capabilities(model: &CatalogModelView) -> VideoCapabilities {
    let value: Value = serde_json::from_str(model.capability_json.as_str()).unwrap_or(Value::Null);
    let mut ratios = value
        .get("aspect_ratios")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter_map(video_ratio_from_api)
        .collect::<Vec<_>>();
    ratios.dedup();
    let duration = [
        "duration_secs",
        "duration_seconds",
        "durations",
        "duration_options",
        "duration_range",
    ]
    .iter()
    .find_map(|key| value.get(*key));
    let mut durations = if let Some(values) = duration.and_then(Value::as_array) {
        values
            .iter()
            .filter_map(positive_integer)
            .filter(|value| (4..=3600).contains(value))
            .collect()
    } else {
        let min = duration
            .and_then(|value| value.get("min"))
            .or_else(|| value.get("min_duration_secs"))
            .or_else(|| value.get("min_duration_seconds"))
            .or_else(|| value.get("min_duration"))
            .and_then(positive_integer);
        let max = duration
            .and_then(|value| value.get("max"))
            .or_else(|| value.get("max_duration_secs"))
            .or_else(|| value.get("max_duration_seconds"))
            .or_else(|| value.get("max_duration"))
            .and_then(positive_integer);
        let step = duration
            .and_then(|value| value.get("step"))
            .and_then(positive_integer)
            .unwrap_or(1);
        match (min, max, duration) {
            (Some(min), Some(max), _) if max >= min && max <= 3600 => (min..=max)
                .step_by(step as usize)
                .filter(|value| *value >= 4)
                .collect(),
            (None, None, Some(value)) if positive_integer(value).is_some() => {
                positive_integer(value)
                    .filter(|value| (4..=3600).contains(value))
                    .into_iter()
                    .collect()
            }
            (None, None, None) => (4..=15).collect(),
            _ => Vec::new(),
        }
    };
    durations.sort_unstable();
    durations.dedup();
    let resolutions = ["480P", "720P", "1080P"]
        .into_iter()
        .filter(|quality| {
            let allowed = value
                .get("resolutions")
                .and_then(Value::as_array)
                .map(|values| values.iter().any(|value| value.as_str() == Some(*quality)))
                .unwrap_or(true);
            allowed && !video_rate(model, quality).is_empty()
        })
        .map(str::to_owned)
        .collect();
    VideoCapabilities {
        ratios,
        durations,
        resolutions,
    }
}

fn valid_video_ratio(value: &str) -> bool {
    let Some((width, height)) = value.split_once(':') else {
        return false;
    };
    [width, height]
        .into_iter()
        .all(|part| part.parse::<u32>().is_ok_and(|value| value > 0))
}

fn video_ratio_from_api(value: &str) -> Option<String> {
    let ratio = match value {
        "square" => "1:1",
        "landscape" => "3:2",
        "portrait" => "2:3",
        value => value,
    };
    (valid_video_ratio(ratio)
        && crate::runtime::configuration::supported_ratios()
            .iter()
            .any(|(supported, _, _)| *supported == ratio))
    .then(|| ratio.to_owned())
}

fn selected_capabilities(state: &AppState) -> VideoCapabilities {
    state
        .get_catalog_models()
        .iter()
        .find(|model| model.code == state.get_video_model() && model.purpose == "video_generation")
        .map(|model| parse_video_capabilities(&model))
        .unwrap_or_default()
}

pub(super) fn video_parameters_supported(state: &AppState) -> bool {
    let options = selected_capabilities(state);
    options
        .ratios
        .iter()
        .any(|ratio| ratio == state.get_video_aspect_ratio().as_str())
        && options
            .resolutions
            .iter()
            .any(|quality| quality == state.get_video_resolution().as_str())
        && options
            .durations
            .contains(&state.get_video_duration_seconds())
}

pub(super) fn normalize_video_parameters(state: &AppState) {
    let options = selected_capabilities(state);
    if !options.ratios.iter().any(|ratio| ratio == state.get_video_aspect_ratio().as_str()) {
        state.set_video_aspect_ratio(options.ratios.first().cloned().unwrap_or_default().into());
    }
    if !options.resolutions.iter().any(|quality| quality == state.get_video_resolution().as_str()) {
        state.set_video_resolution(options.resolutions.iter().find(|quality| quality.as_str() == "720P")
            .or_else(|| options.resolutions.first()).cloned().unwrap_or_default().into());
    }
    let durations = options.durations;
    if let Some((index, seconds)) = durations.iter().enumerate().min_by_key(|(_, seconds)| {
        (i64::from(**seconds) - i64::from(state.get_video_duration_seconds())).abs()
    }) {
        state.set_video_duration_seconds(*seconds);
        state.set_video_duration_index(index as i32);
    }
}

pub(super) fn sync_video_capabilities(state: &AppState) {
    let options = selected_capabilities(state);
    state.set_video_parameters_available(
        !options.ratios.is_empty()
            && !options.durations.is_empty()
            && !options.resolutions.is_empty(),
    );
    if !options
        .ratios
        .iter()
        .any(|ratio| ratio == state.get_video_aspect_ratio().as_str())
    {
        state.set_video_aspect_ratio(options.ratios.first().cloned().unwrap_or_default().into());
    }
    if !options
        .resolutions
        .iter()
        .any(|quality| quality == state.get_video_resolution().as_str())
    {
        state.set_video_resolution(
            options
                .resolutions
                .iter()
                .find(|quality| quality.as_str() == "720P")
                .or_else(|| options.resolutions.first())
                .cloned()
                .unwrap_or_default()
                .into(),
        );
    }
    state.set_video_480_available(options.resolutions.iter().any(|quality| quality == "480P"));
    state.set_video_720_available(options.resolutions.iter().any(|quality| quality == "720P"));
    state.set_video_1080_available(options.resolutions.iter().any(|quality| quality == "1080P"));
    state.set_video_ratio_options(ModelRc::new(VecModel::from(
        options
            .ratios
            .into_iter()
            .map(SharedString::from)
            .collect::<Vec<_>>(),
    )));
    state.set_video_duration_options(ModelRc::new(VecModel::from(options.durations)));
    normalize_video_parameters(state);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(code: &str, capabilities: Value) -> CatalogModelView {
        CatalogModelView {
            code: code.into(),
            purpose: "video_generation".into(),
            capability_json: capabilities.to_string().into(),
            video_price_480: "100".into(),
            video_price_720: "200".into(),
            video_price_1080: "300".into(),
            ..Default::default()
        }
    }

    #[test]
    fn video_capabilities_follow_catalog_ranges_discrete_durations_and_resolutions() {
        let options = parse_video_capabilities(&model(
            "range",
            serde_json::json!({
                "aspect_ratios": ["16:9", "9:16", "broken"], "duration_secs": {"min":4,"max":20,"step":2}, "resolutions":["720P"]
            }),
        ));
        assert_eq!(options.ratios, ["16:9", "9:16"]);
        assert_eq!(options.durations, [4, 6, 8, 10, 12, 14, 16, 18, 20]);
        assert_eq!(options.resolutions, ["720P"]);
        let options = parse_video_capabilities(&model(
            "discrete",
            serde_json::json!({
                "aspect_ratios":["1:1"], "durations":[10,5,5,3], "resolutions":["1080P"]
            }),
        ));
        assert_eq!(options.durations, [5, 10]);
        assert_eq!(options.resolutions, ["1080P"]);
        assert_eq!(parse_video_capabilities(&model("aliases", serde_json::json!({"aspect_ratios":["square","landscape","portrait"]}))).ratios,
            ["1:1", "3:2", "2:3"]);
        assert_eq!(
            parse_video_capabilities(&model(
                "invalid-ratios",
                serde_json::json!({"aspect_ratios":["nonsense","7:5","21:9","16:9"]})
            ))
            .ratios,
            ["21:9", "16:9"]
        );
    }

    #[test]
    fn video_capabilities_switch_preserves_compatible_values_and_normalizes_unsupported_ones() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().unwrap();
        let state = app.global::<AppState>();
        state.set_catalog_models(ModelRc::new(VecModel::from(vec![
            model("first", serde_json::json!({"aspect_ratios":["16:9","1:1"],"durations":[4,8,12],"resolutions":["720P"]})),
            model("second", serde_json::json!({"aspect_ratios":["9:16","1:1"],"durations":[5,10],"resolutions":["1080P"]})),
        ])));
        state.set_video_model("first".into());
        state.set_video_duration_seconds(8);
        sync_video_capabilities(&state);
        assert!(video_parameters_supported(&state));
        state.set_video_aspect_ratio("1:1".into());
        state.set_video_model("second".into());
        sync_video_capabilities(&state);
        assert_eq!(state.get_video_aspect_ratio(), "1:1");
        assert_eq!(state.get_video_resolution(), "1080P");
        assert_eq!(state.get_video_duration_seconds(), 10);
        assert_eq!(state.get_video_duration_index(), 1);
        assert!(!state.get_video_720_available());
        assert!(state.get_video_1080_available());
        assert!(video_parameters_supported(&state));
        state.set_video_aspect_ratio("16:9".into());
        assert!(!video_parameters_supported(&state));
        state.set_video_model("missing".into());
        sync_video_capabilities(&state);
        assert!(!state.get_video_parameters_available());
        assert_eq!(state.get_video_ratio_options().row_count(), 0);
    }

    #[test]
    fn video_capabilities_reject_invalid_explicit_limits() {
        for duration in [
            serde_json::json!([]),
            serde_json::json!({"min":10,"max":5}),
            serde_json::json!({"min":4,"max":2147483647}),
        ] {
            assert!(parse_video_capabilities(&model(
                "invalid",
                serde_json::json!({"duration_secs":duration})
            ))
            .durations
            .is_empty());
        }
    }
}
