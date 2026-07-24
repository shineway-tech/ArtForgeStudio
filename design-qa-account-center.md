# 账号中心第三种方案设计 QA

## QA 基准

- Source of visual truth: `/Users/fanxiao/.codex/generated_images/019f6f61-2d31-7ee2-85b4-3e72ded67e2b/exec-cbc6e6b8-5176-4eae-a386-5130dd099fea.png`
- Implementation screenshots: `/tmp/artforge-account-center-implementation-v2.png`, `/tmp/artforge-account-overview-after.png`
- Full-view comparisons: `/tmp/artforge-account-center-comparison.png`, `/tmp/artforge-account-overview-comparison.png`
- Focused overview comparison: `/tmp/artforge-account-overview-focused-comparison.png`
- Viewport/state: 1440 × 928 logical desktop window, macOS light theme；登录方式与账户概览页签分别检查。
- Source dimensions: 1487 × 1058 px.
- Implementation dimensions: 3016 × 1992 px, captured from a 1440 × 928 logical window on a HiDPI display. 用户问题截图为 1756 × 1052 px。
- Density normalization: 对比图按视觉高度归一，避免物理像素密度影响尺寸判断。

## Primary interactions checked

- 顶部账号入口打开账号中心，并默认定位到“登录方式”。
- 左侧“账户概览 / 登录方式 / 登录设备”均绑定独立内容区。
- 微信未绑定时显示“立即绑定”，已绑定时显示“解除绑定”，并保留确认流程。
- 邮箱未绑定时显示“绑定邮箱”，已绑定时展示脱敏邮箱。
- 登录设备保留单设备退出，底部保留关闭、退出登录和退出全部设备。
- 本次只做编译和界面检查，未运行单元测试。

## Comparison history

### Pass 1

- P1: 主内容标题与登录方式卡片标题发生重叠。
- P2: 登录方式行的通用 `title` 属性与弹窗标题命名冲突，导致定位不可控。

Fixes applied:

- 将登录方式行标题属性改为 `method-title`。
- 明确设置主内容标题的位置和层级。

### Pass 2

- Typography: 标题、侧栏、卡片标题、状态与说明层级清楚，无截断或异常换行。
- Spacing/layout: 弹窗、侧栏、内容卡片、底部操作区与选定方案的结构和留白一致。
- Colors/tokens: 复用 ArtForge 品牌紫、浅灰面板、成功绿和警示红；状态与操作语义一致。
- Image/icon asset fidelity: 账户、登录方式、设备、邮箱和安全提示均使用同一套 Bootstrap SVG 图标，风格统一。
- Copy/content: “账号中心”“登录方式”“安全建议”及绑定状态文案与选定方案一致；实际绑定状态由服务端数据决定。
- Focused-region review: 登录方式内容区已经包含所有高风险视觉元素，完整窗口对比足以覆盖标题、状态、图标和按钮，无需额外局部截图。
- No actionable P0/P1/P2 visual issues remain.

### Pass 3 — 账户概览响应式修正

- P1: 账户概览标题、账号、会员和积分字段在实际窗口中出现错位与重叠。
- Evidence before fix: `/tmp/artforge-account-overview-before.png`，与用户提供的 `/var/folders/1j/k6y_5_2x7td4514dgwwm58z80000gn/T/codex-clipboard-e6e2bda0-8f3e-4576-aa19-6d79b1675128.png` 表现一致。

Fixes applied:

- 新增 `AccountOverviewRow`，将标签、值和按钮拆成明确列，并为所有元素声明固定起点和可伸缩宽度。
- 为账户概览和登录设备标题、说明与分隔线补齐显式坐标，防止自由布局中的默认位置发生漂移。

Post-fix evidence:

- Full view: `/tmp/artforge-account-overview-comparison.png` 左侧为修正前、右侧为修正后，相同窗口、账号和页签状态。
- Focused region: `/tmp/artforge-account-overview-focused-comparison.png` 清晰确认标题回到内容顶部，三行标签、值和按钮互不重叠。
- Typography: 标题和正文层级恢复正常，邮箱、会员等级与积分均完整可读。
- Spacing/layout: 三列对齐稳定，分隔线与行高一致，按钮保持右对齐。
- Colors/tokens: 未改变现有品牌色、文字色和边框色。
- Image/icon asset fidelity: 本次问题区域不含新增图像资产，侧栏图标保持原 SVG 资源。
- Copy/content: 原有账号、会员和积分文案及真实数据保持不变。
- No actionable P0/P1/P2 visual issues remain.

## Runtime notes

- Native client, so browser console checks are not applicable.
- QA 时仅临时把账号中心设为默认打开；最终代码已恢复为正常关闭状态。
- 对比图中的“未绑定”与设计稿中的“已绑定”属于账号数据状态差异，组件已覆盖两种状态。

final result: passed
