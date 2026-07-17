# ArtForge Studio

桌面端 AI 美术生产套件。当前 workspace 只包含正式产品客户端
`native-client`，唯一应用二进制为 `ArtForgeStudio`。

`crates/` 保留早期模块化迁移源码作为历史参考，但已排除在 workspace
之外，不参与构建、测试或发布。

详见 `docs/rewrite/00-index.md`。

客户端与服务端接入进度见
[`docs/FRONTEND_BACKEND_INTEGRATION_EXECUTION_PLAN.md`](docs/FRONTEND_BACKEND_INTEGRATION_EXECUTION_PLAN.md)，
旧版客户端迁移规则见 [`native-client/MIGRATION.md`](native-client/MIGRATION.md)。

## 构建

```sh
cargo build --release
```

输出为 `target/release/ArtForgeStudio`（Windows 下为 `.exe`）。
即使使用 `cargo build --release --workspace`，也只会构建这个客户端。

## 项目结构

- `native-client` — 当前 ArtForgeStudio 产品客户端和唯一 workspace 成员
- `native-client/src/runtime` — 回调、生成流程、服务、存储和展示逻辑
- `native-client/ui` — Slint 状态、组件、页面和弹窗
- `native-client/MIGRATION.md` — 旧 Provider、本地账号/积分与作品数据的迁移说明
- `crates` — 已排除的历史模块化源码

## 平台

`ArtForgeStudio` 当前可在 Windows 和 macOS 构建。

正式支付窗口仅在 Windows 使用 WebView2；macOS 开发环境会使用系统浏览器打开同一服务端收银台 URL。

## GitHub Actions 发布

`.github/workflows/release-desktop.yml` 构建并上传三个独立制品：包含完整素材的 Windows x64 安装器 EXE、macOS Intel DMG 和 macOS Apple Silicon DMG。普通分支、Pull Request 和手动运行只上传 Actions 制品；`v*` 标签还会签名、公证 macOS 应用，并把三个安装文件上传 OSS。

Secrets、OSS 路径和发版步骤见 [`docs/GITHUB_ACTIONS_RELEASE_SETUP.md`](docs/GITHUB_ACTIONS_RELEASE_SETUP.md)。
