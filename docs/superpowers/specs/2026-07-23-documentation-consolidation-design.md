# ArtForge Studio 文档整理设计

## 目标

将客户端仓库的文档收敛为一套面向开发维护的当前事实说明，并保留一页简洁的产品边界。删除已经完成、已经失效或与当前实现冲突的历史资料，不在仓库内建立归档目录；历史追溯统一依赖 Git。

整理完成后，开发者应能从根目录 `README.md` 快速找到以下信息：

- 产品和仓库当前边界。
- 本地启动、构建、测试和打包方法。
- 活动客户端的架构与主要数据流。
- 服务端和客户端的数据权威边界。
- 发布、签名、公证和制品验证流程。
- 从旧客户端迁移时保留和舍弃的数据。

## 读者与范围

主要读者是维护 `native-client` 的开发者和发布人员。

文档描述当前客户端仓库，不承担以下职责：

- 不保存已经完成的实施计划、阶段进度或临时 TODO。
- 不复制服务端数据库表结构和服务端内部实现方案。
- 不把服务端动态下发的会员价格、积分包、模型名称或模型计费写成客户端常量。
- 不维护已经排除出 Cargo workspace 的历史 `crates/` 架构说明。
- 不修改或删除未纳入版本控制的 `design-qa.md`。

## 权威来源

文档内容按以下顺序核对：

1. `native-client/Cargo.toml`、根 `Cargo.toml` 和当前源码。
2. `.github/workflows/` 与 `scripts/` 中实际执行的构建、打包和发布逻辑。
3. 当前仍与源码一致的 `native-client/ARCHITECTURE.md`、`native-client/MIGRATION.md` 和 `docs/GITHUB_ACTIONS_RELEASE_SETUP.md`。
4. 旧设计、计划和状态文档只用于确认需要删除或提取的历史背景，不能覆盖当前源码事实。

当文档与代码不一致时，以代码为准并更新文档。

## 目标文档结构

### 根目录

#### `README.md`

仓库唯一入口，保持简洁，包含：

- 产品一句话介绍和当前版本来源。
- `native-client` 是唯一 Cargo workspace 成员和唯一应用二进制。
- 支持 Windows x64、macOS Intel 和 macOS Apple Silicon。
- 本地启动、检查、测试和 Release 构建的最短命令。
- Windows 与 macOS 打包脚本入口。
- 指向 `docs/README.md` 的文档导航。

README 不展开架构细节、完整 CI Secrets 或历史迁移过程。

#### `AGENTS.md`

面向代码维护者和自动化开发代理，内容全部与活动客户端一致：

- 当前仓库边界和禁止修改的历史目录。
- `native-client/src/runtime` 与 `native-client/ui` 的准确分层。
- Slint 1.16.1、Rust 2021 和当前依赖边界。
- 服务端 API 是账号、会员、积分、订单、模型和生成任务的权威来源。
- UI 回调、后台任务、Slint 事件循环、日志脱敏和本地存储约定。
- 启动、检查、测试、Release 构建与平台验证命令。
- 不再描述旧 `ui/`、旧 Provider Registry、旧主题 TOML 或已归档 crate 的开发步骤。

### `docs/`

#### `docs/README.md`

唯一文档索引，说明每份文档的用途和权威边界。新成员推荐阅读顺序为：

1. `PRODUCT.md`
2. `ARCHITECTURE.md`
3. `DEVELOPMENT.md`
4. 按需要阅读 `MIGRATION.md` 或 `RELEASE.md`

#### `docs/PRODUCT.md`

一页产品边界，包含：

- 桌面端 AI 美术创作定位。
- 欢迎、灵感、美术创作、生成历史、素材、模型、积分/会员、通知和设置等当前页面域。
- 登录、会员、积分、订单、支付、协议、模型目录和生成任务由服务端管理。
- 作品文件、部分展示元数据、界面偏好和恢复记录保存在本地。
- 模型目录、价格、会员权益和积分包均动态下发，文档不记录易变数值。

#### `docs/ARCHITECTURE.md`

合并并替代 `native-client/ARCHITECTURE.md` 和 `docs/CURRENT_CLIENT_MERGE.md`，包含：

- 单 workspace 成员和单二进制边界。
- Rust 入口、runtime 模块、API、生成、存储、功能和展示分层。
- Slint 的 app、state、types、components、pages 和 dialogs 分层。
- 启动、账户快照、图片生成、提示词任务、支付和结果落盘的关键数据流。
- Windows WebView2 与 macOS 系统能力的差异。
- 安全边界：Token、支付 URL、签名 URL和提示词不进入日志；平台密钥不进入客户端。
- `crates/`、根 `ui/`、`schemas/` 和 `themes/` 是历史源码，不参与当前构建。

#### `docs/DEVELOPMENT.md`

包含：

- Rust 工具链和平台依赖。
- `cargo run`、`cargo check`、`cargo test`、Release 构建命令。
- `ARTFORGE_API_BASE_URL` 等代码中实际支持的开发环境覆盖项。
- 跨栈 Mock API 测试的当前可复现命令。
- Windows 真机构建限制，以及 macOS 不能替代 MSVC/SDK 验证。
- 常见编译、渲染后端、会话和网络问题的排查入口。

只记录源码或脚本中真实存在的环境变量和命令。

#### `docs/RELEASE.md`

由 `docs/GITHUB_ACTIONS_RELEASE_SETUP.md` 重写并扩展本地打包入口，包含：

- Cargo 版本号、Git 标签和发布制品命名规则。
- Windows 安装器/免安装包、macOS x64/arm64 DMG 的脚本和 CI 来源。
- GitHub Actions 触发规则。
- OSS、Apple 签名和公证所需 Secrets 的名称，不包含实际凭据。
- 本地未签名包与正式签名包的区别。
- 标签发布前后的检查清单，以及 Windows WebView2/支付真机验收要求。

#### `docs/MIGRATION.md`

合并并替代 `native-client/MIGRATION.md`，包含：

- 旧客户端升级后保留的作品、草稿、界面偏好和恢复数据。
- 不再采用的本地账号、积分、Provider Endpoint、API Key 和供应商模型设置。
- 首次启动后的重新登录、协议确认和服务端账户核对步骤。
- 不误删作品目录的安全提示。

## 删除清单

以下资料直接从仓库删除，不移入归档目录：

- `PROJECT_STRUCTURE.md`
- `assets/README.md`
- `schemas/README.md`
- `native-client/ARCHITECTURE.md`
- `native-client/MIGRATION.md`
- `docs/ArtForgeStudio-exe-function-breakdown.md`
- `docs/ArtStudio-main-source-function-interaction-map.md`
- `docs/CURRENT_CLIENT_MERGE.md`
- `docs/FRONTEND_BACKEND_INTEGRATION_EXECUTION_PLAN.md`
- `docs/GITHUB_ACTIONS_RELEASE_SETUP.md`
- `docs/MEMBERSHIP_DATABASE_DESIGN.md`
- `docs/MEMBERSHIP_INTEGRATION_PLAN.md`
- `docs/MIGRATION_PLAN.md`
- `docs/STATUS.md`
- `docs/TODOS.md`
- `docs/plans/`
- `docs/rewrite/`

`docs/GITHUB_ACTIONS_RELEASE_SETUP.md`、`native-client/ARCHITECTURE.md` 和 `native-client/MIGRATION.md` 的仍有效内容先合并到新文档，再删除原文件。

## 去重规则

- 项目入口和最短命令只在 `README.md` 出现；专题细节使用链接。
- 架构事实只在 `docs/ARCHITECTURE.md` 展开；`AGENTS.md` 只保留执行开发任务必须遵循的规则。
- 完整开发命令只在 `docs/DEVELOPMENT.md` 维护；README 仅保留常用子集。
- 发布流程只在 `docs/RELEASE.md` 维护。
- 用户数据迁移只在 `docs/MIGRATION.md` 维护。
- 产品动态配置不复制具体数值，统一说明以服务端响应为准。
- 历史计划、测试日期和阶段完成度不转写到新文档。

## 验证标准

整理完成后执行以下检查：

1. `rg --files -g '*.md'` 中只保留目标文档、设计/实施规格和用户未跟踪文件。
2. 扫描所有 Markdown 相对链接，确保目标存在。
3. 扫描旧关键字，包括 `PySide6`、`ArtAITRust.exe`、`Slint 1.8`、`Provider Registry`、旧本地积分和旧固定价格；目标文档不得把这些描述为当前实现。
4. 对照 Cargo workspace、`native-client` 源码、脚本和工作流检查命令、版本来源、制品名称和平台信息。
5. 运行 `cargo check -p artforge-studio-native`，确认纯文档改动未意外影响项目配置。
6. 查看 `git diff --check` 和 `git status --short`，确认没有修改未跟踪的 `design-qa.md`，也没有混入构建产物。

## 完成定义

- 新开发者只读根 README 和 `docs/README.md` 就能找到全部当前资料。
- 仓库中不存在被误认为当前实现的旧架构、旧计划和旧计费文档。
- 同一主题只有一个权威专题文档。
- 所有保留内容均能从当前代码、脚本或工作流验证。
