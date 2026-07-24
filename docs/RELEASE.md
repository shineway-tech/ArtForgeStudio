# Release

## Version and tag

应用版本来自 `native-client/Cargo.toml` 的 package version。发布标签必须使用 `vX.Y.Z`，且去掉 `v` 后与 Cargo 版本完全一致；发布工作流会在打包前验证，不一致时直接失败。

建议在干净工作区完成版本更新、检查和测试后创建标签：

```bash
git tag vX.Y.Z
git push origin vX.Y.Z
```

不要为同一版本重复使用或移动已经发布的标签。

## Local packages

macOS DMG：

```bash
./scripts/package-macos.sh x64
./scripts/package-macos.sh aarch64
```

输出：

```text
dist/ArtForgeStudio_<version>_macos_x64.dmg
dist/ArtForgeStudio_<version>_macos_aarch64.dmg
```

脚本读取 Cargo 版本、原生构建对应 Rust target、组装 `.app`、校验 `Info.plist` 并创建压缩 DMG。没有设置 `APPLE_SIGNING_IDENTITY` 时，脚本会明确生成未签名开发 DMG，不能当作正式发布包。

Windows PowerShell：

```powershell
./scripts/package-native-client.ps1 -Target windows
```

该命令生成完整免安装目录和 portable ZIP。`scripts/build-release.ps1` 会先执行测试和检查，再调用同一个 Windows 打包入口；可用 `-SkipTests` 跳过前置验证，但正式发布不应跳过。

## Release workflow

`.github/workflows/release-desktop.yml` 仅在推送 `v*` 标签时触发：

- `macos-15` 分别构建 Intel 与 Apple Silicon DMG。
- `windows-2025` 构建 Windows x64 portable ZIP，并使用 Inno Setup 生成安装器。
- macOS 导入 Developer ID Application 证书，签名应用，提交公证并 staple DMG。
- 四个制品上传为 GitHub Actions artifacts。
- 四个制品同时上传到 OSS 的版本路径和稳定路径。
- 所有平台制品上传成功后，生成并发布固定地址的 `update-manifest.json`，客户端据此在启动时提示新版本。

普通 `master` push、目标为 `master` 的 Pull Request 和手动验证由 `.github/workflows/ci.yml` 处理，只执行三平台 check/test，不发布制品。

## Artifacts

| Platform | Artifact |
|---|---|
| Windows x64 installer | `ArtForgeStudio_<version>_windows_x64_setup.exe` |
| Windows x64 portable | `ArtForgeStudio_<version>_windows_x64_portable.zip` |
| macOS Intel | `ArtForgeStudio_<version>_macos_x64.dmg` |
| macOS Apple Silicon | `ArtForgeStudio_<version>_macos_aarch64.dmg` |

Windows portable ZIP 包含 `ArtForgeStudio.exe`、随包素材和初始 `data` 目录；必须完整解压后运行。用户迁移 portable 目录时应保留相邻 `data`。

上传 OSS 时，脚本为每个制品生成：

```text
<prefix>/<version>/<stable-file-name>
<prefix>/<stable-file-name>
```

稳定文件名会移除文件名中的版本号，供固定下载入口使用。

更新清单同时发布到版本路径和稳定路径：

```text
<prefix>/<version>/update-manifest.json
<prefix>/update-manifest.json
```

客户端启动和手动检查版本时读取稳定路径。普通更新可以稍后处理；服务端返回最低版本限制时，客户端会显示不可关闭的更新提示，并保留已登录设备的离线入口。

## Required secrets

只在 GitHub 仓库 Secrets 中配置值，不要写入代码、文档或日志。

Apple 签名与公证：

- `APPLE_CERTIFICATE`
- `APPLE_CERTIFICATE_PASSWORD`
- `KEYCHAIN_PASSWORD`
- `APPLE_API_ISSUER`
- `APPLE_API_KEY`
- `APPLE_API_KEY_BASE64`

OSS 上传：

- `ALIYUN_OSS_REGION`
- `ALIYUN_OSS_BUCKET`
- `ALIYUN_OSS_ACCESS_KEY_ID`
- `ALIYUN_OSS_ACCESS_KEY_SECRET`
- `ALIYUN_OSS_ENDPOINT`（可选）
- `ALIYUN_OSS_PUBLIC_BASE_URL`（可选）
- `ALIYUN_OSS_PREFIX`（可选）
- `ALIYUN_OSS_OBJECT_ACL`（可选）

`APPLE_SIGNING_IDENTITY` 由工作流导入证书后写入运行环境，本地打包时也可显式提供；它不是需要提交到仓库的配置文件。

## macOS signing and notarization

标签工作流执行：

1. 从 `APPLE_CERTIFICATE` 解码 `.p12`，导入临时 keychain。
2. 查找 Developer ID Application 身份并设置 `APPLE_SIGNING_IDENTITY`。
3. `package-macos.sh` 使用 hardened runtime、timestamp 和深度签名。
4. 从 `APPLE_API_KEY_BASE64` 写入临时 App Store Connect API key。
5. 使用 `notarytool submit --wait` 提交 DMG。
6. 使用 `stapler` staple 并验证 DMG。

本地未签名 DMG 只用于开发冒烟。正式包需要在目标架构真机验证 Gatekeeper、首次启动、登录、生成和更新入口。

## Windows packaging

Windows Runner 使用 MSVC 构建 release 二进制。`package-native-client.ps1` 组装 portable 目录、素材和数据目录，再创建 ZIP。

CI 随后查找或安装 Inno Setup，并使用 `installer/ArtForgeStudio.iss` 生成当前用户安装器。安装器输入目录、版本和输出目录由工作流通过以下临时环境变量传入：

- `ARTFORGE_APP_VERSION`
- `ARTFORGE_PACKAGE_DIR`
- `ARTFORGE_RELEASE_DIR`

这些变量是安装器构建接口，不是客户端运行时配置。

## Release checklist

标签前：

1. 确认 `native-client/Cargo.toml` 版本与计划标签一致。
2. 确认 `Cargo.lock` 已提交，工作区没有意外改动。
3. 执行 `cargo check -p artforge-studio-native`。
4. 执行 `cargo test -p artforge-studio-native`。
5. 在目标平台完成必要的启动和核心流程冒烟。
6. 确认 Apple 与 OSS Secrets 已配置且未过期。

标签后：

1. 确认 macOS Intel、macOS Apple Silicon 和 Windows x64 jobs 全部成功。
2. 下载并核对四个 Actions 制品的文件名、版本、大小和可打开性。
3. 核对四个 OSS 版本 URL 与四个稳定 URL。
4. 在 Intel Mac、Apple Silicon Mac 和 Windows x64 真机分别安装/解压并启动。
5. 验证现有作品目录、升级数据、登录、模型目录、生成、下载和通知。
6. 记录工作流链接、版本、制品校验值和验收平台；禁止记录 Token、支付 URL、签名 URL 或密钥。

## Windows payment and WebView2 acceptance

使用发布候选后端和支付宝沙箱或正式小额订单执行：

1. 登录后分别创建积分充值和会员订单。
2. 支付窗口作为客户端内嵌窗口显示，可正常关闭和重新打开。
3. 只允许 HTTPS、当前收银台主机和支付宝可信域名；HTTP、第三方域名、下载和新窗口导航必须被拦截。
4. 关闭支付窗口不取消服务端订单，客户端继续同步状态。
5. `paid` 且权益仍在重试时显示“权益处理中”，不能降级为失败。
6. 公网异步回调完成后，积分或会员到账、通知更新并清理待恢复订单。
7. 重启客户端后，未完成订单仍可恢复。

验收记录包含 Windows、WebView2、Actions 工作流链接、订单号和服务端请求标识；不得记录 Token、签名 URL、支付 URL 或密钥。
