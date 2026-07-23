# ArtForge Studio — Agent Guide

## Repository boundary

ArtForge Studio 是 Rust + Slint 桌面客户端。`native-client` 是唯一活动客户端和唯一 Cargo workspace 成员，包名为 `artforge-studio-native`，唯一应用二进制为 `ArtForgeStudio`。

`crates/`、根 `ui/`、`schemas/` 和 `themes/` 是早期模块化客户端的历史源码，不参与当前构建、测试或发布。除非任务明确要求处理历史源码，不要修改、恢复或重新接入这些目录。

当前技术基线：

- Rust edition 2021。
- 版本号来自 `native-client/Cargo.toml`。
- Slint 1.16.1，使用自建组件，不引入标准 widgets。
- HTTP 使用 `reqwest` 0.12 + rustls。
- Windows 和 macOS 的嵌入式网页能力使用 `wry`。

## Active architecture

Rust 入口和运行时：

- `native-client/src/main.rs`：平台可执行入口。
- `native-client/src/lib.rs`：应用库入口和测试入口。
- `native-client/src/runtime/app.rs`：启动、渲染器选择和顶层 callback 接线。
- `native-client/src/runtime/callbacks/`：按功能划分的 Slint callback 适配层。
- `native-client/src/runtime/api/`：平台 API、认证会话、账号、会员、积分、订单、通知、上传和生成协议。
- `native-client/src/runtime/generation/`：图片任务提交、轮询、取消、恢复和结果交付。
- `native-client/src/runtime/storage/`：应用路径、本地展示数据和恢复记录。
- `native-client/src/runtime/features/`：查看器、灵感等独立功能。
- `native-client/src/runtime/presentation/`：Slint 模型同步和主题应用。
- `native-client/src/runtime/services/image_processing.rs`：仅限本地图片处理。

Slint UI：

- `native-client/ui/app.slint`：窗口与页面/弹窗组合。
- `native-client/ui/app-state.slint`：全局展示状态和 callback 接口。
- `native-client/ui/types.slint`：Rust 可见的 UI 数据结构。
- `native-client/ui/theme.slint`：运行时调色板。
- `native-client/ui/components/`：可复用组件。
- `native-client/ui/pages/`：完整页面。
- `native-client/ui/dialogs/`：模态流程和覆盖层。

新增功能应进入职责最接近的模块。不要把业务逻辑堆到 `app.rs`、单个 callback 或 Slint 文件中。

## Build and verification

常用命令：

```bash
cargo run -p artforge-studio-native --bin ArtForgeStudio
cargo check -p artforge-studio-native
cargo test -p artforge-studio-native
cargo build --release -p artforge-studio-native --bin ArtForgeStudio
```

使用网络回环的 HTTP 单元测试可能需要允许本地监听端口。标记为 ignored 的跨栈测试要求后端 Mock API 已启动，不属于普通 `cargo test` 的外部依赖。

Windows release 和目标平台验收必须在 Windows MSVC + Windows SDK 环境执行；macOS 构建不能替代 Windows 真机验证。打包命令和制品要求见 `docs/RELEASE.md`。

## Slint and Rust integration

- 全局展示状态通过 `AppState` 读写，用户事件通过 callback 进入 Rust。
- callback 负责输入规范化、调用运行时能力和同步结果，不承担长时间网络或磁盘工作。
- 网络、轮询、下载和图片处理不得阻塞 Slint UI 事件循环。
- 后台结果必须通过 Slint 事件循环回到 UI 线程后再更新组件状态。
- 长任务使用稳定的任务/请求标识和显式状态，避免重试创建重复业务。
- 组件、页面和弹窗优先复用现有主题与控件，不使用 emoji 代替功能图标。

## Server-authoritative data

以下数据以 ArtForge 服务端为唯一权威：

- 账号、认证会话、设备会话和协议接受状态。
- 会员套餐、当前权益、积分余额与流水。
- 积分包、订单、支付状态和权益发放结果。
- 模型目录、支持能力、画质权限和计费。
- 提示词任务、图片生成任务和远端任务状态。

客户端只缓存展示和恢复所需信息。禁止重新加入 Provider Endpoint、平台 API Key、供应商模型配置、客户端本地加积分或客户端自行判定支付到账的逻辑。

模型、会员、积分包和价格均由服务端动态下发。不要在文档、UI 或 Rust 中把当前商业数值当成长期常量，除非它只是服务端返回数据的展示或测试夹具。

## Local persistence and recovery

客户端本地保存：

- 已下载作品和展示所需元数据。
- 提示词草稿、自定义提示词和界面偏好。
- 设备标识、安全刷新会话和必要的恢复记录。
- 未完成生成、交付确认和支付同步所需的稳定请求标识。

涉及保存流程时保持“下载并校验 → 原子写入 → 更新本地元数据 → 确认服务端交付”的顺序。不要在写入失败时把任务误标为已完成，也不要删除用户现有作品目录。

## Platform behavior

- Windows 默认使用 `winit-femtovg` GPU 渲染，降低最小化后恢复的重绘卡顿。
- 非 Windows 默认使用 `winit-software`；显式设置 `SLINT_BACKEND` 时尊重用户覆盖。
- Windows 使用 WebView2，macOS 使用 WKWebView 后端承载受信任的支付和协议内容。
- 嵌入网页只允许 HTTPS 和代码中明确列出的可信主机；禁止任意新窗口、下载和第三方导航。
- 与窗口、渲染器、拖拽和系统打开方式有关的改动必须分别考虑 Windows 与 macOS。

## Security and logging

- 不得提交或记录 API Key、Access Token、Refresh Token、验证码或签名材料。
- Prompt、参考图内容、支付 URL、OSS 签名 URL 和协议正文不得进入普通日志。
- 错误展示可以给出用户可理解的信息，但不得泄露内部业务码、请求标识或服务端响应正文。
- 发布 Secrets 只记录变量名，不记录值；证书、私钥和 OSS 凭据不得写入仓库。
- 所有支付和协议 URL 必须通过 HTTPS 与主机白名单校验。

## Change guidelines

- 先核对当前代码，再更新文档；代码和脚本是当前事实来源。
- 保持模块职责单一，避免跨层直接修改内部状态。
- API 字段中的积分、金额、游标和其他 BIGINT 边界继续使用十进制字符串。
- 对创建订单、提交生成和恢复任务等可能重复的操作保留幂等标识。
- 修改生成、支付、会话或本地保存流程时补充对应测试。
- 保留用户工作区中的未跟踪文件和无关改动；提交时只暂存本任务文件。
- 已完成的计划、日期状态快照和后端数据库设计不进入当前客户端文档。
