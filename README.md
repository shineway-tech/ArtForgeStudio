# ArtForge Studio

ArtForge Studio 是使用 Rust 与 Slint 构建的跨平台桌面 AI 美术生产客户端。根 Cargo workspace 只构建 `native-client`，并生成唯一应用二进制 `ArtForgeStudio`。应用版本以 `native-client/Cargo.toml` 为准。

## Supported platforms

- Windows x64
- macOS Intel（x86_64）
- macOS Apple Silicon（aarch64）

正式发布由对应平台 Runner 原生构建。Windows 版本依赖 MSVC、Windows SDK 和 WebView2；macOS 安装包要求 macOS 11 或更高版本。

## Quick start

安装 Rust stable 工具链后，在仓库根目录运行：

```bash
cargo run -p artforge-studio-native --bin ArtForgeStudio
```

客户端默认连接生产 API。开发环境可通过 `ARTFORGE_API_BASE_URL` 覆盖服务地址，具体规则见 [开发指南](docs/DEVELOPMENT.md)。

## Build and test

```bash
cargo check -p artforge-studio-native
cargo test -p artforge-studio-native
cargo build --release -p artforge-studio-native --bin ArtForgeStudio
```

Release 二进制位于 `target/release/ArtForgeStudio`，Windows 下为 `target/release/ArtForgeStudio.exe`。

## Package

macOS：

```bash
./scripts/package-macos.sh x64
./scripts/package-macos.sh aarch64
```

Windows PowerShell：

```powershell
./scripts/package-native-client.ps1 -Target windows
```

本地 macOS 脚本在没有签名身份时生成未签名开发 DMG。正式签名、公证、Windows 安装器和发布制品流程见 [发布指南](docs/RELEASE.md)。

## Repository layout

- `native-client/`：当前产品客户端，也是唯一 Cargo workspace 成员。
- `native-client/src/runtime/`：启动、API、账号、生成、支付、本地存储和展示同步。
- `native-client/ui/`：当前 Slint 状态、组件、页面和弹窗。
- `scripts/`：本地构建、打包和发布辅助脚本。
- `.github/workflows/`：持续集成和桌面端标签发布。
- `assets/sucai/`：随客户端分发的灵感素材。
- `crates/`、根 `ui/`、`schemas/`、`themes/`：早期模块化客户端的历史源码，已排除在当前构建之外。

## Documentation

从 [docs/README.md](docs/README.md) 进入当前文档。历史规划、阶段状态和旧架构不再作为仓库文档保留，需要追溯时使用 Git 历史。
