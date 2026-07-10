# ArtAIT Rust 重构资料：UI 主题系统

结论：Slint 没有 CSS，但通过 **Theme global + 自建轻量组件层 + TOML 主题文件** 可以实现"颜色/圆角/字号/字体/间距运行时切换 + 用户自定义"，体验等价于改 CSS 变量。**结构性自定义（改布局、改控件层级）做不到，这是 Slint 的范式限制，文档要让用户预期对齐。**

## 设计目标

- 3 套预设：深色 / 浅色 / 跟随系统。
- 用户自定义主题（TOML 文件 + 设置页可视化编辑）。
- 运行时切换零延迟，不重启。
- 不引入 std-widgets，避免编译期主题污染体积。
- 中文字体优先，避免依赖系统字体差异。

## Slint 主题机制说明

Slint 的内置风格（fluent / material / cosmic / qt）是 **编译期锁定**，通过 `SLINT_STYLE` 环境变量或 `build.rs` 选一种。运行时无法切换内置风格（除非把多套都打进二进制）。

Slint 没有 CSS：没有选择器、没有级联、没有 `!important`。

**解决方案：** 用 Theme global + 全局属性绑定。Rust 端改 global 字段，全 UI 立即重绘。这套机制在 ArtAIT 范围内完全够用。

## Theme global 定义

`ui/theme.slint`：

```slint
export struct ThemePalette {
    bg: color,
    bg-elevated: color,
    bg-hover: color,
    bg-active: color,
    fg: color,
    fg-muted: color,
    fg-disabled: color,
    border: color,
    border-strong: color,
    accent: color,
    accent-hover: color,
    accent-active: color,
    success: color,
    warning: color,
    error: color,
    overlay: color,
}

export struct ThemeShape {
    radius-sm: length,
    radius-md: length,
    radius-lg: length,
    border-width: length,
}

export struct ThemeTypography {
    family: string,
    family-mono: string,
    size-xs: length,
    size-sm: length,
    size-md: length,
    size-lg: length,
    size-xl: length,
    weight-regular: int,
    weight-medium: int,
    weight-bold: int,
    line-height: float,
}

export struct ThemeSpacing {
    xs: length,
    sm: length,
    md: length,
    lg: length,
    xl: length,
    xxl: length,
}

export struct ThemeMotion {
    duration-fast: duration,
    duration-normal: duration,
    duration-slow: duration,
}

export global Theme {
    in-out property <ThemePalette> palette;
    in-out property <ThemeShape> shape;
    in-out property <ThemeTypography> typo;
    in-out property <ThemeSpacing> spacing;
    in-out property <ThemeMotion> motion;
    in-out property <string> id;
    in-out property <bool> is-dark;
}
```

所有自建组件读 `Theme.*`，运行时改 global 即时重绘。

## 主题 TOML 格式

```toml
id = "dark"
display-name = "深色"
is-dark = true

[palette]
bg            = "#1a1a1a"
bg-elevated   = "#242424"
bg-hover      = "#2e2e2e"
bg-active     = "#383838"
fg            = "#eaeaea"
fg-muted      = "#9a9a9a"
fg-disabled   = "#5a5a5a"
border        = "#333333"
border-strong = "#454545"
accent        = "#4a90e2"
accent-hover  = "#5ba3f5"
accent-active = "#3a7bc8"
success       = "#52c41a"
warning       = "#faad14"
error         = "#ff4d4f"
overlay       = "#000000aa"

[shape]
radius-sm    = 4
radius-md    = 8
radius-lg    = 12
border-width = 1

[typography]
family         = "Sarasa UI SC"
family-mono    = "Sarasa Mono SC"
size-xs        = 11
size-sm        = 12
size-md        = 14
size-lg        = 16
size-xl        = 20
weight-regular = 400
weight-medium  = 500
weight-bold    = 700
line-height    = 1.5

[spacing]
xs  = 4
sm  = 8
md  = 12
lg  = 16
xl  = 24
xxl = 32

[motion]
duration-fast   = "120ms"
duration-normal = "200ms"
duration-slow   = "300ms"
```

## 三套预设

### dark.toml

主色 `#4a90e2`，背景 `#1a1a1a`。生产工具默认色。

### light.toml

主色 `#1677ff`，背景 `#ffffff`，前景 `#1a1a1a`。

### system.toml

不含具体颜色，仅作为标识。运行时读 Windows `HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize\AppsUseLightTheme`，决定加载 `dark.toml` 或 `light.toml`。监听 `WM_SETTINGCHANGE`，系统切换时跟随。

三套预设 TOML 通过 `include_str!` 编译进二进制，避免依赖外部资源文件。

## 用户自定义

路径：`%APPDATA%\ArtAIT\themes\user.toml`

加载流程：

1. 启动读 `app_config.toml.theme.id`。
2. 若为 `dark` / `light` / `system`，用预设。
3. 若为 `user`，读 `themes/user.toml`，缺字段 fallback 到 dark 预设。
4. `notify` watch `themes/` 目录，文件变更即时重载。

设置页提供可视化编辑：

- 颜色字段用颜色选择器。
- 数值字段用滑块 + 输入框。
- 字体字段从系统字体列表 + 用户字体目录选择。
- 修改后即时预览（写到内存 Theme global，未保存）。
- 保存按钮写回 `user.toml`。

导入/导出：

- 导出当前主题为 TOML 文件。
- 从 TOML 文件导入并应用。
- 这是后期"主题市场/分享"的基础。

## 自建轻量组件层

不引入 std-widgets，自建约 10 个核心组件，全部读 Theme：

```text
ui/components/
├── button.slint        Button
├── icon-button.slint   IconButton
├── input.slint         Input、TextArea、SecretInput
├── select.slint        Select、Combo
├── card.slint          Card
├── list-item.slint     ListItem
├── dialog.slint        Dialog
├── tabs.slint          Tabs、TabPanel
├── tooltip.slint       Tooltip
├── progress.slint      Progress、Spinner
└── markdown-view.slint MarkdownView（基础）
```

每个组件示例（Button）：

```slint
import { Theme } from "../theme.slint";

export component Button {
    in property <string> text;
    in property <bool> primary: false;
    in property <bool> disabled: false;
    callback clicked();

    Rectangle {
        background: root.disabled
            ? Theme.palette.bg-hover
            : (root.primary
                ? (touch.has-hover ? Theme.palette.accent-hover : Theme.palette.accent)
                : (touch.has-hover ? Theme.palette.bg-hover : Theme.palette.bg-elevated));
        border-radius: Theme.shape.radius-md;
        border-width: root.primary ? 0px : Theme.shape.border-width;
        border-color: Theme.palette.border;
        animate background { duration: Theme.motion.duration-fast; }

        Text {
            text: root.text;
            color: root.primary ? white : Theme.palette.fg;
            font-family: Theme.typo.family;
            font-size: Theme.typo.size-md;
        }

        touch := TouchArea {
            enabled: !root.disabled;
            clicked => { root.clicked(); }
        }
    }
}
```

工程量评估：每个组件 50–150 行 .slint，10 个组件约 1000 行；样式集中在 Theme global，复用度高。

## Rust 端集成

`artait-app::theme`：

```rust
pub struct ThemeManager {
    current: RwLock<LoadedTheme>,
    watcher: notify::RecommendedWatcher,
    sys_listener: SysThemeListener,
}

impl ThemeManager {
    pub fn apply(&self, app: &AppWindow, theme: LoadedTheme) {
        let palette = theme.to_slint_palette();
        let shape   = theme.to_slint_shape();
        let typo    = theme.to_slint_typography();
        let spacing = theme.to_slint_spacing();
        let motion  = theme.to_slint_motion();

        app.global::<Theme>().set_palette(palette);
        app.global::<Theme>().set_shape(shape);
        app.global::<Theme>().set_typo(typo);
        app.global::<Theme>().set_spacing(spacing);
        app.global::<Theme>().set_motion(motion);
        app.global::<Theme>().set_id(theme.id.clone().into());
        app.global::<Theme>().set_is_dark(theme.is_dark);

        *self.current.write() = theme;
    }
}
```

callbacks 暴露：

- `set_theme(id)` — 切换预设或 user。
- `update_theme_field(field, value)` — 设置页实时预览。
- `save_user_theme()` — 写回 user.toml。
- `import_theme(path)` — 导入。
- `export_theme(path)` — 导出。

## 字体策略

中文字体差异在不同 Windows 安装下表现不一致。为保证一致性：

- 应用自带 `Sarasa UI SC` 子集字体（开源、覆盖中英）。
- 字体文件放 `assets/fonts/`，`include_bytes!` 嵌入，启动时通过 `slint::register_font_from_memory` 注册。
- 用户主题可以指定其他字体名（系统已安装 + 用户自定义字体目录）。
- 字体加载失败时 fallback 到 `Microsoft YaHei UI`。

## 限制说明

明确告诉用户能做什么、不能做什么：

**能做：**

- 颜色（背景、前景、强调色、状态色等）
- 圆角大小
- 边框宽度
- 字体族、字号、字重、行高
- 间距尺度
- 动画时长

**不能做：**

- 改某个特定按钮的样式（无选择器）
- 改控件的布局结构（声明式编译时已定）
- 媒体查询、响应式断点
- CSS 关键帧动画（slint 自己有 animation 语法，不是 CSS）
- 引入新的控件类型

如果用户对结构性自定义有强需求，未来可以考虑：

- 暴露布局参数（侧栏宽度、卡片密度、紧凑/宽松模式）作为 Theme 字段。
- 提供"布局 preset"选择，每个 preset 是预先写好的 .slint 变体。

但这不是 MVP 范围。

## 主题切换性能

- Theme global 字段变化触发 Slint 属性绑定重新求值。
- 自建组件直接读 Theme，无中间状态。
- 实测在万级元素页面（如图库网格）切换主题应在 50 ms 内完成重绘。
- 字体切换需要重新布局，可能产生 100–200 ms 闪烁，可接受。

## 测试策略

- 主题 TOML 解析单测：默认值、缺字段、非法颜色。
- 系统主题监听单测（mock 注册表读取）。
- Slint UI smoke：构造窗口，切换主题，断言 global 字段变化。
- 视觉回归（可选）：用 `slint::take_snapshot` 输出截图，对比基线。

## MVP 实现顺序

1. 写 `theme.slint`（Theme global 定义）。
2. 写 dark/light/system TOML 预设，编译期嵌入。
3. 写 `ThemeManager` 加载/应用。
4. 写最小 4 个自建组件（Button / Input / Card / Dialog），验证主题切换。
5. 设置页接入主题切换下拉。
6. 接入 `notify` 监听 `user.toml`。
7. 接入 Windows 系统主题监听。
8. 设置页接入可视化编辑器（后期）。
