# ArtForge Studio 前端接入后端执行计划

> 状态：dev 开发接入已完成，正式发布验收延期
> 基线日期：2026-07-14
> 前端范围：`native-client`（当前唯一活动客户端）
> 后端范围：同级独立仓库 `../server/artforge-api`
> 当前边界：仅开发环境；代码暂不提交；后端不配置 Runner；Redis 保持现状；公网 HTTPS 与支付回调后续处理。
> 执行原则：dev 接入与正式发布验收分开记录，延期项不得误记为当前开发阻塞。

本文用于指导 ArtForge Studio 桌面客户端从“本地账号、积分和 Provider 直连”迁移到
“服务端权威账号、会员、积分、支付、模型目录和生成任务”。后续开始编码前先阅读本文，
实现过程中持续更新复选框和偏差记录。

## 1. 目标与完成定义

本次改造的最终目标：

- 用户通过邮箱验证码登录，服务端会话是登录状态的唯一权威。
- 会员、积分、订单、支付、模型价格和生成任务均以后端数据为准。
- 平台模型 Endpoint、API Key 和供应商内部模型 ID 不再下发或保存在新客户端状态中。
- 图片永久保存在客户端本地；OSS 只承担参考图和生成结果的短期中转。
- 客户端断网时仍可查看本地作品、编辑和保存提示词草稿，但不能执行在线业务。
- 客户端崩溃或重启后可以恢复刷新会话、待支付订单、生成任务和未完成交付。

整体完成必须同时满足：

- 邮箱登录、token 刷新、退出和离线模式闭环。
- 账户、会员、积分、模型目录全部来自后端。
- 无参考图生成、参考图生成、多图、取消、失败和部分成功闭环。
- 下载校验、本地原子保存和交付确认闭环。
- 会员购买、续费、升级、积分充值和支付宝网站支付闭环。
- 客户端不再执行本地发积分、本地充值或 Provider 直连。
- 关键状态机和 API 错误有自动测试。

## 2. 参考资料与权威边界

实现前需要同时参考：

- [会员系统接入方案](./MEMBERSHIP_INTEGRATION_PLAN.md)
- [会员系统数据库设计](./MEMBERSHIP_DATABASE_DESIGN.md)
- [后端 API 约定](../../server/artforge-api/docs/API_CONVENTIONS.md)
- [认证 API](../../server/artforge-api/docs/AUTH_API.md)
- [会员与支付 API](../../server/artforge-api/docs/MEMBERSHIP_PAYMENT_API.md)
- [模型与生成 API](../../server/artforge-api/docs/GENERATION_API.md)
- [后端 OpenAPI](../../server/artforge-api/docs/openapi.yaml)

当本文与后端实际路由或 OpenAPI 不一致时：

1. 先确认后端实际契约。
2. 不在客户端静默兼容猜测字段。
3. 将差异记录到本文“偏差与决策记录”。
4. 必要时先修改后端契约，再继续客户端实现。

## 3. 当前后端 dev 基线

截至基线日期：

- dev `/health/ready`：可用。
- dev MySQL：可连接。
- dev Redis：可连接。
- OSS：真实上传、读取、签名下载和删除验证通过。
- 图片模型：公开 `model_code` 为 `openai_image`，显示名称为 `GPT Image 2`，供应商模型为 `gpt-image-2`。
- 提示词模型：公开 `model_code` 为 `openai_prompt`，显示名称为 `GPT-5.5`，供应商模型为 `gpt-5.5`。
- OpenAI-compatible 模型目录检查通过。
- 真实付费提示词、生图和参考图编辑尚未执行。
- 支付宝真实小额支付和公网异步回调尚未验收。

本文不得记录 OSS、OpenAI、SMTP、支付宝、数据库或 Redis 密钥。

### 3.1 当前 dev 验收边界（2026-07-15）

- 当前只以本地开发环境的接口接入、代码级回归和本地构建检查作为验收依据。
- 代码保留在当前工作区，暂不 commit、push 或创建合并请求。
- 后端不配置 CNB/GitHub Runner；后端测试由本地命令执行。
- dev Redis 继续使用现有配置，不调整淘汰策略；生产 `noeviction` 要求保留为发布前门禁。
- 公网域名、HTTPS 证书、支付宝异步回调和真实小额支付均列入正式发布阶段，不属于当前 dev 阻塞项。
- Windows WebView2 真机和 Windows release 构建仍属于客户端发布验收，待具备 Windows 环境后执行。

## 4. 当前前端问题清单

| 编号 | 当前问题 | 处理方式 |
|---|---|---|
| F01 | 手机号登录和固定验证码 `123456` | 改为邮箱验证码登录，删除固定验证码 |
| F02 | `logged_in` 本地字段决定登录状态 | 改为服务端 session + 本地离线状态机 |
| F03 | 本地每日免费积分 | 删除，注册赠送积分只由后端发放 |
| F04 | 本地充值直接增加积分 | 删除，改为积分包下单和支付宝支付 |
| F05 | 积分、会员和价格在 Slint 中硬编码 | 改为后端账户、套餐和模型目录数据 |
| F06 | 客户端保存 Provider Endpoint/API Key | 停用 Provider 管理，新版本不再使用旧密钥 |
| F07 | 客户端直接调用图片/提示词供应商 | 改为后端生成任务 API |
| F08 | 生成任务、轮询和取消均是本地状态 | 改为服务端任务状态，本地只缓存展示和恢复信息 |
| F09 | 生成结果直接从供应商下载 | 改为 OSS 签名 URL 下载、校验和交付确认 |
| F10 | 支付界面仍是本地二维码假流程 | 改为支付宝电脑网站支付嵌入式二维码，macOS 使用 WKWebView、Windows 使用 WebView2 |
| F11 | 邀请返利页面使用本地假数据 | 后端无对应 API，首版隐藏入口 |
| F12 | 昵称可本地编辑 | 后端无修改昵称接口，首版显示服务端昵称并设为只读 |
| F13 | 画布比例需要与后端精确比例一致 | UI 展示并提交服务端支持的完整数字比例 |
| F14 | 前端可能选择超过 4 张 | 首版限制为最多 4 张 |
| F15 | 积分、金额和游标使用整数 | API DTO 和 UI 展示模型改用十进制字符串 |
| F16 | 无 refresh token 并发保护 | 实现单飞刷新，禁止同一 refresh token 并发使用 |
| F17 | POST 重试未持久化幂等键 | 先落盘请求 ID，再发请求，超时复用原 ID |
| F18 | 断网、服务不可用、会话失效没有区分 | 引入明确的在线/离线/失效/强更状态机 |
| F19 | 下载成功与本地保存、交付确认没有事务顺序 | 严格执行下载→校验→原子保存→元数据→确认交付 |
| F20 | API Token、Prompt、支付 URL、签名 URL 可能进入日志 | 建立客户端日志脱敏规则 |

当前重点关联文件：

- `native-client/src/runtime/app.rs`
- `native-client/src/runtime/model.rs`
- `native-client/src/runtime/features/account.rs`
- `native-client/src/runtime/storage/local_store.rs`
- `native-client/src/runtime/callbacks/generation.rs`
- `native-client/src/runtime/callbacks/provider.rs`
- `native-client/src/runtime/generation/controller.rs`
- `native-client/src/runtime/services/http.rs`
- `native-client/src/runtime/services/image_api.rs`
- `native-client/src/runtime/services/prompt_api.rs`
- `native-client/ui/app-state.slint`
- `native-client/ui/dialogs/auth-dialog.slint`
- `native-client/ui/pages/credits-page.slint`
- `native-client/ui/pages/models-page.slint`
- `native-client/ui/pages/invite-rebate-page.slint`

## 5. 已确定的实现决策

### 5.1 数据权威边界

服务端权威数据：

- 用户、会话和协议接受记录。
- 会员套餐、当前周期、未来周期和权益。
- 积分余额、批次、流水和预占。
- 订单、支付事实和权益兑现状态。
- 模型目录、能力、画质权限和积分价格。
- 生成任务、任务条目、取消和服务端临时文件。
- 通知和设备会话列表。

客户端本地数据：

- 本地作品文件及本地图库元数据。
- 提示词草稿、主题、字体和界面设置。
- 稳定设备 ID。
- refresh token 的系统凭据管理器引用。
- 为崩溃恢复保存的请求 ID、任务 ID、订单 ID和目标保存目录。
- 最近一次成功同步的账户和模型展示缓存。

客户端不得本地修改服务端权威余额、会员或订单终态。

### 5.2 API 环境

- dev 默认 API 地址：`https://artforge-api.honeykid.cn`。
- dev 允许通过环境变量 `ARTFORGE_API_BASE_URL` 覆盖。
- prod 使用编译或受控配置中的固定 HTTPS 域名。
- prod 设置页不提供任意 API 地址输入框。
- dev/prod 配置不得包含平台模型密钥。

### 5.3 HTTP 执行模型

- 继续复用 `reqwest::blocking::Client`。
- 所有网络请求必须在后台线程执行，不能阻塞 Slint UI 线程。
- 后台结果统一通过 `slint::invoke_from_event_loop` 返回 UI。
- API callback 只负责读取 UI 参数、调用 API/service 和显示结果。
- 不为每个轮询 tick 无限制创建新线程；使用受控后台任务或单任务轮询器。
- 暂不引入 Tokio；只有阻塞模型无法满足支付或长期任务需求时再评估。

### 5.4 设备与请求身份

- 首次运行生成 UUID v4 设备 ID，原子写入本地配置。
- 不使用 MAC、硬盘序列号或其他硬件指纹作为设备 ID。
- 登录时发送 `device_id`、可读 `device_name`、`platform=windows` 和 `app_version`。
- 登录后的请求携带 `X-Client-Version` 与 `X-Device-ID`。

### 5.5 Token 存储和单飞刷新

- access token 只保存在内存中。
- refresh token 只存系统凭据管理器，不写入 JSON/TOML/日志。
- 使用一个进程级 `SessionManager` 管理 token。
- 同一时间最多一个 refresh 请求；其他请求等待同一个刷新结果。
- 刷新成功后先安全保存新 refresh token，再向等待请求发布新 access token。
- 刷新成功但凭据保存失败时，禁止继续使用旧 refresh token，转为重新登录。
- 收到 `refresh_token_reused`、`session_invalid`、`session_device_mismatch` 等错误时清理会话并强制登录。
- 网络超时或 5xx 不能当作会话失效，应进入离线/重试状态。

### 5.6 离线模式

- 首次使用必须联网登录。
- 曾经成功登录且存在本地认证标记的设备，在服务不可达时允许离线进入。
- 离线只允许浏览本地作品、编辑和保存提示词草稿。
- 离线禁止生成、上传、支付、充值、升级和修改会员。
- 网络恢复后自动刷新 token、账户、会员、积分、模型和任务。
- 明确 401/会话撤销时不能继续离线冒充有效登录。
- 客户端版本过低时仍允许进入本地离线模式。

### 5.7 BIGINT 和金额

- 服务端返回的积分、金额、游标、内部序号一律用 `String` 接收。
- 不把十进制 BIGINT 转为 Slint `int`、Rust `i32` 或浮点数。
- 金额在 UI 层使用字符串格式化“分→元”，禁止浮点运算决定支付金额。
- 客户端只展示服务端报价，不自行计算升级或折扣金额。

### 5.8 幂等与崩溃恢复

- 订单和生成任务在发请求前生成稳定 `client_request_id`。
- `client_request_id` 同时作为 `Idempotency-Key`。
- 请求身份与业务参数摘要先写入本地恢复记录，再发出网络请求。
- 超时、进程崩溃或响应丢失后复用相同 ID。
- 只有收到明确资源 ID 或确认请求未创建资源时才能清理恢复记录。
- 不带稳定幂等键的 POST 不自动重试。

### 5.9 比例、张数和画质

后端支持精确数字比例，客户端 UI 与任务提交保持同一组值：

- 支持 `1:1`、`3:2`、`2:3`、`4:3`、`3:4`、`5:4`、`4:5`、`16:9`、`9:16`、`2:1`、`1:2`、`21:9`、`9:21`。
- UI 直接展示数字比例，并将相同值作为 `aspect_ratio` 提交给服务端。
- 服务端兼容旧值 `square`、`landscape`、`portrait`，分别标准化为 `1:1`、`3:2`、`2:3`，仅用于旧请求和历史任务恢复。
- 张数限制为 1–4。
- 画质使用 `1K`、`2K`、`4K`。
- 是否可选由服务端模型目录和会员权益决定。
- 服务端返回权限错误时刷新会员和模型状态，不在本地绕过。

### 5.10 暂不支持的 UI 功能

首版隐藏或禁用：

- 邀请返利入口和假数据。
- Provider 创建、编辑、测速、删除和模型角色设置。
- 每日免费积分。
- 自定义积分充值金额。
- 自动续费、年付、会员降级、现金退款。
- 服务端昵称编辑。

旧 Provider/API Key 不上传、不继续使用，也不擅自删除旧本地文件；后续可提供用户主动清理入口。

## 6. 目标代码结构

计划新增：

```text
native-client/src/runtime/api/
├── mod.rs
├── client.rs              # Client、公共头、响应 envelope、超时
├── error.rs               # API 错误码、网络错误和中文展示
├── types.rs               # 公共 DTO，BIGINT 全部为 String
├── session.rs             # access/refresh token、单飞刷新、应用私有文件存储
├── auth.rs                # 协议、验证码、登录、刷新、退出
├── account.rs             # 账户和设备会话
├── membership.rs          # 套餐、当前会员、升级
├── credits.rs             # 积分账户、积分包、流水
├── orders.rs              # 订单查询和同步
├── catalog.rs             # 模型目录
├── uploads.rs             # 参考图 OSS PostObject
├── generation.rs          # 创建、查询、取消、交付
└── notifications.rs       # 通知列表和已读

native-client/src/runtime/backend/
├── mod.rs
├── bootstrap.rs           # 登录后并行加载账户/会员/积分/模型/通知
├── generation_flow.rs     # 前端生成编排和任务恢复
├── delivery.rs            # 下载、SHA-256、原子保存、确认交付
├── payment.rs             # 订单状态机和轮询
└── offline.rs             # 在线/离线/失效状态切换
```

建议依赖调整：

- `reqwest` 增加 `multipart` 特性，用于 OSS PostObject。
- refresh token 保存到应用数据目录的私有文件；Unix/macOS 下目录权限为 `0700`、文件权限为 `0600`，不访问系统钥匙串。
- 增加 `sha2`，校验下载文件。
- 支付阶段评估 `webview2-com` 或 `wry`；先完成与 Slint/winit 单事件循环兼容性验证。
- 测试可增加适合 blocking client 的 mock HTTP 工具，或使用本地 `TcpListener` 测试服务器。

## 7. API 接口映射

### 7.1 认证与协议

| 功能 | 方法与路径 | 备注 |
|---|---|---|
| 协议列表 | `GET /v1/agreements` | 登录前可调用 |
| 发送邮箱验证码 | `POST /v1/auth/email/code` | body 含 `email`、`app_version` |
| 登录或注册 | `POST /v1/auth/email/login` | 含设备、平台、版本和协议接受 |
| 刷新 token | `POST /v1/auth/refresh` | body 含 refresh token、设备 ID和版本 |
| 当前设备退出 | `POST /v1/auth/logout` | 需要鉴权头 |
| 全设备退出 | `POST /v1/auth/logout_all` | 需要鉴权头 |

### 7.2 账户和设备

| 功能 | 方法与路径 |
|---|---|
| 当前账户 | `GET /v1/account` |
| 会话列表 | `GET /v1/account/sessions` |
| 撤销其他会话 | `DELETE /v1/account/sessions/:session_id` |

### 7.3 会员、积分、订单和支付

| 功能 | 方法与路径 |
|---|---|
| 套餐列表 | `GET /v1/membership/plans` |
| 当前会员 | `GET /v1/membership/current` |
| 购买或续费 | `POST /v1/membership/orders` |
| 升级报价 | `POST /v1/membership/upgrade-quotes` |
| 升级下单 | `POST /v1/membership/upgrade-orders` |
| 积分账户 | `GET /v1/credits/account` |
| 积分包 | `GET /v1/credits/packs` |
| 积分流水 | `GET /v1/credits/ledger` |
| 积分充值下单 | `POST /v1/credits/orders` |
| 查询订单 | `GET /v1/orders/:order_id` |
| 主动同步支付 | `POST /v1/orders/:order_id/sync` |

### 7.4 模型、上传和生成

| 功能 | 方法与路径 |
|---|---|
| 模型目录 | `GET /v1/models` |
| 准备参考图上传 | `POST /v1/uploads/references` |
| 完成参考图上传 | `POST /v1/uploads/references/:file_id/complete` |
| 删除未使用参考图 | `DELETE /v1/uploads/references/:file_id` |
| 创建生成任务 | `POST /v1/generation/tasks` |
| 任务列表 | `GET /v1/generation/tasks` |
| 任务详情 | `GET /v1/generation/tasks/:task_id` |
| 取消任务 | `POST /v1/generation/tasks/:task_id/cancel` |
| 确认文件交付 | `POST /v1/generation/tasks/:task_id/deliveries/:file_id/ack` |
| 删除任务内容 | `DELETE /v1/generation/tasks/:task_id/content` |

### 7.5 通知

| 功能 | 方法与路径 |
|---|---|
| 通知列表 | `GET /v1/notifications` |
| 单条已读 | `POST /v1/notifications/:notification_id/read` |
| 全部已读 | `POST /v1/notifications/read_all` |

## 8. 关键状态机

### 8.1 客户端会话状态

```text
signed_out
  -> requesting_code
  -> authenticating
  -> online

online
  -> refreshing -> online
  -> network_unavailable -> offline
  -> session_invalid -> signed_out
  -> client_unsupported -> update_required

offline
  -> network_restored -> refreshing -> online
  -> explicit_logout -> signed_out
```

建议在 Rust 层使用枚举作为真实状态，Slint 只接收用于展示的派生属性。

### 8.2 生成和交付状态

```text
draft
  -> preparing_references
  -> uploading_references
  -> creating_task
  -> queued
  -> processing
  -> completed / partially_completed / failed / cancelled

completed or partially_completed
  -> downloading
  -> verifying
  -> saving_local
  -> metadata_saved
  -> acknowledging_delivery
  -> delivered
```

必须遵守：

- 每个服务端任务只能有一个轮询器。
- queued 建议 2–3 秒轮询；processing 建议 2 秒；长期后台逐步退避至 5–10 秒。
- 窗口最小化时降低频率，恢复前台时立即同步一次。
- 签名 URL 不持久化；恢复时重新查询任务详情获取新 URL。
- 本地文件和元数据保存完成前绝不确认交付。
- 本地保存使用同目录临时文件和原子重命名。
- 确认交付失败时保留本地已保存标记并重试 ack，不重复下载。

### 8.3 支付状态

```text
creating_order
  -> pending_payment
  -> checkout_open
  -> syncing
  -> paid_fulfilling
  -> fulfilled

pending_payment/checking
  -> expired / closed
  -> paid_fulfilling
```

必须遵守：

- 创建订单前落盘 client request ID。
- 保存待支付订单 ID；应用重启后可继续查询。
- WebView 关闭不代表订单取消。
- 每两秒同步一次，连续后台时逐步退避。
- 不为同一业务同时创建多个待支付订单。
- `paid + fulfillment_retry` 显示“支付成功，权益处理中”。
- 不把真实已支付状态降级为失败。
- WebView 只允许 HTTPS；正式联调后记录并审核支付宝跳转域名白名单。
- 不向 WebView 注入 access token 或 refresh token。

## 9. 分阶段执行步骤

### 阶段 0：建立基线和迁移护栏

任务：

- [x] 执行并记录 `cargo check -p artforge-studio-native`（2026-07-15 通过；旧直连链路 warning 已清理，完整 Wire DTO 未展示字段按模块意图标注）。
- [x] 执行并记录 `cargo test -p artforge-studio-native`（2026-07-15，当前 29/29 通过）。
- [x] 记录当前 UI 页面和本地数据文件位置。
- [x] 为旧配置读取增加版本标记，禁止迁移旧积分和登录状态。
- [x] 确认旧 Provider/API Key 不上传、不展示、不删除。
- [x] 在实现分支中保持每个阶段可编译。

完成条件：当前客户端基线可复现，迁移不会破坏本地作品和设置。

### 阶段 1：API Client、设备 ID和安全会话基础

任务：

- [x] 新增 `runtime/api` 基础目录和 DTO。
- [x] 实现统一响应 envelope 与错误类型。
- [x] 实现 API base URL dev 覆盖和 prod HTTPS 限制。
- [x] 生成并持久化设备 UUID。
- [x] 引入系统凭据管理器保存 refresh token。
- [x] 实现 `SessionManager` 单飞刷新。
- [x] 自动注入版本、设备和 access token 请求头。
- [x] 实现日志脱敏和请求 ID 展示。
- [x] 为超时、401、5xx、无效 JSON 和错误码添加测试。

完成条件：不接 UI 业务也能通过测试验证登录前请求、鉴权请求和单飞刷新行为。

### 阶段 2：邮箱登录、协议和离线模式

任务：

- [x] `auth-phone` 等状态重命名为邮箱语义。
- [x] 登录弹窗改为邮箱、验证码和协议勾选。
- [x] 删除固定验证码判断。
- [x] 接入协议列表、发送验证码和登录接口。
- [x] 实现验证码倒计时、重复发送限制和错误提示。
- [x] 实现启动自动 refresh。
- [x] 实现当前设备退出和全部设备退出。
- [x] 实现首次必须联网、已登录设备离线进入。
- [x] 区分网络故障、服务端 5xx、会话失效和版本过低。

完成条件：真实 dev 后端可以完成邮箱登录、重启自动登录、退出、断网离线和恢复在线。

### 阶段 3：账户、会员、积分和启动 Bootstrap

任务：

- [x] 登录后并行加载账户、当前会员、积分账户、套餐、积分包和模型目录。
- [x] 建立服务端账户展示模型，BIGINT 使用字符串。
- [x] 删除本地每日免费积分。
- [x] 删除本地充值增加余额。
- [x] 积分页面改为服务端余额、积分包和流水。
- [x] 会员卡、有效期、清晰度权益来自后端。
- [x] 个人资料昵称首版设为只读。
- [x] 隐藏邀请返利入口。
- [x] 网络恢复后统一刷新账户快照。

完成条件：重启和多设备登录后显示相同服务端余额，客户端无法本地篡改积分。

### 阶段 4：服务端模型目录和 Provider 退场

任务：

- [x] 模型页面改为 `GET /v1/models` 数据源。
- [x] 只保存公开 `model_code`，不保存供应商模型 ID。
- [x] 展示模型能力、价格和支持画质。
- [x] 根据会员权益禁用 2K/4K 或提示升级。
- [x] 停用 Provider 编辑器和相关 callback。
- [x] 停用客户端 Provider 测速和模型列表请求。
- [x] UI 比例改为服务端支持的完整数字比例列表。
- [x] UI 张数限制为 1–4。
- [x] 删除 Slint 中硬编码的积分价格计算。

完成条件：客户端不需要 Provider/API Key 即可展示可用模型和服务端价格。

### 阶段 5：最小生成闭环（无参考图、单张）

任务：

- [x] 生成按钮创建稳定 request ID并先写恢复记录。
- [x] 调用 `POST /v1/generation/tasks` 创建单张任务。
- [x] 保存服务端 task ID和目标本地目录。
- [x] 实现任务详情轮询和状态映射。
- [x] 实现任务取消。
- [x] 下载 OSS 签名 URL。
- [x] 校验 SHA-256 和字节数。
- [x] 临时文件原子保存到本地图库。
- [x] 写入本地作品元数据。
- [x] 最后调用交付确认。
- [x] 成功后刷新积分账户。

完成条件：邮箱登录后可使用 dev 后端生成一张图，文件永久保存本地，OSS 交付确认完成。

### 阶段 6：参考图、多图、部分成功和任务恢复

任务：

- [x] 准备参考图上传，校验格式和大小限制。
- [x] 按后端返回字段执行 OSS multipart PostObject。
- [x] 调用完成上传接口进行真实类型验证。
- [x] 创建带 reference file ID 的生成任务。
- [x] 支持 1–4 张及部分成功条目。
- [x] 分别下载并确认每个成功文件。
- [x] 失败条目显示服务端错误，不重复扣费。
- [x] 应用重启后从本地恢复记录和任务列表恢复轮询。
- [x] 已保存但 ack 失败的文件只重试 ack。
- [x] 用户取消或放弃未绑定参考图时调用删除接口。

完成条件：参考图、多图、取消、部分成功、重启恢复和交付重试均可复现。

### 阶段 7：会员购买、升级、充值和支付宝网站支付

任务：

- [x] 购买前确认会员服务协议和积分规则。
- [x] 接入会员购买、续费、升级报价和升级下单。
- [x] 接入积分包下单。
- [x] 保存 pending order ID和 request ID。
- [x] 封装与 Slint 共用事件循环的 `PaymentWindow`。
- [ ] （正式发布阶段）在 Windows 真机完成 WebView2 编译、显示、关闭和支付宝跳转技术验证。
- [x] 会员与积分订单加载 `qr_pay_mode=4` 的支付宝签名页面，在应用内显示二维码，不打开系统浏览器。
- [x] 实现订单同步、过期、关闭和权益处理中状态。
- [x] 支付窗口手动关闭后继续允许订单恢复。
- [x] 支付完成刷新会员、积分和通知。
- [x] 审核 WebView 导航白名单和日志脱敏。

dev 完成条件：支付代码、订单恢复和状态一致性回归通过。真实小额支付、HTTPS 回调和 Windows
WebView2 真机验证延期至正式发布阶段。

### 阶段 8：通知、设备会话、最低版本和强制协议

任务：

- [x] 通知页面接入列表、单条已读和全部已读。
- [x] 个人资料页展示当前设备和其他会话。
- [x] 支持撤销其他设备会话。
- [x] 处理最低版本错误并显示强制更新界面。
- [x] 版本过低仍允许访问本地离线作品。
- [x] 协议版本更新时重新要求接受。
- [x] 会员到期提醒和支付/任务通知刷新正确。

完成条件：通知、会话撤销、协议升级和最低版本流程有真实接口或 mock 回归证据。

### 阶段 9：旧逻辑清理和本地迁移收尾

任务：

- [x] 移除手机号、固定验证码和本地积分字段。
- [x] 移除每日积分和本地充值函数。
- [x] 移除 Provider 页面入口和 callback。
- [x] 移除客户端直连图片和提示词服务代码。
- [x] 删除不再使用的 API Key UI 属性。
- [x] 保留旧配置文件但不再加载 Provider 到新 UI。
- [x] 保留本地作品、草稿、主题和界面设置。
- [x] 增加用户主动清除旧 Provider 密钥的后续入口或迁移提示。
- [x] 更新 README、架构文档和用户迁移说明。

完成条件：代码搜索不再存在固定验证码、本地发积分或新流程中的供应商 API Key 访问。

### 阶段 10：回归、稳定性和发布准备

任务：

- [x] 登录、刷新轮换、并发 401 和旧 token 重放代码级回归；服务端旧 token 状态分类已接入真实撤销逻辑。
- [ ] （真实登录环境验收）首次离线、已登录离线、网络恢复和会话撤销 UI 全链路回归；网络/终态会话错误分类已有单测。
- [x] BIGINT 超过 `2^53 - 1` 的展示和分页回归（DTO、Slint 状态和游标均保持十进制字符串，已补单测）。
- [x] 积分不足、画质无权限、模型下线错误码与用户提示回归。
- [x] 任务超时、取消、部分成功、Worker 恢复代码级回归。
- [x] 下载损坏、磁盘写入失败、ack 失败和重启恢复代码级回归。
- [x] 支付重复回调、订单过期和权益处理中代码级回归。
- [x] 检查日志中不存在 token、验证码、Prompt、API Key、支付 URL和签名 URL。
- [x] 执行 `cargo check` 和 `cargo test`。
- [ ] （客户端正式发布阶段）在 Windows runner 执行 release 构建并完成 WebView2 真机验证；仓库已加入 Windows CI 门禁。
- [x] 统一 Cargo 客户端版本来源，并记录最低客户端版本发布策略；生产示例最低版本为 `0.1.0`。

完成条件：所有主路径和故障路径有可重复证据，旧版本仍可访问本地作品。

## 10. 测试矩阵

至少覆盖：

| 范围 | 必测场景 |
|---|---|
| API Client | 成功、业务错误、401、429、5xx、超时、非 JSON、请求 ID |
| Token | 单次刷新、并发刷新、刷新失败、凭据写入失败、重放撤销 |
| 登录 | 验证码、协议缺失、版本过低、新用户、老用户、断网 |
| BIGINT | 大积分、大金额、大游标，不发生截断或浮点误差 |
| 幂等 | 请求超时重试、重启恢复、参数冲突、重复点击 |
| 模型 | 目录刷新、模型下线、画质权限变化、积分价格变化 |
| 上传 | JPEG/PNG/WebP、类型伪装、超限、过期、取消删除 |
| 生成 | queued、processing、完成、部分完成、失败、取消、恢复 |
| 交付 | 哈希错误、大小错误、磁盘失败、元数据失败、ack 重试 |
| 支付 | 待支付、关闭 WebView、过期、成功、权益处理中、重复恢复 |
| 离线 | 首次离线、已登录离线、恢复联网、服务端撤销、强制更新 |
| 迁移 | 作品保留、草稿保留、旧积分不迁移、旧 Provider 不上传 |

## 11. 编码禁止事项

- 禁止在 Slint callback 中执行阻塞网络请求。
- 禁止并发使用同一个 refresh token。
- 禁止在请求超时后为同一业务生成新的幂等键。
- 禁止将积分、金额或 BIGINT ID 转为浮点数。
- 禁止在本地直接增加积分、开通会员或修改订单终态。
- 禁止把 Provider API Key 上传到 ArtForge 后端。
- 禁止记录 token、验证码、Prompt、支付 URL或 OSS 签名 URL。
- 禁止在文件和元数据保存完成前确认交付。
- 禁止把网络不可用直接当成会话失效。
- 禁止静默把前端精确比例伪装成后端不支持的精确输出。
- 禁止在迁移中删除用户旧作品、草稿、主题或旧 Provider 配置文件。
- 禁止在未验证 WebView 安全边界前开放任意 URL 导航。

## 12. 总进度清单

- [x] 后端 dev 基础设施 ready。
- [x] OSS 真实读写与删除验证。
- [x] OpenAI-compatible 模型目录验证。
- [ ] （正式发布阶段）后端真实付费提示词和图片生成验证。
- [x] 阶段 0：基线和护栏。
- [x] 阶段 1：API Client 和安全会话。
- [x] 阶段 2：邮箱登录和离线模式。
- [x] 阶段 3：账户、会员和积分。
- [x] 阶段 4：模型目录和 Provider 退场。
- [x] 阶段 5：最小生成闭环。
- [x] 阶段 6：参考图、多图和任务恢复。
- [x] 阶段 7：dev 代码接入与状态回归完成；真实支付、回调和 Windows 验收延期。
- [x] 阶段 8：通知、会话、版本和协议。
- [x] 阶段 9：旧逻辑清理和迁移。
- [x] 阶段 10：dev 本地代码级回归完成；真实环境与 Windows 发布验收延期。

## 13. 偏差与决策记录

### 2026-07-14 实施记录

- dev 模型目录已升级为 `2026-07-15.1`：提示词模型 `GPT-5.5`（`gpt-5.5`）升为 v3，图片模型 `GPT Image 2`（`gpt-image-2`）升为 v2；公开 `model_code` 保持不变。
- 本地 API `/health/ready` 返回 `ready: true`，客户端启动后已真实访问 `/v1/agreements`。
- 客户端 `cargo check -p artforge-studio-native` 通过；2026-07-15 完整单测为 29/29，通过真实回环 TCP 覆盖 HTTP 超时、401、5xx 和无效 JSON，并覆盖并发刷新、Token 轮换、离线/撤销/强更启动决策、业务错误映射、BIGINT 字符串边界、下载完整性、磁盘失败、部分成功恢复、支付状态不降级、HTTPS 白名单和会员到期提醒。
- 已实现：安全会话、邮箱登录、自动网络恢复、全设备退出、强更和协议升级、并行账户快照、字符串积分余额、动态积分包/会员套餐/模型能力价格、参考图直传、服务端生成与提示词任务、任务列表与本地日志双重恢复、原子保存和 ack 恢复、上传阶段取消清理、会员购买/续费/升级、可恢复支付订单、与 Slint 共用事件循环的 Windows WebView2 子窗口及 HTTPS 导航白名单、通知、会员到期提醒和设备会话。
- 已物理移除活动 UI 中的 Provider 编辑器、API Key/Endpoint、测速和供应商模型列表回调；旧配置文件仍按迁移原则保留但不加载。
- 已清理：客户端直连图片/提示词旧模块、Provider API Key/Endpoint、遗留本地账号/积分/邀请字段；已补充用户迁移说明。
- 正式发布阶段仍需外部环境验收：Windows MSVC/SDK runner 的 release 构建，以及真实邮箱、付费生成、支付宝小额支付和公网 HTTPS 异步回调全链路；这些不属于当前 dev 阻塞项。macOS 交叉检查会因缺少 Windows SDK C 头文件停在 `ring`，不能替代 Windows runner。
- 服务端验证：2026-07-15 使用公开 `dev.example.yaml` 在隔离的本地测试条件下执行 `npm test` 30/30、`npm run lint` 通过；覆盖 refresh token 重放分类、支付通知幂等/脱敏、订单 checkout URL 过期、BIGINT、任务部分成功/取消、Worker stale 恢复、Provider 超时、协议内容 SHA-256、Redis 策略解析和最低客户端版本。dev `/health/ready` 返回 MySQL、Redis、OSS、OpenAI-compatible provider 均 ready。Redis 当前报告淘汰策略为 `volatile-lru`，dev 暂不修改；生产发布前再按门禁调整为 `noeviction`。
- dev `GET /v1/agreements` 当前返回空列表；开发接入不因此阻塞，正式发布前再发布并启用用户协议、隐私政策、会员服务协议和积分规则的当前版本。
- macOS 客户端已完成启动冒烟并进入 Slint 事件循环；当前会重复输出 ICU4X 缺少日语分词模型的非阻断日志，发布前应消除日志噪声或补齐对应分词数据。

### 2026-07-15 收尾记录

- 客户端内部服务端模型目录已从遗留 `Provider` 命名收敛为 `ModelGroup/ModelCatalog`，移除 provider ID 状态、旧 callback 文件和未使用的 Provider 选择组件。
- 删除未使用的直连生成结果分支、旧提示词拼装函数、旧模型角色函数、客户端硬编码积分价格函数和图片 data URL 转换函数。
- 客户端仓库已改用 GitHub Actions：Windows x64、macOS Intel 和 macOS Apple Silicon 分别执行 locked check、library test 和 release 打包；`v*` 标签会将 EXE 与两个 DMG 上传 OSS。
- Runner、Secrets、OSS 路径和 WebView2/支付宝真机验收说明见 `docs/GITHUB_ACTIONS_RELEASE_SETUP.md`。
- 按当前开发范围不为后端配置 Runner；服务端测试继续使用本地 Node 24 命令执行。
- 生产 `preflight` 新增协议目录完整性门禁；用户协议、隐私政策、会员服务协议和积分规则缺少任一项均禁止通过预检。协议正文、HTTPS 发布地址和 SHA-256 仍需法务/部署提供。
- 修复隔离本地环境中服务端无私有配置可用的问题：关闭支付宝时不初始化 RSA SDK，dev/prod 示例配置均可通过 schema；启用支付宝仍强制校验正式商户号和 RSA 密钥。
- 新增代码级故障回归：旧 refresh token 重放、Provider 超时、部分成功/取消、Worker stale 恢复、损坏下载、磁盘写入失败、ack 恢复、支付重复通知、订单过期和权益处理中不降级。
- 客户端 UI、请求头和安装包元数据统一从 Cargo package version 取值；生产最低版本只允许在新安装包发布并验证后提高。
- 生产预检会实际下载四份 HTTPS 协议静态内容并校验 2 MiB 大小上限与 SHA-256，避免协议 URL 指向错误或内容被静默替换。
- Redis 健康检查新增 `eviction_policy/policy_status`；dev 的 `volatile-lru` 作为 warning 保留且暂不修改，生产预检仍要求 `noeviction`。
- 当前工作区代码暂不提交；公网 HTTPS、支付异步回调和生产 Redis 调整统一延期到正式发布阶段。

### 2026-07-15 dev Mock 全流程验收

- 后端公开示例配置检查、30 项单元测试和 ESLint 全部通过。
- `npm run api:check` 通过：以 Mock 邮件验证码和支付宝完成真实 HTTP 路由的注册登录、100 初始积分、账户、设备会话、会员、积分、模型目录、任务与通知读取、提示词任务创建/查询/取消、会员下单、积分包下单、支付状态同步、refresh token 轮换、退出和退出后 401；测试账号、订单、任务和连接自动清理。
- `npm run membership:check` 通过：使用 Mock 支付宝覆盖会员购买、重复支付幂等、续费、升级报价与升级、积分充值、异步通知重复回调、主动查单、订单过期和积分预占结算。
- `npm run generation:check` 通过：使用 Mock OSS/OpenAI 覆盖参考图上传与类型校验、任务幂等、图片部分成功、提示词优化、积分结算、结果交付确认、取消和临时对象清理。
- 客户端 29 项测试全部通过；覆盖本地 HTTP 超时、401、5xx、无效 JSON、token 单飞刷新、离线/撤销/强更决策、BIGINT、部分成功恢复、下载校验、磁盘失败、支付状态和 WebView 导航白名单；`cargo check --locked` 通过。
- 当前 Rust 工具链未安装 `cargo fmt` 和 `cargo clippy` 组件，因此这两项未纳入本次功能验收；不影响上述构建和测试结果。
- 修正 OpenAPI 中会员升级路由的命名偏差：与实际服务端和客户端统一为 `upgrade-quotes`、`upgrade-orders`。

### 2026-07-15 前后端跨栈联测

- 后端新增仅允许在 dev 且显式设置 `ARTFORGE_ENABLE_MOCK_API=1` 时启动的 Mock API；固定验证码并替换邮件、支付宝、OSS 和任务队列外部动作，关闭时自动清理联测账号、订单、任务、文件和测试配额。
- 客户端跨栈契约测试已拆分为 39 组独立用例，由真实 Rust API 层请求真实 Koa HTTP 路由，不经过手写 JSON 断言替代客户端解析；常规测试默认忽略，联测命令显式启用并按单线程执行。
- 成功契约覆盖协议目录、验证码登录、Token 安装/刷新、并行账户快照、会员/积分/模型 DTO、提示词与图片任务、multipart 参考图上传/确认/删除、积分包和会员下单、订单查询/同步/幂等重放、通知和退出。
- 参数矩阵累计执行 172 次 JSON 错误响应断言，全部核对 HTTP 状态与业务 `code`；其中 80 次进一步核对 `details` 是数组且包含准确的 `details.field`。此外单独设置 1 项 405 JSON 信封协议门禁，当前因服务端返回纯文本而失败。
- 认证粒度：分别验证邮箱格式和长度、`app_version` 缺失/格式、验证码长度/错误值、`device_id` 长度、设备名称长度、平台枚举、未认证访问、设备不匹配、refresh token 长度、旋转后重放、会话失效和退出清理。
- 协议与分页粒度：分别验证空协议数组、非法协议类型的嵌套字段路径、重复协议、积分流水和通知的 `limit=0/101`、非法 cursor、非法布尔值，以及合法单条分页的 DTO/meta。
- 提示词任务粒度：分别验证幂等键少于 8/超过 64、模型代码格式、任务类型、提示词空值/超过 10000、提示词任务夹带图片字段、翻译目标语言缺失/少于 2/超过 64、模型不存在、重复幂等键同参重放与异参冲突。
- 图片任务粒度：分别验证清晰度和数量缺失、8K、数量 0/5、非法比例、9 张参考图、非法 UUID、重复参考图、免费会员请求 2K；同时验证完整数字比例、旧比例别名兼容、积分预占、额度不足，以及取消后释放预占积分。
- 队列粒度：连续创建 20 个排队任务并确认列表恰好为 20，第 21 个准确返回 `429/generation_queue_limit_reached`，随后逐项取消并释放资源。
- 支付与订单粒度：分别验证积分包/会员套餐代码格式与不存在、请求幂等键少于 8/超过 64、同参重放、异参冲突、升级目标套餐、升级 quote UUID/不存在、订单 UUID/不存在、支付状态同步和无会员升级限制；支付宝行为仍由 Mock 替代，不代表真实支付验收。
- 上传与通知粒度：分别验证文件名空值/超过 255、MIME 白名单、大小 0/超过 10 MiB、真实 multipart 上传确认删除、空通知列表、全部已读和不存在通知。
- 合法边界粒度：分别验证设备 ID 恰好 8/256、设备名空值/恰好 128、请求幂等键恰好 8/64、提示词恰好 1/10000、翻译语言恰好 2/64、文件名恰好 1/255、文件大小恰好 1/10 MiB、三种允许 MIME，以及 128 字符 `Idempotency-Key` 请求头。
- 认证状态粒度：验证相同邮箱验证码 60 秒冷却、连续 4 次错误仍返回普通错误、第 5 次准确进入尝试次数耗尽、耗尽后正确验证码也不可再使用；同时验证 refresh 所有必填字段、认证请求头的非法版本、dev 最低版本、撤销当前会话、`logout_all` 后旧 Token 立即失效。
- 生成状态粒度：验证所有必填字段逐项缺失、非图片任务逐项夹带图片/翻译字段、列表所有合法状态和合法分页边界、活动任务禁止删除内容、取消重复调用、终态内容删除重复调用、交付确认的 SHA-256/文件大小逐字段校验和不存在结果文件。
- 上传状态粒度：验证完成上传重复调用、删除重复调用、删除后禁止再次完成，并创建恰好 32 个待上传对象验证允许，第 33 个准确返回 `429/reference_upload_limit_reached`，最后逐项删除测试对象。
- 用户隔离粒度：使用两个真实测试账号验证相同幂等键按用户隔离；B 用户无法查询/取消/删除 A 的任务，无法查询/同步 A 的订单，无法撤销 A 的会话，无法完成/删除/引用 A 的参考图，统一返回资源不可见结果。
- HTTP 协议粒度：验证合法 `X-Request-ID` 原样回传、非法请求 ID 由服务端替换、404 统一 JSON 信封、405 方法限制、坏 JSON、超过 64 KiB 请求体、错误信封的 `request_id/data/error` 结构，以及创建任务 202、取消和下单 200 的准确成功状态。
- 认证请求头粒度：分别验证仅发送 Bearer Token、缺少 `X-Token`、缺少版本、缺少设备 ID、伪造 Token 和非法客户端版本，确保认证顺序与业务 `code` 稳定。
- 并发粒度：8 个线程共享客户端同时刷新，确认只发生一次有效 Token 轮换且所有调用拿到同一 Access Token；8 个线程使用同一生成幂等键同时创建，确认只产生一个任务，竞争请求只允许同资源重放或 `request_in_progress`。
- 游标连续性粒度：创建 3 个任务后按 `limit=2` 翻两页，确认 2+1、无重复且末页 cursor 为空；积分流水按 `limit=1` 连续翻页，确认条目不重复。
- 资源和 DTO 不变量粒度：验证参考图绑定任务后禁止删除和重复绑定；账户、会员套餐、积分包、模型、价格、余额、会话、流水的 UUID、十进制字符串、正数、枚举、唯一代码和时间格式均可被客户端稳定解析。
- 首次联测发现新用户响应中的 `nickname` 为 `null`，而客户端 DTO 要求字符串，导致登录响应反序列化失败；客户端已按后端可空契约改为 `Option<String>`，UI 未设置昵称时显示邮箱，复测通过。
- 扩展测试时确认注册赠送积分受 IP 日配额控制；Mock 服务现为每个请求注入独立测试 IP，并在退出时清理配额桶，避免测试顺序影响 100 积分注册赠送，也不影响真实 dev 配额。
- 第二轮细粒度联测发现新的契约偏差：登录接口允许空 `device_name`，`auth_session` 会将空值存为 `null`，账户会话接口直接返回该值；但 OpenAPI 将 `device_name` 定义为非空字符串，客户端 `AccountSessionDto` 也按 `String` 解析，最终导致账户快照协议错误。复现断言明确显示 `account session device_name must follow the OpenAPI string contract, got null`。建议后端会话视图将数据库空值归一为 `""`；本轮仅测试和记录，尚未修改业务实现。
- 第三轮协议联测发现第二个偏差：Koa `allowedMethods()` 生成的 405 响应为 `text/plain; charset=utf-8` 和正文 `Method Not Allowed`，没有进入项目统一的 `{ request_id, data, error }` JSON 信封。404、坏 JSON 和超过 64 KiB 请求体均正确使用 JSON 信封，问题只落在 405 处理链路；建议在统一错误处理中显式包装 405。本轮同样只测试和记录，尚未修改业务实现。
- 当前回归结果：客户端常规测试 29 项全部通过，跨栈测试 39 项中 37 项通过、2 项分别因 `device_name` 和 405 JSON 信封契约偏差失败，`cargo check --locked` 通过；服务端 30 项测试、配置检查和 ESLint 通过。此前通过的真实 HTTP API 流程检查、Mock 会员支付和 Mock 生成集成检查未受测试代码扩展影响。

### 2026-07-16 支付宝应用内二维码 dev 验收

- 后端会员购买、续费、升级和积分充值继续共用 `alipay.trade.page.pay`，并统一发送 `qr_pay_mode=4`、`qrcode_width=220`；嵌入模式不发送 `return_url`。
- macOS 使用 WKWebView、Windows 使用 WebView2，在应用主窗口内加载支付宝签名页面；初始地址只接受支付宝 HTTPS 网关，后续跳转限制为支付宝受信任域名，下载和新窗口被拒绝。
- dev Mock API 只验证会员与积分订单均返回受信任格式的 `checkout_url`、客户端 DTO 解析和订单轮询，不加载真实支付宝交易，也不代表真实付款验收。
- 真实小额支付、公网 HTTPS 异步回调和 Windows release runner 仍属于正式发布阶段，不是当前 dev 阻塞项。

复现时先在服务端仓库启动 Mock API：

```bash
ARTFORGE_CONFIG=configs/dev.local.yaml \
ARTFORGE_ENABLE_MOCK_API=1 \
ARTFORGE_MOCK_PORT=39091 \
ARTFORGE_MOCK_EMAIL_CODE=654321 \
npm run mock:api
```

然后在客户端仓库执行：

```bash
ARTFORGE_CROSS_STACK_BASE_URL=http://127.0.0.1:39091 \
ARTFORGE_MOCK_EMAIL_CODE=654321 \
cargo test -p artforge-studio-native --locked \
  cross_stack_ -- --ignored --nocapture --test-threads=1
```

测试结束后在 Mock API 终端按 `Ctrl+C`，等待输出 `Mock API stopped (SIGINT)`，确保测试数据和连接清理完成。

执行过程中按以下格式追加，不覆盖历史记录：

```text
日期：YYYY-MM-DD
阶段：N
问题：实际 API、UI 或平台限制与计划不一致的内容
决策：采用的实现
影响：涉及文件、兼容性、测试和后续工作
```

当前已确认决策：

- 2026-07-17：前端、后端任务 API 和生产模型目录统一支持完整数字比例列表；旧 `square/landscape/portrait` 只作为兼容别名。
- 2026-07-14：首版最多生成 4 张，与后端限制保持一致。
- 2026-07-14：邀请返利首版隐藏，等待后端独立设计。
- 2026-07-14：昵称首版只读，等待服务端资料修改 API。
- 2026-07-14：prod API 地址不开放给普通用户编辑，dev 通过环境变量覆盖。
- 2026-07-14：支付使用支付宝电脑网站支付，不继续使用本地二维码假流程。
