# Development

## Prerequisites

通用环境：

- Rust stable 工具链，Cargo 可用。
- Git。
- 能够访问 Cargo registry 的网络环境。

平台环境：

- Windows：MSVC 工具链、Windows SDK 和 WebView2 Runtime。正式 Windows 验证必须在 Windows 主机或 Windows Runner 执行。
- macOS：Xcode Command Line Tools。DMG 打包还需要系统自带的 `plutil`、`hdiutil`、`codesign`，以及脚本读取 Cargo metadata 所需的 Python 3。

当前包使用 Rust edition 2021；具体依赖和版本以 `native-client/Cargo.toml` 与 `Cargo.lock` 为准。

## Run locally

在仓库根目录运行：

```bash
cargo run -p artforge-studio-native --bin ArtForgeStudio
```

开发构建使用当前平台目标。首次构建会编译 Slint、渲染器、HTTP 和 WebView 相关依赖，耗时明显长于后续增量启动。

## Check and test

```bash
cargo check -p artforge-studio-native
cargo test -p artforge-studio-native
cargo build --release -p artforge-studio-native --bin ArtForgeStudio
```

单元测试中的本地 HTTP 测试需要允许测试进程监听回环端口。`cross_stack_` 测试默认标记为 ignored，不会在普通 `cargo test` 中访问外部后端。

提交前至少执行 `cargo check`、`cargo test` 和 `git diff --check`。平台相关改动还要在对应平台执行目标检查与真机冒烟。

## API environment overrides

客户端支持以下开发环境变量：

| Variable | Scope | Purpose |
|---|---|---|
| `ARTFORGE_API_BASE_URL` | Debug client only | 覆盖客户端平台 API 根地址 |
| `ARTFORGE_CROSS_STACK_BASE_URL` | Ignored cross-stack tests | 指定已启动的后端 Mock API |
| `ARTFORGE_MOCK_EMAIL_CODE` | Ignored cross-stack tests | 指定 Mock API 验证码，未设置时测试使用 `654321` |
| `SLINT_BACKEND` | Client startup | 显式覆盖 Slint 渲染后端 |

`ARTFORGE_API_BASE_URL` 只在 debug 构建读取；release 构建使用编译时确定的生产 HTTPS 地址，普通用户不能通过环境变量改写。开发地址可以带路径，客户端会规范化尾部 `/`。

示例：

```bash
ARTFORGE_API_BASE_URL=http://127.0.0.1:39091 \
cargo run -p artforge-studio-native --bin ArtForgeStudio
```

## Cross-stack Mock API tests

跨栈测试使用真实 Rust API 层访问后端仓库提供的 dev Mock API。先在后端仓库按其当前文档启动 Mock API，再在客户端仓库执行：

```bash
ARTFORGE_CROSS_STACK_BASE_URL=http://127.0.0.1:39091 \
ARTFORGE_MOCK_EMAIL_CODE=654321 \
cargo test -p artforge-studio-native --locked \
  cross_stack_ -- --ignored --nocapture --test-threads=1
```

后端启动、依赖和数据清理属于后端仓库职责，本仓库不复制其命令。测试结束后确认后端 Mock API 已正常停止并清理测试数据。

## Renderer selection

客户端在 `SLINT_BACKEND` 未设置时选择默认值：

- Windows：`winit-femtovg`，使用 GPU 渲染。
- 非 Windows：`winit-software`。

排查渲染问题时可临时显式设置：

```bash
SLINT_BACKEND=winit-software \
cargo run -p artforge-studio-native --bin ArtForgeStudio
```

显式覆盖只用于诊断或兼容性验证。Windows 最小化恢复性能需要用默认 GPU 后端进行最终验收。

## Platform-specific verification

GitHub Desktop CI 对以下目标分别执行 locked check 和 library test：

- `x86_64-pc-windows-msvc`
- `x86_64-apple-darwin`
- `aarch64-apple-darwin`

Windows 目标需要 Windows MSVC、SDK 和 WebView2 环境；在 macOS 添加 Windows target 不能替代 Windows 构建。支付窗口、协议窗口、拖拽、缩放、最小化恢复和安装目录行为都必须在对应系统真机确认。

macOS 需要分别验证 Intel 与 Apple Silicon 制品。跨架构构建仍依赖 macOS SDK、链接器和对应 Rust target。

## Troubleshooting

- Cargo 长时间停在 `Compiling artforge-studio-native`：通常是最终链接或 Thin LTO；先检查 `rustc` 是否仍占用 CPU。
- 本地 HTTP 单元测试报 `Operation not permitted`：执行环境禁止监听回环端口，应在允许本地端口的环境重跑。
- 启动后无窗口：检查是否已有单实例进程，再查看终端中的 Slint 渲染后端错误。
- Windows 从任务栏恢复卡顿：确认未覆盖 `SLINT_BACKEND`，并检查当前是否使用 `winit-femtovg`。
- API 请求全部失败：确认 debug 环境变量地址合法、后端可达，且没有把 release 包当作可覆盖 API 地址的开发包。
- 登录反复失效：区分网络失败、Access Token 刷新、Refresh Token 撤销、设备会话撤销、协议升级和最低版本要求。
- 生成结果存在但未显示：分别检查作品文件、本地元数据与 `pending-generations.json`，不要直接删除作品目录。
- 支付窗口打不开：确认 URL 为允许的 HTTPS 主机，并检查 WebView2 或 WKWebView 运行环境。
