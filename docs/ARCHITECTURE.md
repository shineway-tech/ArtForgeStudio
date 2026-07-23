# Architecture

## Repository and binary boundary

根 Cargo workspace 只有 `native-client` 一个成员。它构建包 `artforge-studio-native`，并生成唯一应用二进制 `ArtForgeStudio`（Windows 为 `ArtForgeStudio.exe`）。

`AppContext` 持有运行期共享状态、平台 API 后端和生成任务注册表。Slint 只保存展示状态；后台网络、轮询、下载和磁盘操作完成后，通过事件循环同步回 UI。

## Runtime layers

- `native-client/src/main.rs`：平台可执行入口。
- `native-client/src/lib.rs`：库入口，连接 Slint 编译产物与运行时，也是单元测试入口。
- `native-client/src/runtime/app.rs`：渲染器选择、窗口启动、初始数据加载和顶层 callback 接线。
- `native-client/src/runtime/callbacks/`：认证、账号绑定、会员积分、模型目录、参考图、生成、支付、通知和查看器的 UI 适配层。
- `native-client/src/runtime/api/`：HTTP 客户端、安全会话、账号快照、会员、订单、通知、上传和生成任务契约。
- `native-client/src/runtime/generation/`：任务准备、提交、轮询、取消、恢复和结果交付。
- `native-client/src/runtime/storage/`：应用目录、本地展示数据、生成恢复和订单恢复。
- `native-client/src/runtime/features/`：灵感素材与作品查看器等独立功能。
- `native-client/src/runtime/presentation/`：服务端/本地模型到 Slint 的同步，以及主题应用。
- `native-client/src/runtime/services/image_processing.rs`：仅处理本地图片；模型请求和支付请求不能绕过平台 API。
- `native-client/src/runtime/payment_window.rs`、`agreement_window.rs`：受信任 HTTPS 内容的嵌入式窗口与导航白名单。

依赖方向以“Slint callback → 运行时能力 → 平台 API 或本地存储 → UI 同步”为主。API 层不读取 Slint 状态，Slint 文件也不直接发起 HTTP 请求。

## Slint layers

- `native-client/ui/app.slint`：`AppWindow`、页面路由和弹窗组合。
- `native-client/ui/app-state.slint`：全局展示状态和 Rust callback 接口。
- `native-client/ui/types.slint`：Rust 可见的 UI 数据结构。
- `native-client/ui/theme.slint`：运行时颜色、字体和间距。
- `native-client/ui/components/`：可复用控件和业务组件。
- `native-client/ui/pages/`：欢迎、创作、资产、灵感、通知、积分会员、模型和设置页面。
- `native-client/ui/dialogs/`：认证、账号、协议、支付、版本、查看器和确认流程。

`app.slint` 负责组合，不承载远端业务规则。会员权益、价格、模型能力和任务状态由 Rust 从服务端响应转换成展示模型。

## Startup and account bootstrap

启动顺序由 `runtime/app.rs` 组织：

1. 在没有显式 `SLINT_BACKEND` 时选择平台默认渲染器。
2. 创建 Slint 窗口并设置版本、主题和应用目录。
3. 加载用户界面偏好、本地作品数据和内置灵感素材。
4. 创建 `AppContext` 与 API 后端，向 Slint 推送本地展示状态。
5. 注册所有 callback。
6. 初始化安全会话并执行在线账号启动流程。
7. 进入 Slint 事件循环。
8. 正常退出时保存提示词草稿、界面偏好和本地展示数据。

在线启动会同步账号、会员、积分、模型目录、设备会话和通知等服务端快照。网络不可用、会话失效、协议更新和最低版本要求是不同状态，不能互相降级或混用。

## Image and prompt task flow

图片与提示词任务都通过平台 API：

1. callback 读取并规范化用户选择，检查登录、模型和会员画质权限。
2. 客户端生成稳定的请求标识，并在首次远端写入前保存恢复记录。
3. 参考图按服务端上传契约提交，任务请求只引用服务端文件标识。
4. 平台 API 创建提示词或图片任务；超时恢复必须复用原请求标识，避免重复任务和重复扣费。
5. 运行时轮询或恢复服务端任务状态，把排队、执行、部分成功、完成、失败和取消同步到 UI。
6. 取消操作提交到服务端，本地状态不能假定远端已经取消。

模型目录、可用画质、操作价格和扣费结果全部来自服务端。客户端只展示和提交服务端支持的代码值。

## Payment flow

会员购买、续费、升级和积分充值共用服务端订单流程：

1. 客户端使用稳定请求标识创建订单，并持久化待恢复记录。
2. 服务端返回通过白名单校验的 HTTPS 收银台地址。
3. 客户端在嵌入式 WebView 中显示支付内容，同时继续同步订单状态。
4. 关闭支付窗口不取消订单；重启后仍可恢复未完成订单。
5. 只有服务端订单与权益结果可以更新会员或积分展示。
6. `paid` 但权益仍在重试时保持“处理中”，不能降级为失败。

## Result delivery and local recovery

生成结果按以下顺序交付：

1. 从服务端提供的临时地址下载结果。
2. 校验文件大小与 SHA-256。
3. 原子写入用户作品目录。
4. 更新本地作品元数据和生成记录。
5. 向服务端确认该结果已经交付。

`pending-generations.json` 保存生成请求、服务端任务、下载结果和确认状态；`pending-orders.json` 保存待同步订单。写入恢复文件使用临时文件替换，终态生成只有在全部成功结果确认交付后才从恢复记录移除。

本地展示缓存不能覆盖服务端任务、积分、订单或支付事实。磁盘写入或交付确认失败时必须保留可恢复状态。

## Platform differences

- Windows 默认设置 `SLINT_BACKEND=winit-femtovg`，使用 GPU 渲染改善最小化后恢复体验。
- 非 Windows 默认设置 `SLINT_BACKEND=winit-software`。
- 用户显式设置 `SLINT_BACKEND` 时，客户端不覆盖该值。
- Windows 的 `wry` 后端使用 WebView2，macOS 使用 WKWebView；支付和协议窗口均限制为 HTTPS 与明确的可信主机。
- Windows 拖拽、窗口和安装器行为需要 Windows MSVC、Windows SDK 与真机验证。
- macOS 分别发布 Intel 与 Apple Silicon 安装包，最低系统版本由打包脚本写入应用元数据。

## Security boundary

- 平台模型密钥不进入客户端。
- Access Token 只保存在内存；Refresh Token 使用系统安全凭据或受限的本地安全存储。
- API Key、Token、验证码、Prompt、参考图内容、支付 URL、签名 URL 和协议正文不得进入普通日志。
- HTTP 错误向用户展示可理解的消息，不泄露内部业务码、请求 ID 或原始服务端响应。
- 支付和协议 WebView 拒绝 HTTP、非白名单主机、下载和任意新窗口导航。
- 账号、会员、积分、订单、模型和任务的远端状态不能由本地文件直接修改。

## Historical source boundary

`crates/`、根 `ui/`、`schemas/` 和 `themes/` 属于早期模块化重构源码，已从 Cargo workspace 排除，不参与当前构建、测试和发布。`assets/sucai/` 仍作为当前客户端的随包灵感素材使用。

历史源码只用于 Git 追溯。新功能不得在历史目录实现，也不得重新引入客户端直连 Provider、可编辑 Endpoint/API Key 或本地积分逻辑。
