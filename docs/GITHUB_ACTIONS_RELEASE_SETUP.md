# GitHub Actions 桌面端发布流程

客户端远端位于 GitHub。`.github/workflows/release-desktop.yml` 使用 GitHub 托管 Runner 构建三个独立版本：

| 平台 | Runner | Rust target | 产物 |
|---|---|---|---|
| Windows x64 | `windows-2025` | `x86_64-pc-windows-msvc` | `ArtForgeStudio_<version>_windows_x64_setup.exe`、`ArtForgeStudio_<version>_windows_x64_portable.zip` |
| macOS Intel | `macos-15` | `x86_64-apple-darwin` | `ArtForgeStudio_<version>_macos_x64.dmg` |
| macOS Apple Silicon | `macos-15` | `aarch64-apple-darwin` | `ArtForgeStudio_<version>_macos_aarch64.dmg` |

## 触发规则

- 推送到 `master`、创建目标为 `master` 的 Pull Request，或手动运行 Desktop CI 时：在三个目标上执行 check 和 test。
- 推送 `v*` 标签时：执行 release 打包；macOS 额外完成 Developer ID 签名、公证和 stapling；四个发布文件同时上传 OSS。
- 普通分支和 Pull Request 不访问发布密钥，也不会写入 OSS。

版本号以 `native-client/Cargo.toml` 的 package version 为准。发布标签必须与版本一致，例如版本 `1.0.0` 使用 `v1.0.0`。

## GitHub Secrets

OSS 上传沿用 `market_tool` 的变量名：

- `ALIYUN_OSS_REGION`
- `ALIYUN_OSS_BUCKET`
- `ALIYUN_OSS_ACCESS_KEY_ID`
- `ALIYUN_OSS_ACCESS_KEY_SECRET`
- `ALIYUN_OSS_ENDPOINT`（可选）
- `ALIYUN_OSS_PUBLIC_BASE_URL`（可选，默认 `https://cdn.honeykid.cn`）
- `ALIYUN_OSS_PREFIX`（可选，默认 `public/artforge_studio`）
- `ALIYUN_OSS_OBJECT_ACL`（可选）

macOS 签名和公证需要：

- `APPLE_CERTIFICATE`：Developer ID Application `.p12` 的 Base64 内容
- `APPLE_CERTIFICATE_PASSWORD`
- `KEYCHAIN_PASSWORD`
- `APPLE_API_ISSUER`
- `APPLE_API_KEY`
- `APPLE_API_KEY_BASE64`：App Store Connect API `.p8` 的 Base64 内容

不要把证书、私钥或 OSS 凭据写入仓库文件或 Actions 日志。

## OSS 对象路径

每个文件会同时写入版本路径和稳定路径。例如 `1.0.0` 的 Apple Silicon DMG：

```text
public/artforge_studio/1.0.0/ArtForgeStudio_macos_aarch64.dmg
public/artforge_studio/ArtForgeStudio_macos_aarch64.dmg
```

版本路径用于历史版本和回滚；稳定路径用于固定下载入口。

Windows 免安装版对应：

```text
public/artforge_studio/1.0.0/ArtForgeStudio_windows_x64_portable.zip
public/artforge_studio/ArtForgeStudio_windows_x64_portable.zip
```

Windows 同时上传 Inno Setup 安装器和免安装 ZIP。安装器把程序和官方素材安装到当前用户目录：

```text
%LOCALAPPDATA%\Programs\ArtForgeStudio\ArtForgeStudio.exe
%LOCALAPPDATA%\Programs\ArtForgeStudio\assets\sucai\...
```

免安装 ZIP 包含 `ArtForgeStudio.exe`、`assets` 和初始 `data` 目录。使用时需要先完整解压，再运行其中的 EXE；不要只复制裸 EXE。用户配置和生成内容会保存在解压目录旁的 `data` 中，因此移动或升级绿色版时应一并保留该目录。

## 发布步骤

```bash
git tag v1.0.0
git push origin v1.0.0
```

标签工作流全部通过后，核对四个 Actions 制品、四个 OSS 稳定 URL 和对应版本 URL。Windows 应分别验证安装版，以及绿色版解压、启动和携带 `data` 目录移动；macOS DMG 还应在 Intel 与 Apple Silicon 真机分别打开验证。

## Windows WebView2 真机验收

使用生产候选后端和支付宝沙箱或正式小额订单执行：

1. 登录后创建积分充值或会员订单。
2. 支付窗口作为客户端子窗口显示，并可正常关闭。
3. 仅允许 HTTPS、当前 checkout host 和支付宝域名；拦截 HTTP、第三方域名、下载与新窗口导航。
4. 关闭窗口不取消服务端订单，客户端继续同步订单。
5. `paid + retry_pending` 显示“权益处理中”，不能降级为失败。
6. 公网异步回调完成后，积分或会员到账、通知更新、pending order 恢复记录删除。
7. 重启客户端后，未完成订单可以继续恢复。

验收时记录 Windows、WebView2、Actions 运行链接、订单号和服务端 request ID；禁止记录 token、签名 URL、支付 URL或密钥。
