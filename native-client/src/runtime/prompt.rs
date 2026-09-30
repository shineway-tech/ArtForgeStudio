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
    if value == "none" {
        return "";
    }
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
    let mut entries = Vec::new();
    let hide_ui_default = controls.category == "ui";
    if controls.creation != "none" && (!hide_ui_default || controls.creation != "free") {
        entries.push(("creation", controls.creation.as_str()));
    }
    if controls.style != "none" && (!hide_ui_default || controls.style != "free") {
        entries.push(("style", controls.style.as_str()));
    }
    if controls.view != "none"
        && (controls.category == "scene" || controls.category == "character")
    {
        entries.push(("view", controls.view.as_str()));
    }
    if controls.category == "scene" && controls.weather != "none" {
        entries.push(("weather", controls.weather.as_str()));
    }
    if controls.category == "scene" && controls.time != "none" {
        entries.push(("time", controls.time.as_str()));
    }
    if controls.light != "none" && (!hide_ui_default || controls.light != "free") {
        entries.push(("light", controls.light.as_str()));
    }
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

pub(super) fn advanced_prompt_preview_text(
    controls: &PromptControls,
    language: PromptLanguage,
) -> String {
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

pub(super) fn build_generation_prompt(
    prompt: &str,
    negative_prompt: &str,
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
    final_prompt = append_negative_prompt_instruction(&final_prompt, negative_prompt, language);
    final_prompt =
        append_parameter_priority_instruction(&final_prompt, category, ratio, quality, language);
    append_category_generation_instruction(&final_prompt, category, language)
}

pub(super) fn build_generation_prompt_for_destination(
    prompt: &str,
    negative_prompt: &str,
    controls: &PromptControls,
    quote: &QuoteContext,
    category: &str,
    ratio: &str,
    quality: &str,
    language: PromptLanguage,
    destination: &GenerationDestination,
) -> String {
    match destination {
        GenerationDestination::Canvas { .. } => build_canvas_generation_prompt(
            prompt, ratio, quality, language == PromptLanguage::English,
        ),
        GenerationDestination::Gallery => build_generation_prompt(
            prompt, negative_prompt, controls, quote, category, ratio, quality, language,
        ),
    }
}

pub(super) fn compose_side_scroll_map_prompt(description: &str, english: bool) -> String {
    let description = description.trim();
    let request = if description.is_empty() {
        if english {
            "Create a new map based on the reference image."
        } else {
            "根据参考图创作一张新地图。"
        }
    } else {
        description
    };
    if english {
        format!("{request}\n\nSide-scrolling seamless map specification: use the uploaded reference image only as the source for visual style, palette, materials, linework, rendering medium, perspective, lighting, atmosphere, and reusable environmental motifs. Create one full-bleed horizontal game map using the selected landscape aspect ratio, not a concept sheet or presentation mockup. Build three clearly readable depth planes by default: foreground, midground, and background. Keep the main traversable route readable across the foreground and midground, with coherent scale and side-view game composition. The map must tile seamlessly from right to left: the terrain height, path elevation, waterline, silhouettes, vegetation density, architecture rhythm, lighting, fog, colors, and texture at the right edge must continue exactly into the left edge. Avoid a unique landmark crossing either boundary. Verify the result by imagining two copies placed side by side; the join must be invisible and the route must remain continuous. Preserve stylistic identity without copying the reference composition. Output only the finished environmental map. The image may contain map scenery and environment only. Never include player characters, protagonists, humans, humanoids, NPCs, enemies, monsters, creatures, animals, or character silhouettes of any kind. If any such subject appears in the reference image or user description, ignore it and do not reproduce it. Do not add gameplay or post-processing special effects, including particles, magic, skill or combat effects, attack trails, explosions, auras, floating glows, or lens flares. These map-only restrictions override any conflicting content in the reference image or user description. No reference-image inset, frame, arrows, carousel controls, labels, words, letters, numbers, UI, watermark, or border.")
    } else {
        format!("{request}\n\n横版无缝地图规范：仅将上传的参考图作为视觉风格、配色、材质、线条、渲染方式、透视、光照、氛围与环境元素的来源。按用户选择的横屏比例输出一张完整铺满画布的横版游戏地图，不是概念设定表或展示稿。默认建立清晰可辨的前景、中景、后景三个景深层级；在前景与中景中保持连续、易读的横向可行走路线，比例统一，符合横版游戏侧视构图。地图必须能够从右向左无缝循环：右边缘与左边缘的地形高度、道路标高、水位、轮廓、植被密度、建筑节奏、光照、雾气、颜色和纹理必须准确连续；不要让唯一性地标跨越任一拼接边界。生成前以两张成品左右并排的方式检查，接缝必须不可见，行走路线必须连续。保留参考图的风格识别，但不要复制参考图原有构图。只输出最终环境地图，画面只允许出现地图场景和环境内容。严禁出现玩家角色、主角、人类、类人角色、NPC、敌人、怪物、生物、动物或任何角色轮廓；即使参考图或用户描述中包含这些主体，也必须忽略且不得复现。不得添加任何游戏特效或后期特效，包括粒子、魔法、技能或战斗特效、攻击拖尾、爆炸、光环、悬浮光效和镜头光晕。纯地图限制优先于参考图和用户描述中的任何冲突内容。不得出现参考图缩略图、边框、箭头、轮播控件、标签、文字、字母、数字、界面、水印或外框。")
    }
}

pub(super) fn compose_scene_composition_prompt(description: &str, english: bool) -> String {
    let description = description.trim();
    let request = if description.is_empty() {
        if english {
            "Combine the uploaded reference images into one complete scene."
        } else {
            "将上传的多张参考图组合成一张完整场景。"
        }
    } else {
        description
    };

    if english {
        format!("User description: {request}\n\nScene composition specification: treat all uploaded reference images as ordered source material for one new, coherent scene. Every uploaded reference must contribute at least one intentional and recognizable subject, object, environmental element, design motif, material, palette, or style characteristic to the final image; do not omit a reference without a visual contribution. Use the first uploaded reference as the canonical art-style anchor. If later references use a different style, redraw and reinterpret their content in the first reference's style instead of mixing incompatible renderings. Rebuild all selected content inside one unified environment rather than copy-pasting source pixels. Match drawing or rendering medium, line quality, shape language, palette, material treatment, texture density, detail level, edge treatment, and overall finish across the entire result. Establish one consistent camera, viewpoint, perspective, horizon, scale, depth system, light direction, light color, shadow softness, atmosphere, and color grading. Arrange the referenced content with believable spatial relationships, contact, occlusion, depth, and visual hierarchy so it reads as a naturally designed scene, not a collection of separate assets. Follow the user description for the scene concept and placement while preserving recognizable reference features. Output exactly one full-bleed finished scene in the selected aspect ratio. Do not output a collage, contact sheet, mood board, asset sheet, before-and-after comparison, split screen, isolated cutouts, reference thumbnails, source panels, pasted rectangles, mismatched borders, or duplicated source images. Do not include arrows, guide lines, grids, frames, labels, words, letters, numbers, captions, logos, UI, signatures, or watermarks. The unified-scene and style-consistency rules override conflicting layout or style instructions in the user description.")
    } else {
        format!("用户描述：{request}\n\n场景组合规范：将全部已上传参考图作为有顺序的素材来源，重新设计并融合为一张完整、连贯的新场景。每张参考图都必须至少为最终画面贡献一个有意图且可辨识的主体、物件、环境元素、设计母题、材质、配色或画风特征，不得无故遗漏任何参考图。以第一张参考图作为统一画风基准；如果后续参考图的画风不同，必须把其中内容重新绘制并转换为第一张参考图的画风，禁止在成品中混用互不相容的渲染风格。所有选取内容都要在同一个环境中重新构建，不能直接复制粘贴源图像素。整张图必须统一绘画或渲染媒介、线条质量、造型语言、配色、材质表现、纹理密度、细节等级、边缘处理和完成度；统一相机、视角、透视、地平线、比例、景深层级、光源方向、光色、阴影软硬、空气感和整体调色。参考内容之间应具有可信的空间关系、接触、遮挡、远近层次和视觉主次，使结果看起来是一张自然设计的完整场景，而不是多个独立素材的集合。按用户描述确定场景主题和位置关系，同时保留参考内容的可识别特征。按所选宽高比只输出一张铺满画布的最终场景。禁止输出拼贴图、联系表、情绪板、素材表、前后对比、分屏、独立抠图、参考图缩略图、源图面板、粘贴矩形、风格不一致的边框或重复的源图。不得出现箭头、辅助线、网格、边框、标签、文字、字母、数字、说明、标志、界面、签名或水印。完整场景与统一画风规则优先于用户描述中冲突的排版或风格要求。")
    }
}

pub(super) fn normalize_skill_icon_count(count: i32) -> i32 {
    count.clamp(6, 20)
}

pub(super) fn normalize_skill_icon_background(background: &str) -> &'static str {
    match background.trim() {
        "black" | "黑色" => "black",
        _ => "white",
    }
}

pub(super) fn normalize_skill_icon_shape(shape: &str) -> &'static str {
    match shape.trim() {
        "square" | "方形" => "square",
        _ => "circle",
    }
}

pub(super) fn compose_skill_icon_prompt(
    description: &str,
    icon_count: i32,
    background: &str,
    shape: &str,
    english: bool,
) -> String {
    let description = description.trim();
    let request = if description.is_empty() {
        if english {
            "The game genre, skill type, and skill descriptions are unspecified; infer a coherent skill set from the uploaded reference images."
        } else {
            "未填写游戏类型、技能类型和技能描述时，请根据全部已上传参考图的视觉线索自由设计一套连贯的技能图标。"
        }
    } else {
        description
    };
    let icon_count = normalize_skill_icon_count(icon_count);
    let background = normalize_skill_icon_background(background);
    let shape = normalize_skill_icon_shape(shape);
    let columns = match icon_count {
        6 => 3,
        7..=8 => 4,
        9 => 3,
        10..=12 => 4,
        13..=15 => 5,
        16 => 4,
        _ => 5,
    };
    let rows = (icon_count + columns - 1) / columns;

    if english {
        let background_rule = if background == "black" {
            "Use a perfectly uniform pure black (#000000) background across the entire canvas and every gutter. Do not use transparency, checkerboards, gradients, textures, scenery, or decorative background graphics."
        } else {
            "Use a perfectly uniform pure white (#FFFFFF) background across the entire canvas and every gutter. Do not use transparency, checkerboards, gradients, textures, scenery, or decorative background graphics."
        };
        let shape_rule = if shape == "square" {
            "Every skill icon must use the same exact square tile shape and side length. Keep all artwork and effects safely inside its square boundary. Do not substitute circles, rounded rectangles, diamonds, or irregular silhouettes."
        } else {
            "Every skill icon must use the same exact circular tile shape and diameter. Keep all artwork and effects safely inside its circular boundary. Do not substitute squares, rounded rectangles, ovals, shields, or irregular silhouettes."
        };
        format!(
            "User skill brief: {request}\n\nSkill icon generation specification: use all uploaded reference images, from one to eight images, as one unified style-reference set. Use the first uploaded reference as the canonical style anchor, then use later references to reinforce recurring choices in color language, linework, rendering medium, shading, lighting, material treatment, effect treatment, border design, texture density, and detail level. If references conflict, resolve them into one coherent system in the first reference's style instead of mixing incompatible styles. Do not copy reference subjects into every icon and do not place any reference image or reference thumbnail in the result. Interpret the user's game genre, skill type, and skill descriptions as design direction; when any field is blank, infer a coherent choice from the combined reference style and the other supplied fields. Generate exactly {icon_count} distinct, game-ready skill icons in one finished icon sheet. Every icon must represent a clearly different ability, silhouette, focal symbol, and effect language while remaining part of one consistent visual system. Avoid duplicates, near-duplicates, repeated symbols, and simple recolors. Arrange the icons in a balanced grid using {columns} columns and at most {rows} rows, ordered left to right then top to bottom. If the final row is incomplete, center that row without adding placeholders. Keep every icon the same size, use even horizontal and vertical spacing, preserve generous outer margins, and never crop an icon or let neighboring effects touch. {background_rule} {shape_rule} Output only the final icon sheet in the selected aspect ratio. Do not create a mockup, menu, inventory screen, phone screen, presentation board, reference comparison, or separate source panel. Do not draw visible grid lines, cell borders, labels, skill names, words, letters, numbers, captions, logos, signatures, UI controls, or watermarks. The exact icon count, selected background, selected shape, and style-consistency rules override conflicting user text or reference content."
        )
    } else {
        let background_rule = if background == "black" {
            "整张画布及图标间隔必须使用完全均匀的纯黑色（#000000）背景。不得使用透明背景、棋盘格、渐变、纹理、场景或装饰性背景图案。"
        } else {
            "整张画布及图标间隔必须使用完全均匀的纯白色（#FFFFFF）背景。不得使用透明背景、棋盘格、渐变、纹理、场景或装饰性背景图案。"
        };
        let shape_rule = if shape == "square" {
            "所有技能图标必须使用尺寸完全一致的标准方形图标框，全部图案与特效都要安全收在方形边界内。禁止替换为圆形、圆角矩形、菱形或不规则轮廓。"
        } else {
            "所有技能图标必须使用直径完全一致的标准圆形图标框，全部图案与特效都要安全收在圆形边界内。禁止替换为方形、圆角矩形、椭圆形、盾牌形或不规则轮廓。"
        };
        format!(
            "用户技能需求：{request}\n\n技能图标生成规范：将全部已上传的1至8张参考图作为同一套统一画风参考。以第一张参考图作为主要画风基准，再用后续参考图补充和强化反复出现的配色语言、线条、渲染媒介、明暗塑造、光照、材质表现、特效表现、边框设计、纹理密度和细节等级；参考图之间存在冲突时，必须统一转换为第一张参考图的画风，禁止混用互不相容的风格。不得把参考图主体机械复制到每个图标中，也不得在结果中放入任何参考图或参考图缩略图。将用户填写的游戏类型、技能类型和技能描述作为设计方向；任一项留空时，根据所有参考图的综合画风和其他已填写内容补全一套合理、连贯的设计。只在一张最终技能图标表中生成恰好{icon_count}个彼此不同、可直接用于游戏的技能图标。每个图标都必须代表明显不同的能力、轮廓、核心符号和特效语言，同时保持为同一套统一视觉系统。禁止重复、近似重复、反复使用同一符号或仅换色。使用{columns}列、最多{rows}行的均衡网格，按从左到右、从上到下排列；最后一行不足时整体居中，不得用空白占位图标补齐。所有图标尺寸一致，横向和纵向间距均匀，四周保留充足安全边距；不得裁切图标，相邻图标的特效不得接触。{background_rule}{shape_rule}按所选宽高比只输出最终图标表。禁止输出样机、菜单、背包界面、手机界面、展示板、参考图对比或独立素材面板。不得绘制可见网格线、单元格边框、标签、技能名称、文字、字母、数字、说明、标志、签名、界面控件或水印。精确图标数量、所选背景、所选形状与统一画风要求优先于用户文本或参考图中的冲突内容。"
        )
    }
}

pub(super) fn normalize_side_scroll_map_ratio(ratio: &str) -> String {
    match ratio.trim() {
        "3:2" | "4:3" | "16:9" | "21:9" => ratio.trim().to_string(),
        _ => "21:9".to_string(),
    }
}

pub(super) fn build_side_scroll_map_generation_prompt(
    prompt: &str,
    ratio: &str,
    quality: &str,
    language: PromptLanguage,
) -> String {
    append_parameter_priority_instruction(prompt, "scene", ratio, quality, language)
}

pub(super) fn normalize_character_multi_direction_count(count: i32) -> i32 {
    match count {
        3 | 4 | 8 => count,
        _ => 8,
    }
}

pub(super) fn normalize_character_multi_direction_action(action: &str) -> &'static str {
    match action.trim() {
        "standing" | "站立" => "standing",
        "walking" | "行走" => "walking",
        "running" | "奔跑" => "running",
        _ => "standing",
    }
}

pub(super) fn compose_character_multi_direction_prompt(
    description: &str,
    direction_count: i32,
    action: &str,
    english: bool,
) -> String {
    let description = description.trim();
    let request = if description.is_empty() {
        if english {
            "Create a character multi-direction sheet from the uploaded reference image."
        } else {
            "根据上传的角色参考图生成角色多向图。"
        }
    } else {
        description
    };
    let direction_count = normalize_character_multi_direction_count(direction_count);
    let action = normalize_character_multi_direction_action(action);

    if english {
        let action_label = match action {
            "walking" => "walking",
            "running" => "running",
            _ => "standing",
        };
        let phase_rules = "Pose-phase lock: treat the character as one frozen three-dimensional pose rotated around the vertical axis; change only facing direction and never re-pose the limbs between cells. Left and right always mean the character's own anatomical left and right, never screen-left or screen-right. Never mirror the pose, swap the left and right legs, reverse the arm swing, or move a weapon, shield, or prop to the opposite hand when the facing direction changes.";
        let action_rules = match action {
            "walking" => format!(
                "Action-state lock: all {direction_count} figures in every occupied cell must unmistakably be walking; do not make only the front view or top row walk. Use one identical representative mid-stride phase in every view: the character's anatomical left leg is always the forward leading leg, the anatomical right leg is always trailing, the anatomical right arm is always forward, and the anatomical left arm is always back. Keep that exact left-foot-forward phase in the front, profile, back, and every diagonal view even though its screen position changes after rotation. Every figure must show clearly offset legs, natural opposite arm-and-leg swing, and a visibly moving center of gravity. No cell may lead with the anatomical right foot, mirror the phase, use a static symmetrical planted stance, a running stride, an airborne pose, or an extra animation frame. Inspect every occupied cell before finalizing: all {direction_count} figures must be walking in the same anatomical left-foot-forward phase."
            ),
            "running" => format!(
                "Action-state lock: all {direction_count} figures in every occupied cell must unmistakably be running; do not make only the front view or top row run. Use one identical representative running phase in every view: the character's anatomical left leg is always the forward leading leg, the anatomical right leg is always trailing, the anatomical right arm is always forward, and the anatomical left arm is always back. Keep that exact left-foot-forward phase in the front, profile, back, and every diagonal view even though its screen position changes after rotation. Every figure must show a long forceful stride, clearly separated legs, bent knees, vigorous opposite arm-and-leg drive, forward momentum, and at least one foot lifted or about to leave the ground. No cell may lead with the anatomical right foot, mirror the phase, use a static standing pose, an ordinary walking step, or an extra animation frame. Inspect every occupied cell before finalizing: all {direction_count} figures must be running in the same anatomical left-foot-forward phase."
            ),
            _ => format!(
                "Action-state lock: all {direction_count} figures in every occupied cell must unmistakably be standing. Every figure must use the identical calm standing phase with both feet stably planted at the same spacing, the same balanced weight distribution, and the same arm and hand positions. Do not switch the weight-bearing leg, place a different foot forward, mirror the stance, or swap the weapon hand between views. No cell may show a walking step, running stride, lifted running foot, airborne motion, or an extra animation frame. Inspect every occupied cell before finalizing: all {direction_count} figures must be standing in the same limb phase."
            ),
        };
        let layout = match direction_count {
            3 => "Use a strict 3-column by 1-row layout with exactly three equal cells and exactly three character figures total, ordered left to right: cell 1 front view facing the viewer with the complete face visible; cell 2 exact 90-degree right profile; cell 3 back view with only the back of the head and back visible. The finished sheet must contain three and only three figures. Do not add a left profile, diagonal or three-quarter views, intermediate turns, duplicate views, inset figures, or extra animation frames. Count the figures before finalizing: the total must equal 3; if a fourth figure exists, remove it.",
            4 => "Use a strict 4-column by 1-row layout with four equal cells and exactly four character figures total, ordered left to right: front view, exact 90-degree right profile, back view, exact 90-degree left profile. Do not add diagonal or three-quarter views, intermediate turns, duplicate views, inset figures, or extra animation frames.",
            _ => "Use a strict 3-column by 3-row nine-cell layout with nine equal invisible cells. Leave the center cell at row 2, column 2 completely empty and fully transparent: no character, reference thumbnail, object, mark, shadow, or decoration may occupy the center. Place exactly eight character views in the eight surrounding cells, all facing inward toward the empty center. Position them exactly as follows: row 1 column 1 faces down-right at 45 degrees; row 1 column 2 is the front view facing the viewer with the complete face visible; row 1 column 3 faces down-left at 45 degrees; row 2 column 1 faces exactly right and shows only the right profile; row 2 column 3 faces exactly left and shows only the left profile; row 3 column 1 faces up-right at 45 degrees; row 3 column 2 faces away with the back of the head and back visible and no facial features; row 3 column 3 faces up-left at 45 degrees. All eight directions must be visibly different and form one complete evenly spaced rotation around the empty center.",
        };
        let placement = if direction_count == 8 {
            "Keep all nine invisible cells exactly the same size. Center one character horizontally and vertically within each occupied cell, keep identical character scale in all eight occupied cells, align the feet consistently within each grid row, and make each character about 70% of its cell height. Preserve even spacing around the empty center cell."
        } else {
            "Keep every cell exactly the same size. Center the same character horizontally in every cell, align all feet to one shared horizontal baseline, and make the character about 70% of each cell's height."
        };
        let view = if direction_count == 8 {
            "Fixed camera rule: all eight directions must use a strong isometric-style 2.5D elevated oblique game view, like a three-quarter top-down RPG character camera. Use an orthographic or near-orthographic projection with the camera at least 45 degrees above the horizon, preferably between 45 and 60 degrees, and aimed downward; the elevation must never fall below 45 degrees or approach eye level. Every pose must clearly reveal the top planes of the head or hair and shoulders, plus upper surfaces of clothing, arms and feet where appropriate; the full body must show obvious vertical foreshortening and front-to-back depth stacking from head to feet. The result must read immediately as an isometric or top-down game sprite, not a flat front-side-back character turnaround. Keep the character upright and apply the elevated camera to the entire body; do not fake the angle by tilting only the face or head while leaving the torso eye-level. Keep the camera elevation, viewing direction, projection and scale identical in every cell; rotate only the character around its vertical axis in the specified facing order, not the camera. Do not use a flat eye-level view, a side-scrolling view, a low-angle view or a vertical 90-degree top-down view. This fixed 2.5D camera overrides any conflicting viewpoint in the user description or reference image, while preserving the reference's original drawing or pixel-art style; it does not require converting the artwork into a 3D render. "
        } else {
            ""
        };
        let orientation_rules = match direction_count {
            3 => "The front must show the complete face; the exact right profile may show only one eye and must have a clearly narrower body silhouette; the back must show no facial features. These are orthographic character turnaround views, not diagonal views.",
            4 => "The front must show the complete face; each exact side profile may show only one eye and must have a clearly narrower body silhouette; the back must show no facial features. These are orthographic character turnaround views, not diagonal views.",
            _ => "The front must show the complete face; the back must show no facial features; an exact side profile may show only one eye and must have a clearly narrower body silhouette; diagonal views must be visually intermediate with clear perspective compression.",
        };
        format!(
            "User description: {request}\n\nCharacter multi-direction sheet specification: use the uploaded reference image only to preserve the same character identity, hairstyle, clothing cut, palette, body proportions, height proportions, and rendering or pixel-art style; do not place the reference thumbnail in the result. The selected action is a {action_label} pose. {phase_rules} {action_rules} Generate exactly one representative pose for each requested view, never an animation sequence. {view}{layout} {placement} {orientation_rules} Do not change the hairstyle, clothing, colors, proportions, or body type between views. Keep the same character identity in every cell and change only the requested view while preserving the selected action state and anatomical limb phase. Use a fully transparent background for the entire image, including the empty center cell. Do not add shadows, cast shadows, ground planes, glow, aura, projection, lighting effects, particles, or decorative elements. Do not draw grid lines, frames, borders, separators, cell outlines, text, labels, numbers, direction markers, arrows, captions, logos, or watermarks. Output only the finished transparent sheet."
        )
    } else {
        let action_label = match action {
            "walking" => "行走",
            "running" => "奔跑",
            _ => "站立",
        };
        let phase_rules = "动作相位硬约束：把角色视为同一个冻结的三维姿势绕自身竖直轴旋转，只改变朝向，绝不能在不同格重新摆腿或摆手。所有左右均指角色自身的解剖学左侧和右侧，不是画面左侧和右侧。朝向改变时禁止镜像动作、禁止交换左右腿、禁止反转手臂摆动，也禁止把武器、盾牌或道具换到另一只手。";
        let action_rules = match action {
            "walking" => format!(
                "动作状态硬约束：全部{direction_count}个角色必须逐格处于明确的行走状态，不得只有正面、顶部或部分格子在行走。所有视角统一固定为同一个行走中步态：角色自身的左腿始终是向前迈出的前导腿，角色自身的右腿始终在后；角色自身的右臂始终向前，左臂始终向后。正面、侧面、背面以及所有斜向视角都必须保持这个左脚向前的相同动作相位，即使旋转后它在画面中的左右位置发生变化。每个角色都必须双腿前后明显错开、手臂与腿自然反向摆动、重心处于移动中。禁止任何格子改成角色自身右脚向前，禁止镜像动作，也禁止静止站立、奔跑步幅、腾空姿势或额外动画帧。输出前必须逐格检查：{direction_count}个角色必须全部以角色自身左脚向前的同一相位行走。"
            ),
            "running" => format!(
                "动作状态硬约束：全部{direction_count}个角色必须逐格处于明确的奔跑状态，不得只有正面、顶部或部分格子在奔跑。所有视角统一固定为同一个奔跑动作相位：角色自身的左腿始终是向前冲出的前导腿，角色自身的右腿始终在后；角色自身的右臂始终向前，左臂始终向后。正面、侧面、背面以及所有斜向视角都必须保持这个左脚向前的相同动作相位，即使旋转后它在画面中的左右位置发生变化。每个角色都必须步幅大且有冲力，双腿明显分开、膝盖弯曲，手臂与腿强烈反向摆动，身体具有向前冲刺的动势，并至少有一只脚抬起或即将离地。禁止任何格子改成角色自身右脚向前，禁止镜像动作，也禁止静止站立、普通行走或额外动画帧。输出前必须逐格检查：{direction_count}个角色必须全部以角色自身左脚向前的同一相位奔跑。"
            ),
            _ => format!(
                "动作状态硬约束：全部{direction_count}个角色必须逐格处于明确的站立状态。每个角色都必须采用完全相同的平稳站立相位：双脚以相同间距稳定着地，重心分配一致，手臂和双手位置一致。禁止在不同视角切换承重腿、换一只脚在前、镜像站姿或更换持武器手。禁止任何格子出现行走跨步、奔跑步幅、跑动抬脚、腾空姿势或额外动画帧。输出前必须逐格检查：{direction_count}个角色必须全部保持同一个肢体相位站立。"
            ),
        };
        let layout = match direction_count {
            3 => "严格使用3列1行布局，共3个尺寸完全一致的单元格，整张成图只能出现且必须恰好出现3个完整角色。从左到右依次为：第1格正面朝向观者、完整正脸；第2格为严格90度正右侧面；第3格为背面，只显示后脑勺与后背、没有五官。禁止增加左侧面、斜向、四分之三角度、过渡角度、重复视角、小插图或额外动作帧。输出前必须数清楚：角色总数必须等于3，多一个也不允许；若出现第4个角色必须删除。",
            4 => "严格使用4列1行布局，共4个尺寸完全一致的单元格，整张成图只能出现且必须恰好出现4个完整角色。从左到右依次为：正面、严格90度正右侧面、背面、严格90度正左侧面。禁止增加斜向、四分之三角度、过渡角度、重复视角、小插图或额外动作帧。",
            _ => "严格使用3列3行九宫格布局，共9个尺寸完全一致的隐形单元格。中心格（第2行第2列）必须完全留空并保持透明，中心禁止出现角色、参考图缩略图、物体、标记、阴影或任何装饰。把八个方向严格放在外围八格，并全部朝向中间的空白格：第1行第1列面朝右下方45度；第1行第2列为正面朝向观者，能看到完整正脸；第1行第3列面朝左下方45度；第2行第1列面朝正右方，只看到右侧脸；第2行第3列面朝正左方，只看到左侧脸；第3行第1列面朝右上方45度；第3行第2列为背面朝向观者，只看到后脑勺与后背，没有五官；第3行第3列面朝左上方45度。八格必须各不相同，并围绕中心空格构成一周均匀递进的完整八方向。",
        };
        let placement = if direction_count == 8 {
            "九个隐形单元格尺寸必须完全一致。每个外围格只放一个角色，角色在格内水平和垂直居中，八个角色缩放比例完全一致；每一行内的脚底基线保持一致，角色高度约占单格高度的70%，中心空格四周的间距必须均匀。"
        } else {
            "每格尺寸完全一致，角色在每格中水平居中，所有角色脚底对齐同一条水平基线，角色高度约占每格高度的70%。"
        };
        let view = if direction_count == 8 {
            "固定相机规则：八个方向必须统一采用强等距风格的2.5D俯斜视角，也就是三分之四俯视的RPG游戏角色镜头。使用正交或近似正交投影，相机位于地平线上方至少45°，推荐45°到60°并向下俯拍；俯角绝不能低于45°，也不能接近平视。每个姿态都必须清楚呈现头顶或发顶、肩部顶面，并根据服装和动作显示衣物、手臂与脚部的上表面；整个人体从头到脚必须有明显的纵向透视缩短和前后层叠。画面必须一眼看出是等距或俯视游戏角色精灵，不是平面的正侧背角色转面图。角色保持直立，整个人体都必须服从同一俯视相机，不能只把头脸向下压而身体仍保持平视。所有格子的相机俯角、观察方向、投影方式和缩放比例完全一致，只让角色围绕自身竖直轴按指定顺序转向，不得转动相机。禁止平视、横版侧视、低机位仰视或垂直90度纯俯视。固定2.5D视角优先于用户描述或参考图中冲突的视角要求；仍须保持参考图原有的绘画或像素画风，不代表改成3D渲染。"
        } else {
            ""
        };
        let orientation_rules = match direction_count {
            3 => "正面必须看到完整五官；严格90度正右侧面只能看到一只眼睛，身体轮廓必须明显变窄；背面完全看不到五官，只有后脑勺与后背。这是正交角色三视图，不是斜向多角度转面图。",
            4 => "正面必须看到完整五官；两个严格90度正侧面都只能看到一只眼睛，身体轮廓必须明显变窄；背面完全看不到五官，只有后脑勺与后背。这是正交角色四向转面图，不得混入斜向视角。",
            _ => "正面必须看到完整五官；背面完全看不到五官，只有后脑勺与后背；正侧面只能看到一只眼睛，身体轮廓必须明显变窄；四个斜向介于正面与侧面之间，脸和身体都要有明确的透视压缩。",
        };
        format!(
            "用户描述：{request}\n\n角色多向图规范：仅将上传的参考图作为同一个角色的身份、发型、服装剪裁、配色、身高比例、体型和像素画风依据，禁止把参考图缩略图放入结果。当前选择的是{action_label}动作。{phase_rules} {action_rules} 每个指定视角只生成一个代表姿态，绝不能生成动画序列。{view}{layout} {placement} {orientation_rules} 所有格中的角色必须始终是同一个角色，只改变指定视角，同时保持所选动作状态和解剖学肢体相位一致，不得改变发型、服装、配色、体型或身高比例。整张图以及中心空格都使用完全透明背景。不要出现阴影、投影、地面、地面线、光晕、发光、特效、粒子或任何装饰元素。不要画网格线、边框、分隔线、单元格轮廓。不要出现文字、标签、数字、方向标注、箭头、说明、标志或水印。只输出最终透明角色多向图。"
        )
    }
}

#[cfg(test)]
mod side_scroll_map_tests {
    use super::*;

    #[test]
    fn prompt_requires_depth_and_an_invisible_horizontal_seam() {
        let prompt = compose_side_scroll_map_prompt("山水关卡", false);
        for required in ["横屏比例", "前景", "中景", "后景", "右边缘", "左边缘", "接缝必须不可见"] {
            assert!(prompt.contains(required));
        }
        assert!(prompt.starts_with("山水关卡"));
    }

    #[test]
    fn ratio_accepts_only_landscape_choices() {
        for ratio in ["3:2", "4:3", "16:9", "21:9"] {
            assert_eq!(normalize_side_scroll_map_ratio(ratio), ratio);
        }
        for ratio in ["1:1", "2:3", "3:4", "9:16", "9:21", ""] {
            assert_eq!(normalize_side_scroll_map_ratio(ratio), "21:9");
        }
    }

    #[test]
    fn request_does_not_inherit_hidden_workbench_instructions() {
        let prompt = build_side_scroll_map_generation_prompt(
            "map-only request",
            "21:9",
            "4K",
            PromptLanguage::English,
        );
        assert!(prompt.starts_with("map-only request"));
        assert!(prompt.contains("21:9"));
        assert!(prompt.contains("4K"));
        assert!(!prompt.contains("Generation controls:"));
        assert!(!prompt.contains("Reference image information:"));
        assert!(!prompt.contains("Negative prompt"));
    }
}

#[cfg(test)]
mod scene_composition_tests {
    use super::*;

    #[test]
    fn prompt_merges_every_reference_into_one_style_consistent_scene() {
        let prompt = compose_scene_composition_prompt("夜晚的魔法集市", false);
        for required in [
            "夜晚的魔法集市",
            "每张参考图都必须至少",
            "第一张参考图作为统一画风基准",
            "同一个环境中重新构建",
            "统一相机、视角、透视",
            "只输出一张铺满画布的最终场景",
            "禁止输出拼贴图",
            "水印",
        ] {
            assert!(prompt.contains(required), "missing scene-composition rule: {required}");
        }
    }

    #[test]
    fn english_prompt_restyles_conflicting_sources_instead_of_collaging_them() {
        let prompt = compose_scene_composition_prompt("Mix several different art styles", true);
        for required in [
            "Every uploaded reference must contribute",
            "first uploaded reference as the canonical art-style anchor",
            "redraw and reinterpret their content",
            "one consistent camera, viewpoint, perspective",
            "exactly one full-bleed finished scene",
            "Do not output a collage",
            "watermarks",
        ] {
            assert!(prompt.contains(required), "missing English scene-composition rule: {required}");
        }
    }
}

#[cfg(test)]
mod skill_icon_tests {
    use super::*;

    #[test]
    fn count_and_options_are_normalized_to_supported_values() {
        assert_eq!(normalize_skill_icon_count(2), 6);
        assert_eq!(normalize_skill_icon_count(14), 14);
        assert_eq!(normalize_skill_icon_count(24), 20);
        assert_eq!(normalize_skill_icon_background("black"), "black");
        assert_eq!(normalize_skill_icon_background("unknown"), "white");
        assert_eq!(normalize_skill_icon_shape("square"), "square");
        assert_eq!(normalize_skill_icon_shape("unknown"), "circle");
    }

    #[test]
    fn prompt_locks_count_style_background_shape_and_text_free_output() {
        let prompt = compose_skill_icon_prompt(
            "游戏类型：欧美MMORPG；技能类型：法术；技能描述：火焰、冰霜、治疗",
            14,
            "black",
            "square",
            false,
        );
        for required in [
            "全部已上传的1至8张参考图",
            "第一张参考图作为主要画风基准",
            "所有参考图的综合画风",
            "恰好14个",
            "5列、最多3行",
            "纯黑色（#000000）背景",
            "标准方形图标框",
            "禁止重复",
            "技能名称",
            "水印",
        ] {
            assert!(prompt.contains(required), "missing skill-icon rule: {required}");
        }
    }

    #[test]
    fn english_prompt_centers_an_incomplete_row_without_placeholders() {
        let prompt = compose_skill_icon_prompt("sci-fi MOBA support skills", 7, "white", "circle", true);
        for required in [
            "exactly 7 distinct",
            "4 columns and at most 2 rows",
            "center that row without adding placeholders",
            "pure white (#FFFFFF) background",
            "exact circular tile shape",
            "Do not draw visible grid lines",
        ] {
            assert!(prompt.contains(required), "missing English skill-icon rule: {required}");
        }
    }
}

#[cfg(test)]
mod character_multi_direction_tests {
    use super::*;

    #[test]
    fn count_accepts_only_three_four_or_eight_directions() {
        assert_eq!(normalize_character_multi_direction_count(3), 3);
        assert_eq!(normalize_character_multi_direction_count(4), 4);
        assert_eq!(normalize_character_multi_direction_count(8), 8);
        assert_eq!(normalize_character_multi_direction_count(0), 8);
        assert_eq!(normalize_character_multi_direction_count(12), 8);
    }

    #[test]
    fn action_accepts_the_three_supported_poses() {
        assert_eq!(normalize_character_multi_direction_action("standing"), "standing");
        assert_eq!(normalize_character_multi_direction_action("行走"), "walking");
        assert_eq!(normalize_character_multi_direction_action("running"), "running");
        assert_eq!(normalize_character_multi_direction_action("unknown"), "standing");
    }

    #[test]
    fn every_direction_is_locked_to_the_selected_action() {
        for direction_count in [3, 4, 8] {
            let standing = compose_character_multi_direction_prompt(
                "same hero", direction_count, "standing", false,
            );
            assert!(standing.contains(&format!(
                "全部{direction_count}个角色必须逐格处于明确的站立状态"
            )));
            assert!(standing.contains("禁止在不同视角切换承重腿"));
            assert!(standing.contains(&format!(
                "{direction_count}个角色必须全部保持同一个肢体相位站立"
            )));

            let walking = compose_character_multi_direction_prompt(
                "same hero", direction_count, "walking", false,
            );
            assert!(walking.contains(&format!(
                "全部{direction_count}个角色必须逐格处于明确的行走状态"
            )));
            assert!(walking.contains("不得只有正面、顶部或部分格子在行走"));
            assert!(walking.contains("双腿前后明显错开"));
            assert!(walking.contains("角色自身的左腿始终是向前迈出的前导腿"));
            assert!(walking.contains("禁止任何格子改成角色自身右脚向前"));
            assert!(walking.contains("禁止镜像动作、禁止交换左右腿"));
            assert!(walking.contains(&format!(
                "{direction_count}个角色必须全部以角色自身左脚向前的同一相位行走"
            )));

            let running = compose_character_multi_direction_prompt(
                "same hero", direction_count, "running", true,
            );
            assert!(running.contains(&format!(
                "all {direction_count} figures in every occupied cell must unmistakably be running"
            )));
            assert!(running.contains("do not make only the front view or top row run"));
            assert!(running.contains("long forceful stride"));
            assert!(running.contains("anatomical left leg is always the forward leading leg"));
            assert!(running.contains("No cell may lead with the anatomical right foot"));
            assert!(running.contains("Never mirror the pose, swap the left and right legs"));
            assert!(running.contains(&format!(
                "all {direction_count} figures must be running in the same anatomical left-foot-forward phase"
            )));
        }
    }

    #[test]
    fn eight_direction_prompt_keeps_the_requested_order_and_transparency_rules() {
        let prompt = compose_character_multi_direction_prompt("像素骑士", 8, "running", false);
        for required in [
            "严格使用3列3行九宫格",
            "中心格（第2行第2列）必须完全留空并保持透明",
            "第1行第1列面朝右下方45度",
            "第1行第2列为正面",
            "第1行第3列面朝左下方45度",
            "第2行第1列面朝正右方",
            "第2行第3列面朝正左方",
            "第3行第1列面朝右上方45度",
            "第3行第2列为背面",
            "第3行第3列面朝左上方45度",
            "每一行内的脚底基线保持一致",
            "奔跑动作",
            "完全透明背景",
            "不要出现阴影、投影、地面",
            "不要画网格线、边框、分隔线",
            "不要出现文字、标签、数字",
        ] {
            assert!(prompt.contains(required), "missing multi-direction rule: {required}");
        }
        assert!(!prompt.contains("所有角色脚底对齐同一条水平基线"));
        assert!(!prompt.contains("{count}"));
    }

    #[test]
    fn three_and_four_direction_prompts_use_single_row_layouts() {
        let three = compose_character_multi_direction_prompt("hero", 3, "standing", true);
        assert!(three.contains("strict 3-column by 1-row layout"));
        assert!(three.contains("exactly three character figures total"));
        assert!(three.contains("three and only three figures"));
        assert!(three.contains("cell 2 exact 90-degree right profile"));
        assert!(three.contains("cell 3 back view"));
        assert!(three.contains("Do not add a left profile, diagonal or three-quarter views"));
        assert!(!three.contains("diagonal views must be visually intermediate"));
        assert!(three.contains("align all feet to one shared horizontal baseline"));
        let four = compose_character_multi_direction_prompt("hero", 4, "walking", true);
        assert!(four.contains("strict 4-column by 1-row layout"));
        assert!(four.contains("exactly four character figures total"));
        assert!(four.contains("Do not add diagonal or three-quarter views"));
        assert!(!four.contains("diagonal views must be visually intermediate"));
        assert!(four.contains("walking pose"));
        assert!(four.contains("align all feet to one shared horizontal baseline"));
        assert!(!four.contains("{count}"));
    }

    #[test]
    fn chinese_three_view_prompt_allows_only_front_side_and_back() {
        let prompt = compose_character_multi_direction_prompt("像素骑士", 3, "standing", false);
        for required in [
            "只能出现且必须恰好出现3个完整角色",
            "第1格正面",
            "第2格为严格90度正右侧面",
            "第3格为背面",
            "禁止增加左侧面、斜向、四分之三角度",
            "角色总数必须等于3，多一个也不允许",
            "这是正交角色三视图，不是斜向多角度转面图",
        ] {
            assert!(prompt.contains(required), "missing three-view rule: {required}");
        }
        assert!(!prompt.contains("四个斜向介于正面与侧面之间"));
    }

    #[test]
    fn eight_direction_view_is_fixed_for_every_action_and_language() {
        for english in [false, true] {
            for count in [0, 3, 4, 8] {
                for action in ["standing", "walking", "running"] {
                    let prompt = compose_character_multi_direction_prompt(
                        "Use an eye-level camera / 使用平视镜头", count, action, english,
                    );
                    let fixed = count == 8 || count == 0;
                    assert_eq!(prompt.contains("2.5D"), fixed);
                    if fixed {
                        for rule in if english {
                            [
                                "isometric-style 2.5D elevated oblique game view",
                                "orthographic or near-orthographic projection",
                                "at least 45 degrees above the horizon",
                                "never fall below 45 degrees",
                                "vertical foreshortening and front-to-back depth stacking",
                                "not a flat front-side-back character turnaround",
                                "not the camera",
                                "overrides any conflicting viewpoint",
                                "fully transparent background",
                            ]
                        } else {
                            [
                                "强等距风格的2.5D俯斜视角",
                                "正交或近似正交投影",
                                "地平线上方至少45°",
                                "俯角绝不能低于45°",
                                "纵向透视缩短和前后层叠",
                                "不是平面的正侧背角色转面图",
                                "不得转动相机",
                                "优先于用户描述或参考图中冲突的视角要求",
                                "完全透明背景",
                            ]
                        } {
                            assert!(prompt.contains(rule), "missing fixed viewpoint rule: {rule}");
                        }
                    }
                }
            }
        }
    }
}

pub(super) fn append_negative_prompt_instruction(
    prompt: &str,
    negative_prompt: &str,
    language: PromptLanguage,
) -> String {
    let negative_prompt = negative_prompt.trim();
    if negative_prompt.is_empty() {
        return prompt.to_string();
    }
    if language == PromptLanguage::Chinese {
        format!("{prompt}\n\n反向提示词（画面中必须避免出现）：{negative_prompt}。")
    } else {
        format!("{prompt}\n\nNegative prompt (must not appear in the image): {negative_prompt}.")
    }
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

pub(super) fn append_category_generation_instruction(
    prompt: &str,
    category: &str,
    _language: PromptLanguage,
) -> String {
    if category != "ui" {
        return prompt.to_string();
    }

    format!(
        "{prompt}\n\nUI component atlas rule (mandatory): Create one clean 2D mobile RPG game UI sprite sheet on a flat warm-white background. Visual target: polished casual-game art, smooth solid color fills, crisp dark-navy outlines, simple two-step cel shading, small controlled highlights, rounded geometric shapes, consistent line weight, and clear colorful pictograms. Keep every surface visually smooth and every icon immediately readable at thumbnail size. Use the user's words only for theme, palette, and motif; keep this clean rendering unless the user explicitly names a different rendering method such as pixel art or flat vector art. Arrange about 40 isolated front-facing sprites in a balanced 6-column atlas with even white gutters. Include: four portrait frames; four health or energy bars; six inventory slots; six circular skill icons; four icon-only buttons; one virtual joystick; one minimap frame; four coins or gems; one settings gear; four treasure chests; and two blank dialog or inventory panels. Add a few matching keys, potions, status markers, or panel corners when space remains. Every element must be a separate reusable sprite with a complete silhouette and practical game function. Button faces stay blank and all communication is through icons. The canvas contains only isolated UI sprites and whitespace."
    )
}

pub(super) fn display_generation_prompt(prompt: &str) -> String {
    let normalized = prompt.replace("\r\n", "\n");
    let hidden_prefixes = [
        "生成控制：",
        "参数优先规则：",
        "UI 组件图集规则（必须遵守）：",
        "Generation controls:",
        "Parameter priority rule:",
        "UI component atlas rule (mandatory):",
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
