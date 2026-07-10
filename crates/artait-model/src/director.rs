//! 游戏资产导演级控制模型。

use serde::{Deserialize, Serialize};

use crate::CreationMode;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct DirectorControls {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purpose: Option<AssetPurpose>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_mood: Option<ColorMoodPreset>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub game_view: Option<GameViewPreset>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weather: Option<WeatherPreset>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_of_day: Option<TimeOfDayPreset>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lighting: Option<LightingPreset>,
}

impl DirectorControls {
    pub fn is_empty(&self) -> bool {
        self.purpose.is_none()
            && self.color_mood.is_none()
            && self.game_view.is_none()
            && self.weather.is_none()
            && self.time_of_day.is_none()
            && self.lighting.is_none()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetPurpose {
    SceneConcept,
    #[serde(rename = "tileset", alias = "tile_set")]
    TileSet,
    LevelDesignReference,
    PromoArt,
    LoadingArt,
    MiniMap,
    BuildingKit,
    CharacterPortrait,
    CharacterTurnaround,
    EightDirection,
    SpriteSheet,
    SpineParts,
    NpcAvatar,
    CharacterPoster,
    SkillEffect,
    BuffEffect,
    Explosion,
    SceneEffect,
    UiEffect,
    WeaponTrail,
    Hud,
    MainMenu,
    Inventory,
    Shop,
    Icon,
    LoadingUi,
    Dialog,
}

impl AssetPurpose {
    pub fn from_id(id: &str) -> Option<Self> {
        Some(match id {
            "scene_concept" => Self::SceneConcept,
            "tileset" => Self::TileSet,
            "level_design_reference" => Self::LevelDesignReference,
            "promo_art" => Self::PromoArt,
            "loading_art" => Self::LoadingArt,
            "mini_map" => Self::MiniMap,
            "building_kit" => Self::BuildingKit,
            "character_portrait" => Self::CharacterPortrait,
            "character_turnaround" => Self::CharacterTurnaround,
            "eight_direction" => Self::EightDirection,
            "sprite_sheet" => Self::SpriteSheet,
            "spine_parts" => Self::SpineParts,
            "npc_avatar" => Self::NpcAvatar,
            "character_poster" => Self::CharacterPoster,
            "skill_effect" => Self::SkillEffect,
            "buff_effect" => Self::BuffEffect,
            "explosion" => Self::Explosion,
            "scene_effect" => Self::SceneEffect,
            "ui_effect" => Self::UiEffect,
            "weapon_trail" => Self::WeaponTrail,
            "hud" => Self::Hud,
            "main_menu" => Self::MainMenu,
            "inventory" => Self::Inventory,
            "shop" => Self::Shop,
            "icon" => Self::Icon,
            "loading_ui" => Self::LoadingUi,
            "dialog" => Self::Dialog,
            _ => return None,
        })
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::SceneConcept => "scene_concept",
            Self::TileSet => "tileset",
            Self::LevelDesignReference => "level_design_reference",
            Self::PromoArt => "promo_art",
            Self::LoadingArt => "loading_art",
            Self::MiniMap => "mini_map",
            Self::BuildingKit => "building_kit",
            Self::CharacterPortrait => "character_portrait",
            Self::CharacterTurnaround => "character_turnaround",
            Self::EightDirection => "eight_direction",
            Self::SpriteSheet => "sprite_sheet",
            Self::SpineParts => "spine_parts",
            Self::NpcAvatar => "npc_avatar",
            Self::CharacterPoster => "character_poster",
            Self::SkillEffect => "skill_effect",
            Self::BuffEffect => "buff_effect",
            Self::Explosion => "explosion",
            Self::SceneEffect => "scene_effect",
            Self::UiEffect => "ui_effect",
            Self::WeaponTrail => "weapon_trail",
            Self::Hud => "hud",
            Self::MainMenu => "main_menu",
            Self::Inventory => "inventory",
            Self::Shop => "shop",
            Self::Icon => "icon",
            Self::LoadingUi => "loading_ui",
            Self::Dialog => "dialog",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::SceneConcept => "场景概念图",
            Self::TileSet => "TileSet",
            Self::LevelDesignReference => "地编参考",
            Self::PromoArt => "宣传图",
            Self::LoadingArt => "Loading 图",
            Self::MiniMap => "小地图",
            Self::BuildingKit => "建筑套件",
            Self::CharacterPortrait => "角色立绘",
            Self::CharacterTurnaround => "三视图",
            Self::EightDirection => "8 方向",
            Self::SpriteSheet => "SpriteSheet",
            Self::SpineParts => "Spine 拆件",
            Self::NpcAvatar => "NPC 头像",
            Self::CharacterPoster => "宣传海报",
            Self::SkillEffect => "技能特效",
            Self::BuffEffect => "Buff 特效",
            Self::Explosion => "爆炸",
            Self::SceneEffect => "场景特效",
            Self::UiEffect => "UI 特效",
            Self::WeaponTrail => "武器拖尾",
            Self::Hud => "HUD",
            Self::MainMenu => "主界面",
            Self::Inventory => "背包",
            Self::Shop => "商城",
            Self::Icon => "Icon",
            Self::LoadingUi => "Loading 界面",
            Self::Dialog => "弹窗",
        }
    }

    pub fn prompt_label(self) -> &'static str {
        match self {
            Self::SceneConcept => "game environment concept art, production reference",
            Self::TileSet => {
                "game tileset, modular repeatable environment assets, clean tile boundaries"
            }
            Self::LevelDesignReference => {
                "level design reference, clear spatial layout, navigable game area"
            }
            Self::PromoArt => "promotional key art, polished marketing composition",
            Self::LoadingArt => {
                "game loading screen illustration, clear focal point and readable composition"
            }
            Self::MiniMap => "game minimap design, top-down readable layout, simplified landmarks",
            Self::BuildingKit => {
                "modular building kit, reusable architecture parts, consistent scale"
            }
            Self::CharacterPortrait => "full body character illustration, game character design",
            Self::CharacterTurnaround => {
                "character turnaround sheet, front side back views, consistent proportions"
            }
            Self::EightDirection => {
                "8-direction character views, consistent silhouette, game production sheet"
            }
            Self::SpriteSheet => "sprite sheet, animation-ready frames, consistent character scale",
            Self::SpineParts => {
                "Spine animation separated parts, clean cutout character components"
            }
            Self::NpcAvatar => "NPC avatar portrait, readable face design, game dialogue icon",
            Self::CharacterPoster => "character poster key art, polished heroic composition",
            Self::SkillEffect => {
                "game skill VFX, transparent-friendly visual design, strong motion shape"
            }
            Self::BuffEffect => "game buff VFX, aura effect, readable status effect design",
            Self::Explosion => "game explosion VFX, impact burst, layered particles",
            Self::SceneEffect => "environmental VFX, atmosphere effect, game scene integration",
            Self::UiEffect => "UI VFX, interface feedback effect, clean glow and particles",
            Self::WeaponTrail => "weapon trail VFX, slash arc, readable attack direction",
            Self::Hud => "game HUD interface concept, readable hierarchy, production UI reference",
            Self::MainMenu => "game main menu UI concept, clear navigation and visual hierarchy",
            Self::Inventory => "game inventory UI, item grid, readable slots and categories",
            Self::Shop => "game shop UI, product cards, prices and purchase hierarchy",
            Self::Icon => "game icon design, centered object, clean silhouette",
            Self::LoadingUi => "loading screen UI layout, progress area, readable visual hierarchy",
            Self::Dialog => "game dialog modal UI, readable content area and buttons",
        }
    }

    pub fn default_for_mode(mode: CreationMode) -> Self {
        match mode {
            CreationMode::Ui => Self::Hud,
            CreationMode::Character | CreationMode::AnimationCharacter => Self::CharacterPortrait,
            CreationMode::CharacterTurnaround => Self::CharacterTurnaround,
            CreationMode::Effect => Self::SkillEffect,
            CreationMode::Storyboard => Self::SceneConcept,
            CreationMode::Scene | CreationMode::AnimationScene | CreationMode::ActionSequence => {
                Self::SceneConcept
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GameViewPreset {
    TopDown,
    #[serde(rename = "2_5d", alias = "two_point_five_d")]
    TwoPointFiveD,
    Isometric,
    SideView,
    ThirdPerson,
    FirstPerson,
    Orthographic,
}

impl GameViewPreset {
    pub fn from_id(id: &str) -> Option<Self> {
        Some(match id {
            "top_down" => Self::TopDown,
            "2_5d" => Self::TwoPointFiveD,
            "isometric" => Self::Isometric,
            "side_view" => Self::SideView,
            "third_person" => Self::ThirdPerson,
            "first_person" => Self::FirstPerson,
            "orthographic" => Self::Orthographic,
            _ => return None,
        })
    }

    pub fn prompt_label(self) -> &'static str {
        match self {
            Self::TopDown => "top-down game view, directly overhead camera",
            Self::TwoPointFiveD => "2.5D game perspective, slightly elevated camera, depth visible",
            Self::Isometric => "isometric game perspective, fixed orthographic angle",
            Self::SideView => "side view game perspective, horizontal gameplay plane",
            Self::ThirdPerson => "third person game camera, over the shoulder readable scene",
            Self::FirstPerson => "first person game perspective, immersive player viewpoint",
            Self::Orthographic => {
                "orthographic view, no perspective distortion, UI-friendly layout"
            }
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::TopDown => "Top Down",
            Self::TwoPointFiveD => "2.5D",
            Self::Isometric => "Isometric",
            Self::SideView => "Side View",
            Self::ThirdPerson => "Third Person",
            Self::FirstPerson => "First Person",
            Self::Orthographic => "Orthographic",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WeatherPreset {
    Sunny,
    Cloudy,
    Rainy,
    Storm,
    Snowy,
    Foggy,
    Sandstorm,
}

impl WeatherPreset {
    pub fn from_id(id: &str) -> Option<Self> {
        Some(match id {
            "sunny" => Self::Sunny,
            "cloudy" => Self::Cloudy,
            "rainy" => Self::Rainy,
            "storm" => Self::Storm,
            "snowy" => Self::Snowy,
            "foggy" => Self::Foggy,
            "sandstorm" => Self::Sandstorm,
            _ => return None,
        })
    }

    pub fn prompt_label(self) -> &'static str {
        match self {
            Self::Sunny => "sunny weather, clear sky",
            Self::Cloudy => "cloudy weather, overcast sky",
            Self::Rainy => "rainy weather, wet surfaces, visible rain",
            Self::Storm => "storm weather, dramatic clouds, strong wind and rain",
            Self::Snowy => "snowy weather, snow-covered environment",
            Self::Foggy => "foggy weather, mist, atmospheric depth",
            Self::Sandstorm => "sandstorm weather, dusty air, desert haze",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Sunny => "晴天",
            Self::Cloudy => "阴天",
            Self::Rainy => "雨天",
            Self::Storm => "暴风雨",
            Self::Snowy => "雪天",
            Self::Foggy => "雾天",
            Self::Sandstorm => "沙尘",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimeOfDayPreset {
    Dawn,
    Noon,
    Dusk,
    BlueHour,
    Night,
}

impl TimeOfDayPreset {
    pub fn from_id(id: &str) -> Option<Self> {
        Some(match id {
            "dawn" => Self::Dawn,
            "noon" => Self::Noon,
            "dusk" => Self::Dusk,
            "blue_hour" => Self::BlueHour,
            "night" => Self::Night,
            _ => return None,
        })
    }

    pub fn prompt_label(self) -> &'static str {
        match self {
            Self::Dawn => "early morning dawn light",
            Self::Noon => "midday lighting, clear visibility",
            Self::Dusk => "sunset dusk lighting, warm horizon glow",
            Self::BlueHour => "blue hour lighting, cool twilight atmosphere",
            Self::Night => "deep night scene, controlled readable darkness",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Dawn => "清晨",
            Self::Noon => "正午",
            Self::Dusk => "黄昏",
            Self::BlueHour => "蓝调时刻",
            Self::Night => "深夜",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LightingPreset {
    SoftNatural,
    Cinematic,
    DreamGlow,
    HighContrast,
    Volumetric,
    Neon,
}

impl LightingPreset {
    pub fn from_id(id: &str) -> Option<Self> {
        Some(match id {
            "soft_natural" => Self::SoftNatural,
            "cinematic" => Self::Cinematic,
            "dream_glow" => Self::DreamGlow,
            "high_contrast" => Self::HighContrast,
            "volumetric" => Self::Volumetric,
            "neon" => Self::Neon,
            _ => return None,
        })
    }

    pub fn prompt_label(self) -> &'static str {
        match self {
            Self::SoftNatural => "soft natural lighting, gentle shadows",
            Self::Cinematic => {
                "dramatic lighting, high contrast, cinematic composition, volumetric lighting"
            }
            Self::DreamGlow => "dreamy glowing light, soft bloom, magical atmosphere",
            Self::HighContrast => "high contrast lighting, strong light and shadow separation",
            Self::Volumetric => "volumetric lighting, visible light shafts, atmospheric depth",
            Self::Neon => "neon lighting, colorful emissive signs, cyber glow",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::SoftNatural => "柔和自然光",
            Self::Cinematic => "电影感",
            Self::DreamGlow => "梦幻发光",
            Self::HighContrast => "高对比",
            Self::Volumetric => "体积光",
            Self::Neon => "霓虹灯",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColorMoodPreset {
    WarmHealing,
    ColdOppressive,
    HighSaturation,
    LowSaturation,
    DarkFantasy,
    Cyberpunk,
    JapaneseFantasy,
    GhibliLike,
}

impl ColorMoodPreset {
    pub fn from_id(id: &str) -> Option<Self> {
        Some(match id {
            "warm_healing" => Self::WarmHealing,
            "cold_oppressive" => Self::ColdOppressive,
            "high_saturation" => Self::HighSaturation,
            "low_saturation" => Self::LowSaturation,
            "dark_fantasy" => Self::DarkFantasy,
            "cyberpunk" => Self::Cyberpunk,
            "japanese_fantasy" => Self::JapaneseFantasy,
            "ghibli_like" => Self::GhibliLike,
            _ => return None,
        })
    }

    pub fn prompt_label(self) -> &'static str {
        match self {
            Self::WarmHealing => "warm healing color grading, cozy and gentle palette",
            Self::ColdOppressive => "cold oppressive color grading, desaturated blue-gray mood",
            Self::HighSaturation => "high saturation color palette, vivid game art colors",
            Self::LowSaturation => "low saturation color palette, restrained realistic mood",
            Self::DarkFantasy => "dark fantasy color mood, deep shadows, ominous atmosphere",
            Self::Cyberpunk => "cyberpunk color grading, neon magenta cyan palette",
            Self::JapaneseFantasy => "Japanese fantasy art style, elegant anime game palette",
            Self::GhibliLike => "Ghibli-like warm painterly fantasy mood, soft natural colors",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::WarmHealing => "温暖治愈",
            Self::ColdOppressive => "冷峻压抑",
            Self::HighSaturation => "高饱和",
            Self::LowSaturation => "低饱和",
            Self::DarkFantasy => "暗黑风",
            Self::Cyberpunk => "赛博朋克",
            Self::JapaneseFantasy => "日系幻想",
            Self::GhibliLike => "吉卜力风",
        }
    }
}
