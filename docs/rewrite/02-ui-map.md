# ArtAIT Rust 重构资料：UI 地图

结论：新版本是 **单入口 + 功能开关 + 多页签工作台 + 共享创作组件 + 后台任务反馈**。Rust 重构时页面按用户工作场景命名和拆分，不复刻 PySide 类名。UI 框架使用 Slint，单二进制 `ArtAITRust.exe`。

## 全局窗口结构

只有一个窗口基座（合并旧版的两个入口），具备以下行为：

- 加载 `app_config.toml`。
- 检测首启：配置不存在则进入引导（详见 `10-onboarding.md`）。
- 初始化主题（深色 / 浅色 / 跟随系统 / 用户自定义，详见 `09-ui-theming.md`）。
- 顶部页签右侧显示模型和设置控制。
- 使用懒加载页签减少启动成本。
- 保存上次打开的主页签。
- 单实例启动，外部图片路径通过 IPC 投递。
- 日志输出重定向到界面日志区。
- 后台任务关闭时进行线程收尾。

Slint 映射：

- `AppShell.slint`
- `AppState global`（持有当前路由、任务列表、provider 实例、主题、功能开关）
- `Theme global`（颜色 / 圆角 / 字体 / 间距）
- `services::config_store`、`services::single_instance`、`services::task_runner`

## 单入口与功能开关

旧版"通用生图"和"动画短片"两个 exe 合并为一个。功能模块由 `app_config.toml` 的 `[features]` 控制，UI 按开关动态显示页签：

```toml
[features]
preset = "general"
enabled = [
  "ui_concept",
  "scene",
  "character",
  "effect",
  "action_sequence",
  "asset_browser",
]
```

可选模块：

- `ui_concept` — UI 概念图
- `scene` — 创建场景
- `character` — 创建角色
- `effect` — 特效
- `action_sequence` — 动作序列
- `asset_browser` — 动作预览 / 图库
- `animation_scene` — 动画场景
- `animation_character` — 动画角色
- `character_turnaround` — 角色三视图
- `animation_script` — 动画脚本
- `storyboard` — 分镜板

预设：

| 预设 | 启用模块 |
|------|---------|
| 通用美术 | ui_concept, scene, character, effect, action_sequence, asset_browser |
| 动画短片 | animation_scene, animation_character, character_turnaround, animation_script, storyboard, asset_browser |
| 全功能 | 全部 |
| 自定义 | 用户多选 |

设置页提供"功能开关"区域，随时勾选；切换后立即生效，不需要重启。

## 顶部控制区

位于页签栏右侧，跨页面共享：

- 推理模型选择（图片分析、提示词分析、脚本生成）。
- 生图模型选择（场景、角色、UI、特效、动作图）。
- 视频模型选择（按需展开）。
- 主题按钮：循环切换或弹下拉选 dark/light/system/user。
- 设置按钮：打开设置页签。

界面特征：

- 模型下拉按 provider 实例分组显示。
- 实例标题不可选，模型项可选。
- 切换模型持久化到当前 provider 实例配置。
- 页面内显示当前 provider 和模型，确认任务提交目标。

Slint 映射：

- `components/TopModelBar.slint`
- callback `select_model_instance(scope, instance_id, model)`
- `AppState.provider_instances`、`AppState.selected_models`

## 设置窗口

设置不再是独立弹窗，改为主路由的一个"设置"页签（更小且符合 Slint 模式），内部分组：

- **基础**：输入/输出目录、主题、字体、MPV 路径。
- **功能开关**：模块勾选 + 预设切换。
- **生图**：provider 实例、模型、连接测试、专属配置。
- **生视频**：视频 provider 和模型。
- **推理**：分析 provider 和模型。
- **图床**：上传图片所需的图床配置。
- **图像处理**：通用图像参数与后处理。
- **提示词优化**：Prompt Optimizer Studio 启用、地址、轮询间隔、超时、健康状态。
- **去背景**：Rembg、PhotoRoom 等服务。
- **关于**：版本、日志路径、迁移工具入口。

界面特征：

- provider 支持多个实例，每实例有名称、协议族、适用范围、能力勾选、专属配置（按 schema 渲染）。
- 设置保存后刷新顶部模型选择。
- 连接测试由 provider 协议族提供，不写死在设置页。
- API Key 字段直接绑定本机配置文件，编辑节点时回显。

Slint 映射：

- `pages/settings.slint`
- `components/ProviderInstanceEditor.slint`
- callbacks `save_settings`、`test_provider_connection`、`set_feature_enabled`、`apply_theme`

## 主工作台页面

按功能开关启用，候选页面：

### UI 界面页（`ui_concept`）

定位：生成游戏或应用 UI 设计图。

输入：UI 描述 / 模板 / 参考图 / 比例 / 分辨率。

输出：`out/ui`。

`CreationMode::Ui`，`output_subdir = "ui"`。

### 创建场景页（`scene`）

定位：场景概念图、背景图、环境图。

输入：场景描述 / 模板 / 参考图 / 参数。

输出：`out/scenes`。

`CreationMode::Scene`。

### 创建角色页（`character`）

定位：角色设定图或成图。

特性：可上传风格参考图，调用推理 provider 分析生成提示词；结果可一键加入参考图迭代。

输出：`out/creations`。

`CreationMode::Character`。

### 特效页（`effect`）

定位：技能特效、附魔效果、精灵图、动作帧参考。

输出：`out/effects`，可去黑或去背景。

`CreationMode::Effect`。

### 动作序列页（`action_sequence`）

定位：根据角色图和动作参考素材，批量生成动作图。

界面结构：

- 左：角色图路径 + 选择按钮。
- 中：动作卡片网格（来自 `reference_action` 与动作配置）。
- 右：任务选项 — 强制覆盖提示词、强制覆盖图像、是否生成图像、是否锁定风格。
- 下：执行日志、进度条、开始、取消、打开输出目录。

输出：

- 提示词 → `apply_prompt/<角色名>/`
- 图片 → `out/<角色名>/`
- 外观分析 → `appearance.json` / `appearance.txt`

Slint 映射：`pages/action-sequence.slint`、callback `start_action_batch`。

### 动作预览 / 图库页（`asset_browser`）

定位：浏览本地输出目录，复用与后处理。

特性：

- 子目录选择。
- 缩略图网格 + 大图预览。
- 视频通过 MPV 调用。
- 右键菜单：打开位置、添加到参考图、去黑、去背景、删除。
- `notify` 监听文件变化自动刷新。

Slint 映射：`pages/asset-browser.slint`、`components/AssetGrid.slint`。

### 动画场景页（`animation_scene`）

与场景页同构，默认提示词模板和输出目录面向动画项目。

输出：`out/animation_scenes`。

`CreationMode::AnimationScene`。

### 动画角色页（`animation_character`）

与角色页同构，默认提示词强调动画角色可用性。

输出：`out/animation_characters`。

`CreationMode::AnimationCharacter`。

### 角色三视图页（`character_turnaround`）

定位：将角色设定标准化为正面、侧面、背面参考。

输入：角色设定文本 / 可选角色参考图。

输出：`out/character_turnarounds`。

`CreationMode::CharacterTurnaround`。

### 动画脚本页（`animation_script`）

定位：把主题、文档、参考图转成动画脚本 Markdown，并维护脚本列表与分镜包。

界面结构：

- 左侧输入：主题/故事想法、上传文档、上传参考图、清空、生成脚本。
- 右侧预览：脚本文件列表、打开、重命名、刷新、打开目录。
- 预览页签：脚本全文、分镜包。
- 底部：保存路径提示、发送到分镜板。

输出：

- 脚本 → `out/animation_scripts/<脚本名>.md`
- 分镜包 → `out/animation_scripts/_packages/<脚本名>/*.md`

注意：Slint 没有内置 Markdown 渲染。MVP 用 `pulldown-cmark` 解析 + 自定义 Slint 文本块渲染（标题/段落/列表/代码块）。复杂表格按 monospace 文本展示。

Slint 映射：`pages/script.slint`、`components/MarkdownView.slint`。

### 分镜板页（`storyboard`）

定位：把脚本片段或分镜需求转成分镜板视觉素材。

特性：

- 接收动画脚本页发送的文本，自动切换到本页。
- 默认使用分镜常用比例。
- 复用通用创作工作台能力。

输出：`out/storyboards`。

`CreationMode::Storyboard`，callback `send_script_to_storyboard`。

## 共享创作页面形态

多数创作页共用同一套体验：

- 提示词模板选择
- 提示词编辑
- 参考图列表（拖拽 / 文件选择 / 外部导入）
- 参数设置（比例 / 分辨率 / 模型）
- 优化提示词（普通 / 高级）
- 生成按钮 + 取消按钮
- 任务卡片（状态 / 日志 / 进度）
- 输出图库

Slint 抽象成可配置组件：

```slint
component CreationWorkspace inherits Rectangle {
  in property <CreationMode> mode;
  in property <string> title;
  in property <string> prompt-template-dir;
  in property <string> output-subdir;
  in property <string> file-prefix;
  in property <string> default-prompt;
  in property <FeatureFlags> enabled-features;
  // ...
}
```

不同页面只配置文案、默认模板目录、输出目录、可用功能与默认参数；逻辑共用。

Slint 映射：`pages/workspace.slint`、`components/PromptEditor.slint`、`components/ReferenceList.slint`、`components/TaskCard.slint`。

## 主题与字体

主题在运行时通过 `Theme global` 切换，三套预设 + 用户自定义 TOML 文件。详见 `09-ui-theming.md`。

字体随主题，默认中文优先 `Sarasa UI SC`，回退 `Microsoft YaHei UI`。字体文件可由用户在 `assets/fonts/` 提供并在主题里指定，避免依赖系统字体差异。

## 路由与导航

`AppState.current_page` 是路由真理源，`AppShell` 监听变化切换 `pages/*.slint`。页面切换不卸载组件实例（保留输入状态），但取消订阅事件以减少噪声。

外部图片导入：

- 单实例 IPC 收到路径后投递到 `AppState.import_intent`。
- 当前页是创作页 → 加入参考图。
- 当前页是动作序列页 → 第一张作为角色图。
- 当前页不支持图片输入 → 切到场景页并加入参考图。
- 非图片扩展名忽略；多张不覆盖已有参考图。
