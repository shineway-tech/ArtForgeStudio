use super::*;

pub(super) fn start_countdown(app_weak: Weak<AppWindow>) {
    let timer = Rc::new(slint::Timer::default());
    let timer_for_tick = timer.clone();
    timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_secs(1),
        move || {
            if let Some(app) = app_weak.upgrade() {
                let state = app.global::<AppState>();
                let value = state.get_auth_countdown();
                if value <= 0 {
                    timer_for_tick.stop();
                } else {
                    state.set_auth_countdown(value - 1);
                }
            } else {
                timer_for_tick.stop();
            }
        },
    );
}

pub(super) fn apply_theme(app: &AppWindow, theme: &str) {
    match theme {
        "sprite" => set_theme_palette(
            app,
            (236, 251, 244),
            (255, 255, 255),
            (224, 248, 238),
            (194, 235, 217),
            (7, 19, 15),
            (80, 98, 91),
            (141, 160, 150),
            (0, 217, 130),
            (6, 185, 111),
            (0, 200, 120),
            (245, 165, 36),
            (239, 105, 105),
        ),
        "light" => set_theme_palette(
            app,
            (250, 250, 252),
            (255, 255, 255),
            (244, 244, 248),
            (228, 228, 236),
            (31, 32, 48),
            (74, 76, 96),
            (138, 140, 160),
            (79, 70, 229),
            (67, 56, 202),
            (34, 197, 94),
            (245, 158, 11),
            (239, 68, 68),
        ),
        "ocean" | "blue" => set_theme_palette(
            app,
            (6, 11, 20),
            (12, 16, 28),
            (24, 36, 60),
            (24, 34, 58),
            (228, 236, 248),
            (184, 196, 216),
            (120, 144, 168),
            (14, 165, 233),
            (2, 132, 199),
            (52, 211, 153),
            (245, 158, 11),
            (239, 68, 68),
        ),
        "warm" => set_theme_palette(
            app,
            (12, 8, 6),
            (20, 14, 10),
            (36, 26, 16),
            (42, 30, 22),
            (244, 236, 220),
            (216, 196, 168),
            (156, 124, 88),
            (245, 158, 11),
            (217, 119, 6),
            (52, 211, 153),
            (245, 158, 11),
            (239, 68, 68),
        ),
        "forest" => set_theme_palette(
            app,
            (6, 14, 10),
            (12, 22, 16),
            (24, 36, 26),
            (24, 42, 30),
            (228, 244, 236),
            (184, 216, 196),
            (120, 156, 132),
            (34, 197, 94),
            (22, 163, 74),
            (52, 211, 153),
            (245, 158, 11),
            (239, 68, 68),
        ),
        "rose" => set_theme_palette(
            app,
            (12, 6, 8),
            (20, 10, 14),
            (36, 18, 24),
            (42, 24, 32),
            (244, 220, 228),
            (216, 180, 192),
            (156, 104, 120),
            (244, 63, 94),
            (225, 29, 72),
            (52, 211, 153),
            (245, 158, 11),
            (239, 68, 68),
        ),
        "cyber" => set_theme_palette(
            app,
            (10, 4, 16),
            (20, 8, 28),
            (42, 20, 56),
            (44, 18, 68),
            (244, 220, 248),
            (216, 180, 220),
            (160, 112, 172),
            (217, 70, 239),
            (168, 85, 247),
            (52, 211, 153),
            (245, 158, 11),
            (239, 68, 68),
        ),
        "oled" => set_theme_palette(
            app,
            (0, 0, 0),
            (8, 8, 8),
            (24, 24, 24),
            (26, 26, 26),
            (240, 240, 240),
            (184, 184, 184),
            (112, 112, 112),
            (16, 185, 129),
            (5, 150, 105),
            (52, 211, 153),
            (245, 158, 11),
            (239, 68, 68),
        ),
        "cream" => set_theme_palette(
            app,
            (242, 235, 224),
            (247, 240, 230),
            (235, 227, 213),
            (216, 207, 194),
            (58, 48, 37),
            (102, 90, 78),
            (158, 146, 134),
            (201, 107, 115),
            (163, 78, 88),
            (34, 197, 94),
            (245, 158, 11),
            (239, 68, 68),
        ),
        "system" => set_theme_palette(
            app,
            (236, 251, 244),
            (255, 255, 255),
            (224, 248, 238),
            (194, 235, 217),
            (7, 19, 15),
            (80, 98, 91),
            (141, 160, 150),
            (0, 217, 130),
            (6, 185, 111),
            (0, 200, 120),
            (245, 165, 36),
            (239, 105, 105),
        ),
        "user" => set_theme_palette(
            app,
            (250, 250, 252),
            (255, 255, 255),
            (244, 244, 248),
            (228, 228, 236),
            (31, 32, 48),
            (74, 76, 96),
            (138, 140, 160),
            (91, 95, 199),
            (67, 56, 202),
            (34, 197, 94),
            (245, 158, 11),
            (239, 68, 68),
        ),
        _ => set_theme_palette(
            app,
            (6, 6, 14),
            (12, 12, 28),
            (20, 20, 42),
            (24, 24, 56),
            (228, 228, 244),
            (184, 184, 204),
            (120, 120, 160),
            (79, 70, 229),
            (67, 56, 202),
            (52, 211, 153),
            (245, 158, 11),
            (239, 68, 68),
        ),
    }
}

pub(super) fn set_theme_palette(
    app: &AppWindow,
    bg: (u8, u8, u8),
    panel: (u8, u8, u8),
    panel_soft: (u8, u8, u8),
    border: (u8, u8, u8),
    text: (u8, u8, u8),
    muted: (u8, u8, u8),
    weak: (u8, u8, u8),
    accent: (u8, u8, u8),
    accent_dark: (u8, u8, u8),
    success: (u8, u8, u8),
    warning: (u8, u8, u8),
    danger: (u8, u8, u8),
) {
    let p = app.global::<AppTheme>();
    p.set_bg(rgb(bg));
    p.set_panel(rgb(panel));
    p.set_panel_soft(rgb(panel_soft));
    p.set_border(rgb(border));
    p.set_text(rgb(text));
    p.set_muted(rgb(muted));
    p.set_weak(rgb(weak));
    p.set_accent(rgb(accent));
    p.set_accent_dark(rgb(accent_dark));
    p.set_success(rgb(success));
    p.set_warning(rgb(warning));
    p.set_danger(rgb(danger));
}

pub(super) fn rgb((r, g, b): (u8, u8, u8)) -> slint::Color {
    slint::Color::from_rgb_u8(r, g, b)
}
