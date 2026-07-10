# ArtAIT Rust 重构资料：产品与边界概览

本文档面向 Rust 重构开发。结论先行：ArtAIT 不是一个单页脚本工具，而是一个以桌面界面为外壳、以多 provider 生图/推理服务为核心、以本地素材目录为工作空间的 AI 美术生产套件。Rust 重构时按"产品功能域 + 界面工作流 + provider 合约 + 本地资产模型"拆分，而不是按现有 Python 文件逐行搬迁。

## 当前产品定位

旧版由两个桌面入口和一个独立后端组成：

- 通用生图工具（PySide6）：UI 图、场景图、角色图、特效图、动作序列、结果预览。
- 动画短片工具（PySide6）：动画场景、动画角色、角色三视图、脚本生成、分镜板。
- Prompt Optimizer Studio：独立 Rust/Axum 服务，被 GUI 通过 HTTP 调用。

产品的核心价值不是"调一次 AI 生图接口"，而是把本地参考素材、提示词模板、provider 设置、后台任务、生成结果和二次处理动作组织成可重复的美术生产流程。

## 重构后的产品形态

新版本是 **单二进制 `ArtAITRust.exe`，单入口，功能模块化**：

- 旧版的"通用生图"和"动画短片"合并为一个应用，按用户在首启引导的选择启用功能模块。
- 4 套预设：通用美术 / 动画短片 / 全功能 / 自定义。
- 每个模块对应一个工作页签，支持运行时启用 / 禁用，无需重启。
- UI 框架：Slint（直接原生渲染，无 WebView），自建轻量组件层。
- 后端：Rust workspace 多 crate，`tokio` 异步运行时。
- 平台：仅 Windows（10/11）。
- 配置：TOML，API Key 直接保存到本机配置文件。
- 数据持久化：文件 + 内存懒索引；缩略图缓存到 `%LOCALAPPDATA%`。

非功能目标：

- release exe 体积 ≤ 12 MB。
- 启动到首屏 ≤ 200 ms。
- 长任务不阻塞 UI。
- 主题运行时切换零延迟。
- provider 错误可诊断、密钥不明文散落。

## 旧系统分层（仅供迁移参考）

```text
main.py
  -> artait_generate.py        通用生图桌面入口（旧版）
  -> artait_animation.py       动画短片桌面入口（旧版）

artait_generate.py / artait_animation.py
  -> gui_common.py             共享窗口、页签、设置、任务、图库、提示词组件
  -> generate_actions.py       动作序列、批量提示词、图片生成编排
  -> core/                     配置、上传、视频、任务状态
  -> providers/                AI provider 插件层
  -> prompting/                外观分析、动作提示词构建
  -> prompt/                   提示词模板资源
  -> reference_action/         动作参考图
  -> reference_prompt/         动作提示词模板

providers/prompt_optimizer.py
  -> Back/                     Rust/Axum Prompt Optimizer Studio
```

## 新系统分层

```text
ArtAITRust.exe
  └ artait-app (slint UI + main)
       ↓ callbacks
       artait-service ── 业务编排
         ↓
         artait-task ── TaskRunner、取消、事件总线
         ↓
         artait-provider ── trait、Registry、能力查询
         ↓
         artait-providers ── 协议族实现（OpenAI/Gemini/Wavespeed/...）
         ↓
         artait-config ── AppConfig、secret_store
         ↓
         artait-model ── 数据类型，零依赖
       artait-asset ── 资产懒索引、缩略图、文件监听
  └ Back/ (sidecar，可选)
       Prompt Optimizer Studio （Rust/Axum）
```

详见 `07-rust-architecture.md`。

## 重构范围

Rust 重构覆盖：

- 桌面应用外壳与导航：单入口、主页签、设置页、任务面板、图库预览、首启引导。
- 业务服务：生成任务、动作批处理、提示词模板、参考图管理、视频任务、去背景/去黑。
- provider 合约：生图、角色生成、视频生成、图片分析、连接测试、任务轮询。
- 本地数据模型：配置、provider 实例、提示词模板、生成任务、素材资产、输出索引。
- 主题系统：3 套预设 + 用户自定义 TOML，运行时切换。
- 与 Prompt Optimizer Studio 的集成：保留独立 sidecar，通过 client 调用。

不纳入主重构：

- `input/`、`out/`、`apply_prompt/` 中的历史运行结果。
- `build/`、`Back/target/`、`__pycache__/` 等构建和缓存产物。
- `prompt/create_character_prompt/chivesAI` 第三方资源。
- `config.json` 中的真实密钥内容；只迁移结构。

## 功能域划分

### 1. 创作工作台

负责用户输入创作意图、选择提示词模板、添加参考图、设置尺寸比例、提交生成任务并预览结果。

涵盖功能模块：UI 概念、创建场景、创建角色、特效、动画场景、动画角色、角色三视图、分镜板。

Rust 映射：

- `pages/workspace.slint`（按 `CreationMode` 复用）
- `commands::create_generation_task`
- `services::generation`
- `services::asset_library`
- `models::PromptTemplate`
- `models::GenerationTask`

### 2. 动作序列工作台

从角色图出发，发现动作模板，上传角色图和动作参考图，抽取角色外观，批量生成每个动作的提示词和图像。

Rust 映射：

- `pages/action-sequence.slint`
- `services::action_batch`
- `services::appearance_profile`
- `services::prompt_builder`
- `models::ActionDefinition`
- `models::ActionBatchJob`

### 3. 动画脚本与分镜

把主题、文档和参考图转成动画脚本 Markdown，并自动拆分为分镜包。

Rust 映射：

- `pages/script.slint`
- `pages/storyboard.slint`
- `services::script_generation`
- `services::storyboard_package`
- `models::AnimationScript`
- `models::StoryboardPackage`

### 4. Provider 与任务运行时

连接不同 AI 服务商，并把异步任务、轮询、取消、错误和结果保存统一抽象。

Rust 映射：

- `artait-provider::Provider`
- `artait-provider::ProviderRegistry`
- `artait-task::TaskRunner`
- `artait-task::TaskEvent`
- `models::TaskStatus`
- `models::ProviderInstance`

### 5. 配置、密钥与本地资产

provider 实例、模型选项、目录路径、主题、字体、图床、去背景服务、Prompt Optimizer 设置等持久化。

Rust 映射：

- `services::config_store`
- `services::secret_store`
- `services::filesystem`
- `artait-asset::AssetLibrary`
- `models::AppConfig`
- `models::Asset`

### 6. UI 外壳与主题

`AppShell` 路由、顶栏模型选择、设置页、首启引导、Theme global 切换。

Rust 映射：

- `ui/main.slint`、`ui/theme.slint`
- `pages/onboarding.slint`、`pages/settings.slint`
- `services::theme_loader`
- `services::feature_flags`

## 当前产品体验特征

ArtAIT 是生产工具型工作台，重构后保留这些特征：

- 顶部角落长期显示模型和设置控制。
- 主区域用页签切换工作模式。
- 多数创作页面共享同一种"提示词 + 参考图 + 参数 + 生成 + 图库"体验。
- 长任务都进入后台，UI 通过状态文本、日志、进度条、结果卡片反馈。
- 结果以本地目录为主，用户可以打开文件、打开目录、添加到参考图、去黑、去背景、删除。
- provider 和模型不是固定选项，而是配置驱动的实例列表。

新版增加：

- 单一入口替代两套独立 exe。
- 首启引导让用户按需启用功能。
- 主题运行时切换 + 用户自定义。
- 单二进制安装更轻。

## 关键设计结论

1. **采用 Slint 桌面 UI，不用 Tauri/WebView。**
   理由：体积小、启动快、单二进制、原生渲染。代价：Markdown 自渲，但量可控。

2. **不逐类搬迁 PySide 组件。**
   PySide 类混合了界面构建、状态处理和业务调用。重构先抽功能域、数据模型、command/service 边界，再实现新 UI。

3. **Provider 合约优先稳定。**
   生图、角色生成、视频生成、分析、任务轮询、连接测试是公共能力。trait 稳定后页面迁移和任务运行时才能并行推进。详见 `06-provider-contract.md`。

4. **Provider 配置驱动 + 编译期注册。**
   内置若干"协议族"（OpenAI 兼容、Gemini 兼容、Wavespeed 兼容等），用户在配置里新增实例不需要发版；新增协议族需要发版。

5. **Prompt Optimizer Studio 先按独立 sidecar 保留。**
   `Back/` 已是 Rust/Axum + SQLite + worker。第一阶段通过 HTTP 调用，主应用稳定后再评估合并。

6. **配置集中到 TOML。**
   当前 MVP 为方便本机调试和中转站切换，API Key 直接保存在本机 `app_config.toml`，设置页可回显。

7. **单二进制 + 体积优化。**
   `opt-level = "z"` + `lto = "fat"` + `panic = "abort"` + `strip = "symbols"`，目标 ≤ 12 MB。
