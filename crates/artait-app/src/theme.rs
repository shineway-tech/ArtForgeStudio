//! 主题加载、应用、系统跟随、用户自定义文件监听。

use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use artait_model::{ThemeFile, ThemeId};
use notify::{Event, EventKind, RecursiveMode, Watcher};
use slint::{Color, ComponentHandle, Weak};

use crate::ui::{
    AppShell, Theme as SlintTheme, ThemePalette, ThemeShape, ThemeSpacing, ThemeTypography,
};

const DARK_TOML: &str = include_str!("../../../themes/dark.toml");
const LIGHT_TOML: &str = include_str!("../../../themes/light.toml");
const OCEAN_TOML: &str = include_str!("../../../themes/ocean.toml");
const WARM_TOML: &str = include_str!("../../../themes/warm.toml");
const FOREST_TOML: &str = include_str!("../../../themes/forest.toml");
const ROSE_TOML: &str = include_str!("../../../themes/rose.toml");
const CYBER_TOML: &str = include_str!("../../../themes/cyber.toml");
const OLED_TOML: &str = include_str!("../../../themes/oled.toml");
const CREAM_TOML: &str = include_str!("../../../themes/cream.toml");

/// 加载结果。返回主题数据 + 实际使用的 ThemeId（system 时会变成 Dark/Light）。
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct LoadedTheme {
    pub file: ThemeFile,
    pub effective: ThemeId,
}

/// 加载预设。`System` 会读注册表决定深/浅；`User` 失败时 fallback 到 `Dark`。
pub fn load(id: ThemeId) -> LoadedTheme {
    match id {
        ThemeId::Dark => LoadedTheme {
            file: parse(DARK_TOML),
            effective: ThemeId::Dark,
        },
        ThemeId::Light => LoadedTheme {
            file: parse(LIGHT_TOML),
            effective: ThemeId::Light,
        },
        ThemeId::Ocean => LoadedTheme {
            file: parse(OCEAN_TOML),
            effective: ThemeId::Ocean,
        },
        ThemeId::Warm => LoadedTheme {
            file: parse(WARM_TOML),
            effective: ThemeId::Warm,
        },
        ThemeId::Forest => LoadedTheme {
            file: parse(FOREST_TOML),
            effective: ThemeId::Forest,
        },
        ThemeId::Rose => LoadedTheme {
            file: parse(ROSE_TOML),
            effective: ThemeId::Rose,
        },
        ThemeId::Cyber => LoadedTheme {
            file: parse(CYBER_TOML),
            effective: ThemeId::Cyber,
        },
        ThemeId::Oled => LoadedTheme {
            file: parse(OLED_TOML),
            effective: ThemeId::Oled,
        },
        ThemeId::Cream => LoadedTheme {
            file: parse(CREAM_TOML),
            effective: ThemeId::Cream,
        },
        ThemeId::System => {
            let dark = system_prefers_dark();
            let raw = if dark { DARK_TOML } else { LIGHT_TOML };
            let mut file = parse(raw);
            file.id = "system".into();
            file.display_name = if dark {
                "跟随系统 · 深色"
            } else {
                "跟随系统 · 浅色"
            }
            .into();
            LoadedTheme {
                file,
                effective: if dark { ThemeId::Dark } else { ThemeId::Light },
            }
        }
        ThemeId::User => match read_user_theme() {
            Some(file) => LoadedTheme {
                file,
                effective: ThemeId::User,
            },
            None => {
                tracing::info!("user.toml 不存在或无效，回退到深色");
                LoadedTheme {
                    file: parse(DARK_TOML),
                    effective: ThemeId::Dark,
                }
            }
        },
    }
}

fn parse(s: &str) -> ThemeFile {
    toml::from_str(s).expect("内置主题文件解析失败")
}

fn read_user_theme() -> Option<ThemeFile> {
    let path = user_theme_path()?;
    let raw = std::fs::read_to_string(&path).ok()?;
    match toml::from_str::<ThemeFile>(&raw) {
        Ok(f) => Some(f),
        Err(e) => {
            tracing::warn!(error = %e, path = %path.display(), "解析 user.toml 失败");
            None
        }
    }
}

pub fn user_theme_path() -> Option<PathBuf> {
    Some(
        artait_model::portable_data_dir()
            .join("themes")
            .join("user.toml"),
    )
}

/// 把当前主题应用到 Slint Theme global。
pub fn apply(app: &AppShell, loaded: &LoadedTheme) {
    let theme = app.global::<SlintTheme>();
    theme.set_palette(to_slint_palette(&loaded.file.palette));
    theme.set_shape(to_slint_shape(&loaded.file.shape));
    theme.set_typo(to_slint_typo(&loaded.file.typography));
    theme.set_spacing(to_slint_spacing(&loaded.file.spacing));
    theme.set_id(loaded.file.id.clone().into());
    theme.set_is_dark(loaded.file.is_dark);
}

/// 把用户自定义字体/字号覆写到 Slint Theme 全局，覆盖主题文件内置值。
pub fn apply_font_overrides(app: &AppShell, cfg: &artait_model::AppConfig) {
    let theme = app.global::<SlintTheme>();
    let mut typo = theme.get_typo();
    typo.family = cfg.ui.font_family.clone().into();
    typo.size_md = cfg.ui.font_size as f32;
    typo.size_sm = cfg.ui.font_size.saturating_sub(2) as f32;
    typo.size_xs = cfg.ui.font_size.saturating_sub(3) as f32;
    typo.size_lg = (cfg.ui.font_size + 2) as f32;
    typo.size_xl = (cfg.ui.font_size + 4) as f32;
    theme.set_typo(typo);
}

/// 主题菜单循环：dark → light → system → user → dark
pub fn next_id(current: ThemeId) -> ThemeId {
    match current {
        ThemeId::Dark => ThemeId::Ocean,
        ThemeId::Ocean => ThemeId::Warm,
        ThemeId::Warm => ThemeId::Forest,
        ThemeId::Forest => ThemeId::Rose,
        ThemeId::Rose => ThemeId::Cyber,
        ThemeId::Cyber => ThemeId::Oled,
        ThemeId::Oled => ThemeId::Light,
        ThemeId::Light => ThemeId::Cream,
        ThemeId::Cream => ThemeId::System,
        ThemeId::System => ThemeId::User,
        ThemeId::User => ThemeId::Dark,
    }
}

pub fn id_label(id: ThemeId) -> &'static str {
    match id {
        ThemeId::Dark => "深紫",
        ThemeId::Ocean => "深海蓝",
        ThemeId::Warm => "暖墨棕",
        ThemeId::Forest => "森林绿",
        ThemeId::Rose => "暗玫瑰",
        ThemeId::Cyber => "赛博紫",
        ThemeId::Oled => "纯黑",
        ThemeId::Light => "素白",
        ThemeId::Cream => "奶白",
        ThemeId::System => "跟随系统",
        ThemeId::User => "自定义",
    }
}

pub fn id_from_str(id: &str) -> Option<ThemeId> {
    match id {
        "dark" => Some(ThemeId::Dark),
        "ocean" => Some(ThemeId::Ocean),
        "warm" => Some(ThemeId::Warm),
        "forest" => Some(ThemeId::Forest),
        "rose" => Some(ThemeId::Rose),
        "cyber" => Some(ThemeId::Cyber),
        "oled" => Some(ThemeId::Oled),
        "light" => Some(ThemeId::Light),
        "cream" => Some(ThemeId::Cream),
        "system" => Some(ThemeId::System),
        "user" => Some(ThemeId::User),
        _ => None,
    }
}

pub fn id_str(id: ThemeId) -> &'static str {
    match id {
        ThemeId::Dark => "dark",
        ThemeId::Ocean => "ocean",
        ThemeId::Warm => "warm",
        ThemeId::Forest => "forest",
        ThemeId::Rose => "rose",
        ThemeId::Cyber => "cyber",
        ThemeId::Oled => "oled",
        ThemeId::Light => "light",
        ThemeId::Cream => "cream",
        ThemeId::System => "system",
        ThemeId::User => "user",
    }
}

#[cfg(windows)]
fn system_prefers_dark() -> bool {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let key =
        match hkcu.open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize") {
            Ok(k) => k,
            Err(_) => return true, // 默认深色
        };
    // AppsUseLightTheme: 0 = 深, 1 = 浅
    match key.get_value::<u32, _>("AppsUseLightTheme") {
        Ok(1) => false,
        _ => true,
    }
}

#[cfg(not(windows))]
fn system_prefers_dark() -> bool {
    true
}

/// 监听 user.toml 变化。当当前主题是 `User` 时自动重载。
pub fn spawn_user_theme_watcher(
    rt: &tokio::runtime::Handle,
    app_weak: Weak<AppShell>,
    current_is_user: Arc<AtomicBool>,
) -> Option<notify::RecommendedWatcher> {
    let path = user_theme_path()?;
    let dir = path.parent()?.to_path_buf();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::warn!(error = %e, "创建 themes/ 目录失败");
        return None;
    }

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<()>();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
        if let Ok(event) = res {
            if matches!(
                event.kind,
                EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
            ) {
                let _ = tx.send(());
            }
        }
    })
    .ok()?;

    if let Err(e) = watcher.watch(&dir, RecursiveMode::NonRecursive) {
        tracing::warn!(error = %e, "watch themes/ 失败");
        return None;
    }
    tracing::info!("watching {}", dir.display());

    let app_weak_w = app_weak;
    rt.spawn(async move {
        while let Some(()) = rx.recv().await {
            if !current_is_user.load(Ordering::Relaxed) {
                continue;
            }
            tracing::info!("user.toml 变更，重载");
            // 防抖：编辑器写入往往多个事件，等 200ms 合并
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            while rx.try_recv().is_ok() {}

            let loaded = load(ThemeId::User);
            let app_weak = app_weak_w.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(app) = app_weak.upgrade() {
                    apply(&app, &loaded);
                }
            });
        }
    });

    Some(watcher)
}

fn parse_color(s: &str) -> Color {
    let h = s.trim_start_matches('#');
    let (r, g, b, a) = match h.len() {
        6 => (
            u8::from_str_radix(&h[0..2], 16).unwrap_or(0),
            u8::from_str_radix(&h[2..4], 16).unwrap_or(0),
            u8::from_str_radix(&h[4..6], 16).unwrap_or(0),
            255u8,
        ),
        8 => (
            u8::from_str_radix(&h[0..2], 16).unwrap_or(0),
            u8::from_str_radix(&h[2..4], 16).unwrap_or(0),
            u8::from_str_radix(&h[4..6], 16).unwrap_or(0),
            u8::from_str_radix(&h[6..8], 16).unwrap_or(255),
        ),
        _ => (0, 0, 0, 255),
    };
    Color::from_argb_u8(a, r, g, b)
}

fn to_slint_palette(p: &artait_model::ThemePalette) -> ThemePalette {
    ThemePalette {
        bg: parse_color(&p.bg),
        bg_elevated: parse_color(&p.bg_elevated),
        bg_hover: parse_color(&p.bg_hover),
        bg_active: parse_color(&p.bg_active),
        fg: parse_color(&p.fg),
        fg_muted: parse_color(&p.fg_muted),
        fg_disabled: parse_color(&p.fg_disabled),
        border: parse_color(&p.border),
        border_strong: parse_color(&p.border_strong),
        accent: parse_color(&p.accent),
        accent_hover: parse_color(&p.accent_hover),
        accent_active: parse_color(&p.accent_active),
        success: parse_color(&p.success),
        warning: parse_color(&p.warning),
        error: parse_color(&p.error),
        overlay: parse_color(&p.overlay),
    }
}

fn to_slint_shape(s: &artait_model::ThemeShape) -> ThemeShape {
    ThemeShape {
        radius_sm: s.radius_sm as f32,
        radius_md: s.radius_md as f32,
        radius_lg: s.radius_lg as f32,
        border_width: s.border_width as f32,
    }
}

fn to_slint_typo(t: &artait_model::ThemeTypography) -> ThemeTypography {
    ThemeTypography {
        family: t.family.clone().into(),
        family_mono: t.family_mono.clone().into(),
        size_xs: t.size_xs as f32,
        size_sm: t.size_sm as f32,
        size_md: t.size_md as f32,
        size_lg: t.size_lg as f32,
        size_xl: t.size_xl as f32,
        weight_regular: t.weight_regular as i32,
        weight_medium: t.weight_medium as i32,
        weight_bold: t.weight_bold as i32,
    }
}

fn to_slint_spacing(s: &artait_model::ThemeSpacing) -> ThemeSpacing {
    ThemeSpacing {
        xs: s.xs as f32,
        sm: s.sm as f32,
        md: s.md as f32,
        lg: s.lg as f32,
        xl: s.xl as f32,
        xxl: s.xxl as f32,
    }
}
