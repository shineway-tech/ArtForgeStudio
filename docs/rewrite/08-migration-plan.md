# ArtAIT Rust 重构资料：迁移计划

结论：迁移按 **骨架 → 配置/密钥/Provider Registry → 单图生成 → 通用工作台 → 图库后处理 → 动作序列 → 动画脚本 → Prompt Optimizer → 兼容迁移工具** 推进。栈：Slint + Rust workspace + 仅 Windows + 配置驱动 provider + 文件懒索引 + TOML。MVP 先做高频闭环，复杂功能往后排。

## 总体原则

- 先功能闭环，再视觉细节。
- 先共享能力，再具体页面。
- 先 mock provider，再真实 provider。
- 旧目录第一阶段兼容读取，新目录模型并行引入。
- 旧 `config.json` 只迁移结构，不复制密钥。
- `input/`、`out/`、`apply_prompt/` 视为用户数据，不当源码迁移。
- `prompt/create_character_prompt/chivesAI` 视为第三方资源，不纳入主仓。
- 单二进制 `ArtAITRust.exe`，release 体积目标 ≤ 12 MB。

## 阶段 0：清点与冻结接口

目标：把现有行为整理成可开发规格。

任务：

- 完成 `docs/rewrite/` 全套 10 篇文档。
- 锁定栈：Slint + Rust + Windows + TOML + 文件 + 懒索引。
- 锁定合并入口与功能开关方案（详见 `02-ui-map.md`、`10-onboarding.md`）。
- 锁定主题方案（详见 `09-ui-theming.md`）。
- 锁定 provider trait 设计（详见 `06-provider-contract.md`）。

验收：

- 新开发者能凭文档解释产品结构，不需要打开 Python 源码。
- workspace 骨架方案已就绪。

## 阶段 1：Workspace 骨架

目标：建立可启动的桌面应用骨架。

任务：

- `cargo init` workspace，创建 8 个 crate：
  - `artait-model`
  - `artait-config`
  - `artait-provider`
  - `artait-providers`
  - `artait-task`
  - `artait-asset`
  - `artait-service`
  - `artait-app`
- 配置 `[profile.release]` 体积优化（`opt-level = "z"`、`lto = "fat"`、`strip = "symbols"`、`panic = "abort"`、`codegen-units = 1`）。
- `artait-app` 跑通最小 Slint 主窗口，显示空白 `AppShell`。
- 写 `.gitignore`、`rustfmt.toml`、`clippy.toml`。
- 写 `windows.toml`（应用图标、版本信息）。
- CI 只跑 `cargo build --release` 和 `cargo clippy`。

验收：

- `cargo build --release` 通过。
- `target/release/ArtAITRust.exe` 能弹空白窗口。
- 体积基线记录（预期 5–8 MB 空骨架）。

## 阶段 2：模型层与配置/密钥

目标：把数据类型和配置/密钥跑通。

任务：

- `artait-model`：定义 `AppConfig`、`ProviderInstance`、`PromptTemplate`、`GenerationTask`、`Asset`、`ActionDefinition`、`AnimationScript`、`StoryboardPackage`、`AppearanceProfile`、`TaskEvent`、`ProviderError` 等核心类型。
- `artait-config`：实现 TOML 加载/保存、字段默认值、错误恢复模式。
- `artait-config::secret_store`：基于 `keyring` 封装写入/读取/删除/列出。
- 写读取旧 `config.json` 的兼容解析器（只做结构映射，不读密钥）。
- 单测覆盖：默认值、缺字段恢复、密钥脱敏、旧配置映射。

验收：

- `AppConfig` 能 round-trip。
- 配置文件部分字段缺失时给默认值，不崩。
- secret 通过 `secret_ref` 引用，不出现在 TOML 里。
- 旧配置迁移工具能输出 dry-run 报告。

## 阶段 3：Provider Registry 与 mock provider

目标：让 provider 实例和模型选择跑通，业务先用 mock 闭环。

任务：

- `artait-provider`：定义 `Provider` trait、能力子 trait、`ProviderMeta`、`ProviderContext`、`ProviderError`、`ProviderRegistry`、`HttpClient` 抽象。
- `artait-providers::mock`：实现 mock provider，返回固定结果，模拟延迟和错误。
- `artait-providers::openai_compatible`：第一个真实协议族实现（仅生成）。
- `artait-providers::gemini_compatible`：第二个真实协议族（仅分析）。
- 实现 `instantiate_from_family`，配置驱动实例化。
- 实现连接测试 callback。
- 写 provider 契约测试（`wiremock` 起本地 server）。

验收：

- 设置页能新增 mock 实例并选中。
- 顶部模型下拉按实例分组显示。
- 切换模型持久化。
- 连接测试结果可见。
- 日志/错误不出现密钥。

## 阶段 4：任务运行时与事件总线

目标：把后台任务、取消、事件推送跑通。

任务：

- `artait-task::TaskRunner`：单例，`tokio::Semaphore` 控并发，`broadcast` 推送 `TaskEvent`。
- 任务 lifecycle：`validating → uploading → submitted → polling → saving → completed`，含 `cancelling/cancelled/failed`。
- `CancellationToken` 全链路传递。
- `ResultSaver`：统一处理 `File / Url / Base64 / AsyncTask` 四种 `GenerationOutput`。
- Slint 端：`AppState global` 持有任务列表，`invoke_from_event_loop` 桥接事件。

验收：

- 提交 mock 任务能在 UI 看到状态变化。
- 取消能停止后续阶段。
- 失败有错误显示。
- 超时按预期触发。

## 阶段 5：首启引导与主题系统

目标：完成首次启动体验和主题切换。

任务：

- 实现 `pages/onboarding.slint`：4 步流程（功能预设 / 目录 / 主题 / provider 配置）。
- `Theme global` + 3 套预设 TOML（dark/light/system）。
- 用户主题加载：`%APPDATA%\ArtAIT\themes\user.toml`，`notify` watch。
- 系统跟随：读 Windows `AppsUseLightTheme` 注册表，监听 `WM_SETTINGCHANGE`。
- 自建轻量组件层（Button / Input / Card / ListItem / Dialog 等约 10 个）全部读 Theme。

验收：

- 首启走完引导生成 `app_config.toml`。
- 主题运行时切换，零延迟。
- 用户改 `user.toml` 保存即时生效。
- 系统切换深浅色应用跟随。

## 阶段 6：单图生成闭环

目标：完成一个创作页面的最小可用闭环。建议先做创建场景。

任务：

- 实现 `pages/workspace.slint` 配置化骨架（`CreationMode`、文案、模板目录、输出目录）。
- 实现 `components/PromptEditor.slint`、`components/ReferenceList.slint`、`components/TaskCard.slint`。
- 实现 `commands::create_generation_task`（callback → service）。
- 接入 mock + 一个真实生图 provider。
- 实现输出保存到 `out/scenes`。
- 实现 `notify` 监听后自动刷新图库占位。

验收：

- 输入提示词能生成图片。
- 文件保存到正确目录。
- 错误、取消、重试可用。
- UI 不卡顿。

## 阶段 7：通用创作页面扩展

目标：把通用工作台覆盖多个 `CreationMode`。

任务：

- 配置 UI / 创建角色 / 特效 / 动画场景 / 动画角色 / 角色三视图 / 分镜板的 `CreationMode`。
- 实现模板目录映射（`prompt/<domain>`）。
- 实现参考图分析生成提示词。
- 实现提示词模板创建/编辑弹窗。
- 实现普通提示词优化。
- 接入 Prompt Optimizer client（高级优化）。
- 实现功能开关动态显隐页签。

验收：

- 所有创作页面共享同一套任务和图库逻辑。
- 不同页面输出到正确目录。
- 提示词模板能创建、编辑、加载。
- 关闭某模块在设置里立即生效。

## 阶段 8：图库、后处理、视频

目标：完成本地资产浏览与常用后处理。

任务：

- `artait-asset::AssetLibrary`：扫描元数据，缩略图按需生成并缓存到 `%LOCALAPPDATA%\ArtAIT\thumbnails\`。
- `notify` 监听输出目录，增量刷新。
- 实现 `pages/asset-browser.slint`：子目录选择、缩略图网格、大图预览、右键菜单。
- 实现打开文件、打开目录、删除、添加到参考图。
- 实现去黑（图像处理服务）。
- 实现 Rembg / PhotoRoom 去背景（provider 协议族）。
- 实现 MPV 调用播放视频。

验收：

- 图库不重启即可刷新。
- 缩略图缓存命中率高。
- 后处理动作有明确成功/失败反馈。
- 删除动作有确认。

## 阶段 9：动作序列批处理

目标：迁移高价值的复杂流程。

任务：

- 实现动作发现（扫描 `reference_action/` 与 `reference_prompt/`）。
- 实现 `pages/action-sequence.slint`：角色图、动作卡片网格、运行选项。
- 实现上传策略（图床 / provider binary / base64 / 本地文件）。
- 实现外观分析（推理 provider 抽取 `AppearanceProfile`）。
- 实现动作提示词构建（`prompt_builder` service）。
- 实现批量任务运行：跳过已存在、强制覆盖、取消、并发限制。
- 兼容输出：`apply_prompt/<角色名>/`、`out/<角色名>/`。

验收：

- "只生成提示词"模式可用。
- "生成图片"模式可用。
- 取消不再提交新任务。
- 已存在输出可跳过。
- `appearance.json/txt` 与动作提示词文件正确落盘。

## 阶段 10：动画脚本与分镜包

目标：完成动画短片入口的文本生产链路。

任务：

- 实现 `pages/script.slint`：主题输入、文档上传、参考图、生成按钮。
- 实现文档读取（`.txt` / `.md` 直接读，`.pdf/.docx` 提示不支持）。
- 实现脚本生成 service（推理 provider）。
- 实现脚本文件列表、打开、重命名、刷新、打开目录。
- 实现分镜包解析（识别镜头编号、表格）和拆分。
- 实现 `commands::send_script_to_storyboard`（路由切换 + 文本注入）。
- 实现 `components/MarkdownView.slint`（基于 `pulldown-cmark` + Slint 文本块）。

验收：

- 输入主题即可生成脚本。
- 文档参与生成。
- `.pdf/.docx` 有明确提示。
- 分镜包预览正常。
- 发送到分镜板能生成图。

## 阶段 11：Prompt Optimizer 集成与打包

目标：高级提示词优化稳定可用。

任务：

- `artait-providers::prompt_optimizer`：HTTP client 封装。
- 启动时检测 sidecar 端口，必要时拉起 `Back/` 的 axum 进程。
- 实现健康检查 + 重连。
- 实现 job 创建、轮询、取消、人工接管提示。
- 设置页显示服务状态。
- Windows 安装包包含 sidecar 二进制或提供启动指引。

验收：

- 服务未启动时提示清晰。
- 服务可用时能跑完整 job。
- 超时、失败、暂停、人工确认都有 UI 状态。
- 不泄露 API Key。

## 阶段 12：兼容迁移工具

目标：让旧用户数据可继续使用。

任务：

- 扫描旧 `prompt/` 模板，迁移到新模板目录。
- 扫描旧 `out/` 资产，注入新图库索引。
- 扫描旧 `apply_prompt/` 运行结果，做兼容显示。
- 读取旧 provider 实例结构，生成新 `app_config.toml` 草稿。
- 输出迁移报告（成功项、警告项、需手动确认项）。
- 提供"导入旧密钥"向导（用户重新输入或确认）。

验收：

- 旧输出能在新图库可见。
- 旧模板能在新页面选择。
- 不自动泄露旧密钥。
- 失败有可读报告。

## 体积与性能预算

| 阶段 | exe 体积上限 | 启动时间上限 |
|------|-------------|-------------|
| 阶段 1 空骨架 | 8 MB | 100 ms |
| 阶段 6 单图闭环 | 10 MB | 150 ms |
| 阶段 11 完整功能 | 12 MB | 200 ms |

每阶段记录 `target/release/ArtAITRust.exe` 体积和冷启动耗时，超预算时优先 `cargo bloat` 和 `cargo audit`。

## 风险清单

| 风险 | 影响 | 缓解 |
|------|------|------|
| Slint 主题运行时切换实现成本被低估 | 阶段 5 延期 | 提前在阶段 1 写主题原型，验证 Theme global + 自建组件 |
| std-widgets 编译期主题污染体积 | 体积超标 | 不引入 std-widgets，自建 10 个核心组件 |
| Markdown 渲染没有现成 Slint 组件 | 阶段 10 工作量大 | MVP 只渲染基础元素，复杂表格用 monospace |
| provider 行为差异大 | 生成链路不稳定 | 先稳定 trait 与响应归一化，wiremock 契约测试 |
| 旧配置含敏感信息 | 密钥泄露 | 配置/密钥分离，迁移工具不读密钥值 |
| 动作提示词规则复杂 | 输出不一致 | 保留单元测试和样例 fixture |
| Prompt Optimizer 双服务启动复杂 | 安装失败率高 | sidecar 管理 + 健康检查 + 启动指引 |
| 资产数量大时扫描慢 | 启动慢 | 只扫元数据，缩略图按需生成，必要时引入 SQLite |

## 首批开发任务（阶段 1-3 顺序）

1. 创建 workspace + 8 个 crate 骨架。
2. 配置 release profile 与 Windows 元信息。
3. `artait-model` 写核心类型与单测。
4. `artait-config` 写 TOML 读写、默认值、错误恢复、`secret_store`。
5. `artait-provider` 写 trait、`ProviderMeta`、`ProviderContext`、`Registry`、`HttpClient`。
6. `artait-providers::mock` 实现 mock provider。
7. `artait-app` 弹一个最小 Slint 窗口。
8. 接入 Theme global 原型（dark/light 切换）。
9. `artait-task::TaskRunner` 跑 mock 任务，UI 显示状态。
10. 写迁移报告 CLI（不输出密钥）。

## 最小可发布版本

MVP（阶段 1-8）：

- 单入口 + 功能开关
- 首启引导
- 3 套主题 + 用户自定义
- provider 实例设置 + 配置驱动
- 创建场景、创建角色、特效（通用美术预设）
- 单图生成闭环
- 图库浏览 + 去黑 + 去背景
- 错误、取消、重试
- API Key 本机配置保存与设置页回显

第二版本（阶段 9-12）：

- 动作序列
- 动画脚本 + 分镜板
- Prompt Optimizer sidecar
- 视频生成与播放
- 兼容迁移工具
