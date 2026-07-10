//! 主题与 UI 配置数据。
//!
//! 仅做数据描述，加载和应用逻辑在 artait-app::theme。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeId {
    Dark,
    Light,
    Ocean,
    Warm,
    Forest,
    Rose,
    Cyber,
    Oled,
    Cream,
    System,
    User,
}

impl Default for ThemeId {
    fn default() -> Self {
        ThemeId::Dark
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiConfig {
    pub theme: ThemeId,
    pub font_family: String,
    pub font_size: u32,
    pub locale: String,
    #[serde(default)]
    pub sidebar_collapsed: bool,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            theme: ThemeId::default(),
            font_family: "Sarasa UI SC".to_string(),
            font_size: 14,
            locale: "zh-CN".to_string(),
            sidebar_collapsed: false,
        }
    }
}

/// 主题文件 schema。dark/light/user 都使用这个结构反序列化。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThemeFile {
    pub id: String,
    pub display_name: String,
    pub is_dark: bool,
    pub palette: ThemePalette,
    pub shape: ThemeShape,
    pub typography: ThemeTypography,
    pub spacing: ThemeSpacing,
    pub motion: ThemeMotion,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThemePalette {
    pub bg: String,
    pub bg_elevated: String,
    pub bg_hover: String,
    pub bg_active: String,
    pub fg: String,
    pub fg_muted: String,
    pub fg_disabled: String,
    pub border: String,
    pub border_strong: String,
    pub accent: String,
    pub accent_hover: String,
    pub accent_active: String,
    pub success: String,
    pub warning: String,
    pub error: String,
    pub overlay: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThemeShape {
    pub radius_sm: u32,
    pub radius_md: u32,
    pub radius_lg: u32,
    pub border_width: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThemeTypography {
    pub family: String,
    pub family_mono: String,
    pub size_xs: u32,
    pub size_sm: u32,
    pub size_md: u32,
    pub size_lg: u32,
    pub size_xl: u32,
    pub weight_regular: u32,
    pub weight_medium: u32,
    pub weight_bold: u32,
    pub line_height: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThemeSpacing {
    pub xs: u32,
    pub sm: u32,
    pub md: u32,
    pub lg: u32,
    pub xl: u32,
    pub xxl: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThemeMotion {
    pub duration_fast: String,
    pub duration_normal: String,
    pub duration_slow: String,
}
