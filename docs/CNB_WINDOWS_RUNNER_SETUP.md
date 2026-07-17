# CNB Windows Runner 与支付窗口验收

客户端远端位于 CNB。CNB 公共构建节点不提供 Windows，必须由 `honeykid` 根组织管理员接入组织自托管 Windows Runner，仓库根目录的 `.cnb.yml` 才能执行 Windows release 门禁。

## 1. Runner 前置环境

- Windows 11 x64，安装全部系统更新。
- Git for Windows。
- Rust stable，默认目标为 `x86_64-pc-windows-msvc`。
- Visual Studio 2022 Build Tools：勾选“使用 C++ 的桌面开发”、MSVC v143 和 Windows 10/11 SDK。
- Microsoft Edge WebView2 Runtime。
- Runner 用户对工作目录和 Cargo 缓存目录有读写权限。

在 PowerShell 验证：

```powershell
git --version
rustc --version
cargo --version
where.exe cl
```

## 2. 接入 CNB

1. 由根组织管理员进入 `honeykid / 组织设置 / 构建节点`。
2. 新增 Runner，标签至少包含 `windows`。
3. 打开该 Runner 的“连接指引”，在目标 Windows 机器上执行 CNB 生成的一键接入脚本。
4. 等待节点状态变为“在线”并启用节点。
5. `.cnb.yml` 使用 `runner.namespace: group` 和 `tags: [windows]` 调度该节点。

官方说明：https://docs.cnb.cool/zh/build/build-node.html

## 3. 自动门禁

向 `master` 推送或创建目标为 `master` 的合并请求后，CNB 必须依次通过：

```powershell
cargo check -p artforge-studio-native --locked
cargo test -p artforge-studio-native --lib --locked
cargo build -p artforge-studio-native --bin ArtForgeStudio --release --locked
```

最终产物应为 `target\release\ArtForgeStudio.exe`。任一步失败均不得发布。

## 4. WebView2 真机验收

使用 dev 后端和支付宝沙箱/正式小额订单执行：

1. 登录后创建积分充值或会员订单。
2. 支付窗口必须作为客户端子窗口显示，能够通过标题栏和客户端关闭按钮关闭。
3. 仅允许 HTTPS、当前 checkout host 和支付宝域名；HTTP、第三方域名、下载与新窗口导航必须被拦截。
4. 关闭窗口不能取消服务端订单，客户端仍应继续同步订单。
5. 支付成功后验证 `paid + retry_pending` 显示“权益处理中”，不得降级为失败。
6. 公网异步回调完成后验证积分或会员到账、通知更新、pending order 恢复记录删除。
7. 重启客户端，验证未完成订单能够继续恢复。

验收时记录 Windows 版本、WebView2 版本、CNB 构建链接、订单号和服务端 request ID；禁止记录 token、签名 URL、支付 URL 或密钥。
