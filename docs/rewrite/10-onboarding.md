# ArtAIT Rust 重构资料：首启引导

结论：旧版"开应用就一堆页签"对新用户不友好，新版**首次启动走 4 步引导**，用户按需启用功能、配好目录、选好主题、配置首个 provider，进入主界面即可立刻产生价值。引导只在首次出现（`app_config.toml` 不存在），后续可在设置页重做。

## 触发条件

- 进入应用时检测 `%APPDATA%\ArtAIT\app_config.toml`。
  - 不存在 → 首启，进入引导。
  - 存在但缺 `[features]` 或版本号过旧 → 进入"补充配置"模式（只走需要的步骤）。
  - 完整 → 直接进主界面。
- 设置页提供"重新运行引导"按钮，用户随时回到引导（不删现有数据）。

## 引导流程（4 步）

### 第 1 步：选择功能预设

界面：

- 大标题："你想用 ArtAIT 做什么？"
- 副标题："可以多选或选预设，之后随时改。"
- 4 张卡片，鼠标悬停高亮，点击切换选中：

| 卡片 | 启用模块 | 适用场景 |
|------|---------|---------|
| **通用美术** | UI、场景、角色、特效、动作序列、图库 | 游戏 / 应用美术 |
| **动画短片** | 动画场景、动画角色、三视图、动画脚本、分镜板、图库 | 短片创作者 |
| **全功能** | 全部 | 不确定 / 都想用 |
| **自定义** | 弹折叠多选清单 | 高级用户 |

每张卡片显示：

- 图标 + 名称。
- 一句话定位。
- 包含的功能点（小图标网格）。

自定义卡片展开后是分组多选：

- 通用美术组：UI、场景、角色、特效、动作序列、图库。
- 动画短片组：动画场景、动画角色、三视图、动画脚本、分镜板。
- 共用：图库（默认勾选，建议保留）。

每项勾选框旁有 ⓘ 图标，鼠标悬停显示功能说明（避免新人看名词卡住）。

底部：上一步禁用、下一步按钮（至少选一项才可用）。

数据落点：

```toml
[features]
preset = "general"  # general | animation | full | custom
enabled = ["ui_concept", "scene", "character", "effect", "action_sequence", "asset_browser"]
```

### 第 2 步：确认目录

界面：

- 标题："选择工作目录"
- 副标题："这些目录用来存放你的输入素材和生成结果。可以使用默认值。"
- 三个目录字段（每个有"浏览…"按钮、"重置默认"链接）：

| 字段 | 默认值 | 说明 |
|------|-------|------|
| 输入素材目录 | `<我的文档>\ArtAIT\input` | 拖拽进来的图片暂存到这里 |
| 输出目录 | `<我的文档>\ArtAIT\out` | 所有生成结果保存到这里 |
| 提示词模板目录 | `<应用目录>\prompt` | 内置模板 + 用户模板 |

旧用户提示：

- 应用启动时若检测到旧 `config.json` 或当前工作目录下有 `out/`、`input/`、`apply_prompt/`、`prompt/`，第 2 步上方显示一条横幅：
  - "检测到旧版数据。点这里使用旧目录"。
  - 点击后字段自动填为旧目录路径，并在 `app_config.toml` 标记 `migrated_from = "..."`。

校验：

- 目录不存在则尝试创建；创建失败则给出错误并禁用下一步。
- 路径存在但不可写也禁用下一步。

数据落点：

```toml
[paths]
input  = "C:\\Users\\xxx\\Documents\\ArtAIT\\input"
output = "C:\\Users\\xxx\\Documents\\ArtAIT\\out"
prompt = "C:\\Users\\xxx\\Documents\\ArtAIT\\prompt"
```

### 第 3 步：选择主题

界面：

- 标题："界面外观"
- 副标题："之后可以在设置里随时改。"
- 三张主题卡片（带预览缩略图），点击切换并实时应用到引导窗口：
  - 深色（默认推荐）
  - 浅色
  - 跟随系统
- 字体下拉（默认 `Sarasa UI SC`，列出系统中文字体 + 内置字体）。
- 字号下拉（小 12 / 中 14 / 大 16）。

数据落点：

```toml
[ui]
theme = "dark"  # dark | light | system | user
font  = "Sarasa UI SC"
font-size = 14
```

### 第 4 步：配置首个 provider（可跳过）

界面：

- 标题："配置一个 AI 服务"
- 副标题："之后才能开始生成图片。也可以跳过，回头在设置里配。"
- 协议族下拉：
  - OpenAI 兼容
  - Gemini 兼容
  - Wavespeed 兼容
  - 自定义……
- 协议族选定后按 schema 渲染表单：
  - 实例名称（默认填协议族名 + 序号）
  - API 端点
  - API Key（secret input，写入 Credential Manager）
  - 模型列表（多选，预填协议族默认模型）
  - 适用范围（生图 / 推理 / 视频，按 capabilities 自动勾选）
- 连接测试按钮（异步，结果实时显示）。
- 跳过按钮（提示"未配置 provider 时无法生成，需在设置里完成配置"）。

校验：

- 必填字段全填且连接测试通过 → 完成按钮可用。
- 跳过 → 不写 provider 实例，进主界面后图标提示"未配置"。

数据落点：

```toml
[[provider_instances]]
id = "openai-compatible-1"
name = "我的 OpenAI 兼容服务"
family = "openai-compatible"
endpoint = "https://example.com/v1"
secret_ref = "artait/openai-compatible-1/api_key"
generation_models = ["gpt-image-1"]
scopes = ["generation"]
show_in_main_ui = true
```

## 完成后行为

- 写入 `app_config.toml`（含版本号 `schema_version = 1`）。
- 显示一个 1 秒的"完成"动画（勾号 + 渐隐）。
- 进入主界面，按 `[features].enabled` 渲染页签。
- 默认打开第一个启用的页签（按声明顺序）。
- 如果第 4 步配了 provider，主界面顶部模型选择自动选中该实例的第一个模型。
- 如果第 4 步跳过，顶部模型选择显示"未配置"，点击弹出"前往设置"提示。

## 状态保存与恢复

引导每完成一步即写入临时文件 `%APPDATA%\ArtAIT\.onboarding-draft.toml`，意外退出后下次启动从该步继续。所有步骤完成后才写入 `app_config.toml` 并删除草稿。

## "再次进入引导"模式

设置页"重新运行引导"按钮：

- 不删除现有 `app_config.toml`。
- 进入引导界面，但每步预填当前值。
- 完成后覆盖 `app_config.toml`。
- 不影响已有 provider 实例（第 4 步若用户改 provider，新增/修改实例而不是清空）。

## 错误与边缘情况

- 路径校验失败：保持当前步，显示错误，下一步禁用。
- API Key 校验失败：连接测试报错，允许用户继续重试或跳过。
- `notify` watcher 启动失败（罕见）：不阻塞引导，警告日志即可。
- 用户关掉引导窗口：草稿保留，下次启动恢复。
- 高 DPI 显示器：4 步窗口固定 720×540，DPI 缩放交给 Slint 自动处理。
- 仅一块屏幕的笔记本：窗口居中显示。

## Slint 实现

`pages/onboarding.slint`：

```slint
import { Theme } from "../theme.slint";
import { Button } from "../components/button.slint";

export component Onboarding {
    in-out property <int> step: 1;
    in-out property <OnboardingState> state;

    callback go-next();
    callback go-prev();
    callback finish();
    callback skip-provider();

    // ...

    if step == 1: StepFeatures { ... }
    if step == 2: StepDirectories { ... }
    if step == 3: StepTheme { ... }
    if step == 4: StepProvider { ... }
}
```

每个 step 独立组件，方便分别开发和测试。状态通过 `OnboardingState` struct 集中管理，Rust 端校验后 set 回 Slint。

## 测试要点

- 首次启动检测：`app_config.toml` 不存在 → 走引导。
- 草稿恢复：第 2 步写完关闭 → 重启从第 2 步继续。
- 旧目录检测：当前目录有 `out/` → 横幅出现。
- 跳过 provider：完成后 `app_config.toml` 无 provider 实例。
- 完成后页签按 enabled 渲染。
- 设置页"重新运行引导"不丢现有 provider。
- 主题在引导中实时切换。

## 文案原则

中文为主，简短直接，避免技术术语：

- ✅ "你想用 ArtAIT 做什么？"
- ❌ "请选择工作模式预设"
- ✅ "之后可以在设置里随时改。"
- ❌ "本配置将持久化至 app_config.toml，可于设置页面进行修改。"

按钮文案：

- 主按钮（下一步 / 完成）：实色，accent 色背景。
- 次按钮（上一步 / 跳过）：透明背景 + 边框。
- 跳过按钮放右下角，弱化但可见。

## MVP 实现顺序

1. 写 `OnboardingState` struct（Rust + Slint 共享）。
2. 写 `pages/onboarding.slint` 路由 + 步骤切换。
3. 写第 1 步（功能预设），用 mock 数据点亮路由。
4. 接 `app_config.toml` 写入逻辑。
5. 写第 2 步（目录），含旧目录检测。
6. 写第 3 步（主题），接 Theme global。
7. 写第 4 步（provider），按 schema 渲染。
8. 接草稿保存与恢复。
9. 接"重新运行引导"入口。
