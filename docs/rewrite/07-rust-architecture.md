# ArtAIT Rust 重构资料：Rust 目标架构

结论：目标架构是 **Slint + Rust 单二进制 + Cargo workspace 多 crate**。Prompt Optimizer Studio 第一阶段保留为独立 Rust/Axum sidecar，主应用通过 client 调用。仅 Windows，配置驱动 + 编译期注册 provider，文件 + 懒索引存储，TOML 配置。

## 架构目标

功能目标：

- 合并通用生图与动画短片为单一入口，按用户选择启用功能模块。
- 保留顶部模型控制、多页签工作台、提示词模板、参考图、图库、动作序列、脚本分镜与后处理能力。
- 保留 provider 多实例配置。
- 保留 Prompt Optimizer 高级优化。
- 支持运行时主题切换（深色 / 浅色 / 跟随系统 / 用户自定义）。

非功能目标：

- 单二进制 `ArtAITRust.exe`，release 体积目标 ≤ 12 MB。
- 启动到首屏 ≤ 200 ms。
- 长任务不阻塞 UI。
- provider 错误可诊断。
- 密钥不明文散落。
- 业务服务可单元测试。
- UI 与 provider 实现解耦。

## 技术栈

| 类别 | 选型 | 备注 |
|------|------|------|
| UI | `slint` 1.8+ | 自建轻量组件层，避开 std-widgets 编译期主题锁 |
| 异步运行时 | `tokio` | 多线程 + sync + fs + time |
| HTTP | `reqwest` + `rustls-tls` | 不依赖系统 OpenSSL |
| 错误（库） | `thiserror` | 类型化错误 |
| 错误（应用） | `anyhow` | 顶层 |
| 日志 | `tracing` + `tracing-appender` | 文件轮转 |
| 配置 | `toml` + `serde` | |
| 密钥 | `toml` + `serde` | 本机配置文件保存，设置页可回显 |
| 文件监听 | `notify` | 图库懒索引刷新 |
| 单实例 | `single-instance` | 外部图片导入 |
| 取消 | `tokio_util::sync::CancellationToken` | |
| 测试 | `mockall` + `wiremock` + `insta` | trait mock + HTTP mock + 快照 |

## Workspace 骨架

```text
ArtAITRust/
├── Cargo.toml                     workspace 根
├── crates/
│   ├── artait-model/              数据类型，零异步零 IO
│   ├── artait-config/             AppConfig + secret_store
│   ├── artait-provider/           Provider trait + Registry + schema
│   ├── artait-providers/          内置 provider 协议族实现
│   ├── artait-task/               TaskRunner + 取消 + 事件总线
│   ├── artait-asset/              资产懒索引 + 缩略图 + 监听
│   ├── artait-service/            业务编排（生成 / 动作批 / 脚本 / 优化）
│   └── artait-app/                slint UI + main，唯一二进制产物
├── ui/                            .slint 文件
├── schemas/                       provider 配置 JSON schema
├── themes/                        预设主题 TOML
├── assets/                        图标、字体
└── docs/rewrite/                  规格文档
```

依赖方向（自顶向下，禁止反向）：

```
app → service → task → provider → config → model
                  ↘ asset ↗
```

`model` 零依赖；`provider` 不知道 `service`/`task`；`app` 唯一依赖 slint。

## 单二进制策略

只有 `artait-app` 产生可执行文件 `ArtAITRust.exe`。release profile：

```toml
[profile.release]
opt-level = "z"
lto = "fat"
codegen-units = 1
strip = "symbols"
panic = "abort"
```

合并入口后省一份代码、一份配置、一份图库、一份任务面板。功能开关存在 `app_config.toml` 的 `[features]` 表里，UI 按开关动态隐藏页签。

## Slint UI 分层

```text
ui/
├── main.slint              AppShell，路由、顶栏、侧栏
├── theme.slint             Theme global，颜色/圆角/字体绑定
├── components/             自建轻量组件
│   ├── button.slint
│   ├── input.slint
│   ├── card.slint
│   ├── list-item.slint
│   ├── dialog.slint
│   └── ...
├── pages/
│   ├── onboarding.slint    首次启动引导
│   ├── workspace.slint     通用创作工作台（按 mode 复用）
│   ├── action-sequence.slint
│   ├── script.slint        动画脚本
│   ├── storyboard.slint    分镜板
│   ├── asset-browser.slint
│   └── settings.slint
└── icons.slint             SVG 图标资源
```

Slint 与 Rust 的桥接：

- 全局状态用 `Theme global`、`AppState global`，Rust 端通过 `set_xxx` 推送。
- 后台任务事件通过 `invoke_from_event_loop` 回到 UI 线程。
- 长列表（图库、动作卡片）用 `ListView` + `for` + 回调式数据源。

## 主题系统

预设 3 套（编译期打入）：

- `dark.toml`
- `light.toml`
- `system.toml`（运行时读 `AppsUseLightTheme` 决定走 dark/light）

用户自定义：`%APPDATA%\ArtAIT\themes\user.toml`，`notify` watch 即时生效。

主题字段（颜色 / 圆角 / 字号 / 字体 / 间距）由 Rust 加载后写入 Slint Theme global，全 UI 立即重绘。详见 `09-ui-theming.md`。

## 首启引导

`app_config.toml` 不存在视为首启，进入引导。4 步：

1. 选择功能预设（通用美术 / 动画短片 / 全功能 / 自定义）。
2. 确认输入/输出目录。
3. 选择主题。
4. 至少配置一个生图 provider（可跳过，跳过后首次生成时再要求）。

引导写完生成 `app_config.toml`，下次启动直接进主界面。详见 `10-onboarding.md`。

## Tauri-style command 边界（slint 端）

slint 没有 Tauri 的 invoke 机制。约定：

- UI 通过 `Callback<Args, Ret>` 触发请求。
- Rust 端在 `app` 层注册 callback handler。
- handler 不直接写业务，转发到 `artait-service`。
- 长任务返回 `task_id`，进度通过 `AppState global` 上的 task 列表推送。

请求入口示例：

- `list_provider_instances(scope)`
- `select_model(instance_id, scope, model)`
- `create_generation_task(request)`
- `cancel_task(task_id)`
- `list_assets(domain)`
- `postprocess_asset(asset_id, action)`
- `start_action_batch(request)`
- `generate_animation_script(request)`
- `split_storyboard_packages(script_id)`
- `optimize_prompt(request)`
- `save_settings(settings_patch)`
- `set_feature_enabled(feature, enabled)`
- `apply_theme(theme_id)`

## 事件模型

后台任务通过 `tokio::sync::broadcast` 广播事件，`app` 层订阅并写入 Slint global。

事件：

- `task.started`
- `task.progress`
- `task.log`
- `task.output_created`
- `task.round_update`
- `task.completed`
- `task.failed`
- `task.cancelled`
- `asset.changed`
- `settings.changed`
- `theme.changed`

## 任务运行时

`artait-task::TaskRunner`：

- 单例，`Arc<TaskRunner>` 注入到 service 层。
- `tokio::sync::Semaphore` 控并发上限。
- 每个任务携带 `CancellationToken`。
- 事件通过 `broadcast::Sender<TaskEvent>` 推送。
- provider 调用、文件保存、错误归一化都在 runner 内。
- MVP 不做任务持久化；崩溃后冷启动重置。后期评估 SQLite。

## 配置与密钥架构

普通配置：

- `%APPDATA%\ArtAIT\app_config.toml`
- 字段：路径、主题、字体、provider 实例元信息、API Key、功能开关、最近一次主页签、image_processing、prompt_optimizer、video_player。

密钥：

- 直接保存到本机配置文件，设置页编辑节点时回显。
- `secret_ref` 只用于兼容旧凭据。
- 日志统一脱敏。

迁移：

- 读取旧 `config.json` 结构，生成新 `app_config.toml` 草稿。
- 不自动写入旧密钥；提示用户在迁移向导里确认导入到 Credential Manager。

## Provider 架构

Provider 分三层：

1. `ProviderMeta` 静态描述 + 配置 schema。
2. 单一 `Provider` trait + 能力查询方法（`as_image_generator()` 等返回 `Option<&dyn ...>`）。
3. provider 适配器：请求构造、响应解析、错误转换。

UI 不直接知道具体 provider 文件，也不硬编码 provider ID。

配置驱动：用户在设置里基于"协议族"（`openai-compatible`、`gemini-compatible`、`wavespeed-compatible` 等）创建实例，填 endpoint / 模型列表 / 密钥即可使用，不需要重新发版。详见 `06-provider-contract.md`。

## 本地资产架构

`artait-asset::AssetLibrary`：

- 启动只扫元数据（文件名、mtime、大小、kind）。
- 缩略图按需生成，缓存到 `%LOCALAPPDATA%\ArtAIT\thumbnails\`。
- `notify` 监听输出目录，增量刷新。
- 执行打开文件、打开目录、删除、添加到参考图、去黑、去背景。
- 通过 `asset.changed` 事件通知 UI 刷新。

第一阶段不上 SQLite。当资产数量、搜索、标签需求出现时再评估引入。

## Prompt Optimizer 架构

第一阶段：

- 保留 `Back/` 作为独立 Rust 服务。
- 主应用通过 `artait-providers` 中的 `PromptOptimizerClient` 调用 HTTP API。
- Slint 应用启动时检测端口，必要时拉起 sidecar 进程。

第二阶段评估：合并到 workspace 或继续保留。

## 错误与日志

错误分层：

- `UserError`：用户可修复（缺输入、未配 provider）。
- `ProviderError`：服务商错误、超时、限流。
- `IoError`：文件读写失败。
- `ConfigError`：配置解析或字段缺失。
- `InternalError`：程序缺陷。

日志规则：

- 所有 crate 用 `tracing`。
- UI 只显示简明日志。
- 调试日志写 `%LOCALAPPDATA%\ArtAIT\logs\`，按天轮转。
- API Key、Authorization、Cookie、签名 URL 必须脱敏。

## 测试策略

Level 1 — 核心服务单元测试

- 配置解析、provider schema、提示词模板解析、分镜包拆分、动作发现、路径解析。

Level 2 — provider 契约测试

- 用 `wiremock` 起本地 HTTP server。
- 验证请求构造、响应解析、错误转换、base64/URL/task_id 提取。

Level 3 — 任务运行测试

- 用 `mockall` mock provider trait。
- 任务成功、异步轮询、取消、超时、保存失败。

Level 4 — UI smoke

- slint 单测窗口直接构造，触发 callback，断言 global 状态变化。

## 架构决策记录

### ADR-001：UI 用 Slint，不用 Tauri

理由：体积更小（目标 ≤12 MB），启动更快（≤200 ms），无 WebView 依赖。代价是 Markdown 预览要自渲或调外部，工程量可控。

### ADR-002：Workspace 多 crate

理由：骨架强壮、依赖方向清晰、可独立测试、未来 provider 可拆仓。

### ADR-003：Provider 单 trait + 能力查询

理由：避免 `Arc<dyn Provider>` 无法 downcast 到子 trait 的问题。比 trait 继承更友好。

### ADR-004：Prompt Optimizer 保留独立 sidecar

理由：已是 Rust + Axum，边界清晰，第一阶段降低风险。

### ADR-005：MVP 文件 + 懒索引，不上 SQLite

理由：用户数据规模通常在万级以下，文件 + `notify` 已够用。SQLite 等明确瓶颈再加。

### ADR-006：合并入口，功能开关驱动

理由：通用与动画两套页面同构度极高，合并后省维护、状态共享、首启引导即产品介绍。
