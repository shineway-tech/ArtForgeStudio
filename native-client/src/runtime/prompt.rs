use super::*;

pub(super) fn refresh_advanced_prompt_preview(app: &AppWindow) {
    let state = app.global::<AppState>();
    let language = if state.get_language().as_str() == "en" {
        PromptLanguage::English
    } else {
        PromptLanguage::Chinese
    };
    let controls = PromptControls {
        category: resolve_category(&state.get_asset_type().to_string(), ""),
        creation: state.get_creation_mode().to_string(),
        style: state.get_style_mode().to_string(),
        view: state.get_view_mode().to_string(),
        weather: state.get_weather_mode().to_string(),
        time: state.get_time_mode().to_string(),
        light: state.get_light_mode().to_string(),
    };
    let text = advanced_prompt_preview_text(&controls, language);
    state.set_advanced_prompt_preview(text.into());
}

pub(super) fn resolve_category(selected: &str, _prompt: &str) -> String {
    match selected {
        "character" | "scene" | "ui" | "effect" => selected.to_string(),
        _ => "character".to_string(),
    }
}

pub(super) fn resolve_ratio_for_category(
    category: &str,
    selected: &str,
    prompt: &str,
    quoted: &str,
) -> String {
    let ratios = supported_ratios_for_category(category);
    if selected != "smart" {
        return ratios
            .iter()
            .find(|(label, _, _)| *label == selected)
            .map(|(label, _, _)| (*label).to_string())
            .unwrap_or_else(|| "1:1".to_string());
    }
    let text = prompt.to_lowercase();
    for (ratio, _, _) in ratios {
        if text.contains(*ratio) {
            return (*ratio).to_string();
        }
    }
    if ratios.iter().any(|(ratio, _, _)| *ratio == quoted) {
        return quoted.to_string();
    }
    "1:1".to_string()
}

pub(super) fn control_label(kind: &str, value: &str, language: PromptLanguage) -> &'static str {
    if language == PromptLanguage::Chinese {
        return match (kind, value) {
            ("creation", "character-standee") => "角色立绘",
            ("creation", "character-turnaround") => "角色三视图设定",
            ("creation", "character-8dir") => "角色 8 方向动作",
            ("creation", "character-spritesheet") => "角色 SpriteSheet 序列帧",
            ("creation", "character-spine-parts") => "Spine 角色拆件",
            ("creation", "character-portrait") => "NPC 头像",
            ("creation", "character-poster") => "角色宣传海报",
            ("creation", "scene-concept") => "场景概念设计",
            ("creation", "tileset") => "游戏地图块素材",
            ("creation", "map-ref") => "关卡地图参考",
            ("creation", "poster") => "宣传主视觉海报",
            ("creation", "loading") => "游戏加载页插画",
            ("creation", "minimap") => "俯视小地图",
            ("creation", "building-kit") => "模块化建筑套件",
            ("creation", "ui-hud") => "HUD 战斗界面",
            ("creation", "ui-main-screen") => "游戏主界面",
            ("creation", "ui-backpack") => "背包物品界面",
            ("creation", "ui-shop") => "商城购买界面",
            ("creation", "ui-icon") => "UI 图标",
            ("creation", "ui-loading") => "Loading 载入界面",
            ("creation", "ui-popup") => "弹窗模态界面",
            ("creation", "fx-skill") => "技能特效",
            ("creation", "fx-buff") => "Buff 状态特效",
            ("creation", "fx-explosion") => "爆炸冲击特效",
            ("creation", "fx-scene") => "场景环境特效",
            ("creation", "fx-ui") => "UI 反馈特效",
            ("creation", "fx-weapon-trail") => "武器拖尾轨迹",
            ("creation", "anim-run") => "跑步循环动画",
            ("creation", "anim-walk") => "走路动作",
            ("creation", "anim-attack") => "攻击动作",
            ("creation", "anim-hit") => "受击动作",
            ("creation", "anim-idle") => "待机循环动画",
            ("creation", "anim-jump") => "跳跃动作",
            ("creation", "anim-death") => "死亡动作",
            ("creation", "anim-skill") => "技能动作",
            ("creation", _) => "自由创作",
            ("style", "warm") => "温暖治愈风格",
            ("style", "cold") => "冷系压迫风格",
            ("style", "vivid") => "高饱和鲜艳色彩",
            ("style", "soft") => "低饱和柔和色彩",
            ("style", "dark") => "黑暗奇幻风格",
            ("style", "cyber") => "赛博朋克霓虹风格",
            ("style", "fantasy") => "日式幻想风格",
            ("style", "ghibli") => "绘本动画风格",
            ("style", _) => "自由风格",
            ("view", "top-down") => "俯视视角",
            ("view", "2.5d") => "2.5D 斜视角",
            ("view", "isometric") => "等距视角",
            ("view", "side-view") => "侧视视角",
            ("view", "third-person") => "第三人称视角",
            ("view", "first-person") => "第一人称视角",
            ("view", "orthographic") => "正交视角",
            ("view", _) => "自由视角",
            ("weather", "sunny") => "晴天",
            ("weather", "cloudy") => "阴天",
            ("weather", "rainy") => "雨天",
            ("weather", "storm") => "暴风雨天气",
            ("weather", "snow") => "雪天",
            ("weather", "fog") => "雾天",
            ("weather", "dust") => "沙尘氛围",
            ("weather", _) => "自然天气",
            ("time", "morning") => "清晨",
            ("time", "noon") => "正午日光",
            ("time", "dusk") => "黄昏金色时刻",
            ("time", "blue-hour") => "蓝调时刻",
            ("time", "night") => "深夜",
            ("time", _) => "自然时间",
            ("light", "soft") => "柔和自然光",
            ("light", "cinematic") => "电影感光照",
            ("light", "glow") => "梦幻发光",
            ("light", "contrast") => "高对比光照",
            ("light", "volumetric") => "体积光束",
            ("light", "neon") => "霓虹光照",
            ("light", _) => "自然光照",
            _ => "",
        };
    }

    match (kind, value) {
        ("creation", "character-standee") => "character full-body standing illustration",
        ("creation", "character-turnaround") => "character three-view turnaround sheet",
        ("creation", "character-8dir") => "character 8-direction action set",
        ("creation", "character-spritesheet") => "character SpriteSheet animation frames",
        ("creation", "character-spine-parts") => "character Spine separated parts",
        ("creation", "character-portrait") => "NPC character portrait",
        ("creation", "character-poster") => "character promotional poster",
        ("creation", "scene-concept") => "scene concept art",
        ("creation", "tileset") => "TileSet game tiles",
        ("creation", "map-ref") => "level design map reference",
        ("creation", "poster") => "key visual promotional artwork",
        ("creation", "loading") => "game loading screen artwork",
        ("creation", "minimap") => "mini map top-down game map",
        ("creation", "building-kit") => "modular building kit",
        ("creation", "ui-hud") => "game HUD battle interface",
        ("creation", "ui-main-screen") => "game main screen entry interface",
        ("creation", "ui-backpack") => "backpack item interface",
        ("creation", "ui-shop") => "shop purchase interface",
        ("creation", "ui-icon") => "UI icon",
        ("creation", "ui-loading") => "loading screen UI",
        ("creation", "ui-popup") => "popup modal interface",
        ("creation", "fx-skill") => "skill visual effect",
        ("creation", "fx-buff") => "buff status visual effect",
        ("creation", "fx-explosion") => "explosion impact visual effect",
        ("creation", "fx-scene") => "scene environmental visual effect",
        ("creation", "fx-ui") => "UI feedback visual effect",
        ("creation", "fx-weapon-trail") => "weapon trail visual effect",
        ("creation", "anim-run") => "run cycle animation",
        ("creation", "anim-walk") => "walk animation",
        ("creation", "anim-attack") => "attack animation",
        ("creation", "anim-hit") => "hit reaction animation",
        ("creation", "anim-idle") => "idle loop animation",
        ("creation", "anim-jump") => "jump animation",
        ("creation", "anim-death") => "death animation",
        ("creation", "anim-skill") => "skill action animation",
        ("creation", _) => "free creation",
        ("style", "warm") => "warm healing style",
        ("style", "cold") => "cold oppressive style",
        ("style", "vivid") => "high saturation vivid color",
        ("style", "soft") => "low saturation soft color",
        ("style", "dark") => "dark fantasy style",
        ("style", "cyber") => "cyberpunk neon style",
        ("style", "fantasy") => "Japanese fantasy style",
        ("style", "ghibli") => "storybook animation style",
        ("style", _) => "free style",
        ("view", "top-down") => "top-down camera view",
        ("view", "2.5d") => "2.5D angled view",
        ("view", "isometric") => "isometric view",
        ("view", "side-view") => "side view",
        ("view", "third-person") => "third person view",
        ("view", "first-person") => "first person view",
        ("view", "orthographic") => "orthographic view",
        ("view", _) => "free camera view",
        ("weather", "sunny") => "sunny weather",
        ("weather", "cloudy") => "cloudy weather",
        ("weather", "rainy") => "rainy weather",
        ("weather", "storm") => "storm weather",
        ("weather", "snow") => "snowy weather",
        ("weather", "fog") => "foggy weather",
        ("weather", "dust") => "dust storm atmosphere",
        ("weather", _) => "natural weather",
        ("time", "morning") => "morning time",
        ("time", "noon") => "noon daylight",
        ("time", "dusk") => "dusk golden hour",
        ("time", "blue-hour") => "blue hour",
        ("time", "night") => "deep night",
        ("time", _) => "natural time of day",
        ("light", "soft") => "soft natural lighting",
        ("light", "cinematic") => "cinematic lighting",
        ("light", "glow") => "dreamy glowing light",
        ("light", "contrast") => "high contrast lighting",
        ("light", "volumetric") => "volumetric light beams",
        ("light", "neon") => "neon lighting",
        ("light", _) => "natural lighting",
        _ => "",
    }
}

pub(super) fn visible_prompt_control_entries<'a>(
    controls: &'a PromptControls,
) -> Vec<(&'static str, &'a str)> {
    if controls.category == "action-sequence" {
        return vec![("creation", controls.creation.as_str())];
    }
    let mut entries = vec![
        ("creation", controls.creation.as_str()),
        ("style", controls.style.as_str()),
    ];
    if controls.category == "scene" || controls.category == "character" {
        entries.push(("view", controls.view.as_str()));
    }
    if controls.category == "scene" {
        entries.push(("weather", controls.weather.as_str()));
        entries.push(("time", controls.time.as_str()));
    }
    entries.push(("light", controls.light.as_str()));
    entries
}

pub(super) fn prompt_controls_text(controls: &PromptControls, language: PromptLanguage) -> String {
    visible_prompt_control_entries(controls)
        .iter()
        .map(|(kind, value)| control_label(kind, value, language))
        .filter(|label| !label.is_empty())
        .collect::<Vec<_>>()
        .join(", ")
}

pub(super) fn advanced_prompt_preview_text(controls: &PromptControls, language: PromptLanguage) -> String {
    visible_prompt_control_entries(controls)
        .iter()
        .map(|(kind, value)| {
            let name = match (*kind, language) {
                ("creation", PromptLanguage::Chinese) => "创作方式",
                ("creation", PromptLanguage::English) => "Creation",
                ("style", PromptLanguage::Chinese) => "风格",
                ("style", PromptLanguage::English) => "Style",
                ("view", PromptLanguage::Chinese) => "镜头/视角",
                ("view", PromptLanguage::English) => "Camera/view",
                ("weather", PromptLanguage::Chinese) => "天气",
                ("weather", PromptLanguage::English) => "Weather",
                ("time", PromptLanguage::Chinese) => "时间",
                ("time", PromptLanguage::English) => "Time of day",
                ("light", PromptLanguage::Chinese) => "光照",
                ("light", PromptLanguage::English) => "Lighting",
                _ => "",
            };
            format!("{name}: {}", control_label(kind, value, language))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) fn prompt_with_controls(
    prompt: &str,
    controls: &PromptControls,
    language: PromptLanguage,
) -> String {
    let controls_text = prompt_controls_text(controls, language);
    if controls_text.is_empty() {
        prompt.to_string()
    } else if language == PromptLanguage::Chinese {
        format!("{prompt}\n\n生成控制：{controls_text}")
    } else {
        format!("{prompt}\n\nGeneration controls: {controls_text}")
    }
}

pub(super) fn append_action_sequence_instruction(prompt: &str, language: PromptLanguage) -> String {
    if language == PromptLanguage::Chinese {
        format!(
            "{prompt}\n\n动作序列规则：如果上传了参考图，请以参考图中的角色或主体为基础，生成所选动作类型对应的动作资源，保持角色外观、服装、配色和识别特征一致。动作类型只从待机、跑步、走路、攻击、死亡中选择，不要生成无关动作。"
        )
    } else {
        format!(
            "{prompt}\n\nAction sequence rule: If a reference image is uploaded, use the character or subject in the reference image as the basis for the selected action asset. Keep the character appearance, outfit, colors, and identifying traits consistent. The action type must be one of idle, run, walk, attack, or death; do not generate unrelated actions."
        )
    }
}

pub(super) fn build_generation_prompt(
    prompt: &str,
    controls: &PromptControls,
    quote: &QuoteContext,
    category: &str,
    ratio: &str,
    quality: &str,
    language: PromptLanguage,
) -> String {
    let mut final_prompt = prompt_with_controls(prompt, controls, language);
    if !quote.title.trim().is_empty()
        || !quote.prompt.trim().is_empty()
        || !quote.ratio.trim().is_empty()
        || !quote.quality.trim().is_empty()
        || quote.width > 0
        || quote.height > 0
    {
        if language == PromptLanguage::Chinese {
            final_prompt.push_str(&format!(
                "\n\n参考图片信息：标题：{}；提示词：{}；宽高比：{}；清晰度：{}；尺寸：{} x {}。请把用户需求理解为对参考图片的修改或延续。",
                quote.title,
                quote.prompt,
                quote.ratio,
                quote.quality,
                quote.width,
                quote.height
            ));
        } else {
            final_prompt.push_str(&format!(
                "\n\nReference image information: title: {}; prompt: {}; aspect ratio: {}; resolution: {}; size: {} x {}. Treat the user request as an edit or continuation of the reference image.",
                quote.title,
                quote.prompt,
                quote.ratio,
                quote.quality,
                quote.width,
                quote.height
            ));
        }
    }
    if category == "action-sequence" {
        final_prompt = append_action_sequence_instruction(&final_prompt, language);
    }
    append_parameter_priority_instruction(&final_prompt, category, ratio, quality, language)
}

pub(super) fn append_parameter_priority_instruction(
    prompt: &str,
    category: &str,
    ratio: &str,
    quality: &str,
    language: PromptLanguage,
) -> String {
    if language == PromptLanguage::Chinese {
        format!(
            "{prompt}\n\n参数优先规则：左侧工作台分类和下方已选择的卡片为最终参数，并覆盖用户提示词中冲突的描述。最终分类：{category}。最终宽高比：{ratio}。最终清晰度：{quality}。应用会按所选张数调用生图模型。除非用户明确要求拼图、网格、分屏或多画面构图，否则不要在一张画布里生成多张图。"
        )
    } else {
        format!(
            "{prompt}\n\nParameter priority rule: the left workspace category and selected cards below are final and override any conflicting words in the user's prompt. Final category: {category}. Final aspect ratio: {ratio}. Final quality: {quality}. The application requests the selected image count from the image model. Do not create grids, collages, contact sheets, split panels, or multiple images inside one canvas unless the user explicitly asks for that composition."
        )
    }
}

pub(super) fn display_generation_prompt(prompt: &str) -> String {
    let normalized = prompt.replace("\r\n", "\n");
    let hidden_prefixes = [
        "生成控制：",
        "参数优先规则：",
        "动作序列规则：",
        "Generation controls:",
        "Parameter priority rule:",
        "Action sequence rule:",
    ];
    normalized
        .split("\n\n")
        .filter(|part| {
            let trimmed = part.trim_start();
            !hidden_prefixes
                .iter()
                .any(|prefix| trimmed.starts_with(prefix))
        })
        .collect::<Vec<_>>()
        .join("\n\n")
        .trim()
        .to_string()
}
