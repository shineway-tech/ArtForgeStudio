# 无限画布节点交互 Design QA

## 视觉基准

- 图片节点信息弹窗：`C:\Users\deyx1\AppData\Local\Temp\codex-clipboard-c3cda497-abe0-4444-9994-24d379d33402.png`
- 文本节点信息弹窗：`C:\Users\deyx1\AppData\Local\Temp\codex-clipboard-687bf9ce-0bf7-44b5-bee6-49308649a5a7.png`
- 文本节点选中工具栏：`C:\Users\deyx1\AppData\Local\Temp\codex-clipboard-9ce8c96e-3ef7-42ba-8d3c-59b7f58d96a0.png`
- 合并编辑入口：`C:\Users\deyx1\AppData\Local\Temp\codex-clipboard-6c11df01-c8c3-4492-bc64-6618c8e8d08c.png`
- 字号缩小/放大：`C:\Users\deyx1\AppData\Local\Temp\codex-clipboard-3d543828-8a8c-452d-9c05-26ba427e8a63.png`
- 文字节点 AI 优化：`C:\Users\deyx1\AppData\Local\Temp\codex-clipboard-8457d0fe-0abc-4474-b214-53ead88f9728.png`
- 图片质量选择：`C:\Users\deyx1\AppData\Local\Temp\codex-clipboard-39a47f93-2a25-4e67-9d0f-b6ffa28d9542.png`
- 默认移动画布：`C:\Users\deyx1\AppData\Local\Temp\codex-clipboard-d1eacdf1-82f9-487a-9d64-f897f06e3b3c.png`

## 实现截图

- 节点信息弹窗：`design-qa-canvas-node-info.png`
- 节点信息弹窗对比：`design-qa-canvas-node-info-comparison.png`
- 文本节点工具栏：`design-qa-canvas-text-toolbar.png`
- 文本节点工具栏对比：`design-qa-canvas-text-toolbar-comparison.png`
- 图片节点缩略图：`design-qa-canvas-image-preview.png`
- 本轮完整画布：`design-qa-canvas-followup-full.png`
- 本轮文字节点聚焦图：`design-qa-canvas-followup.png`
- AI 优化节点对比：`design-qa-canvas-followup-comparison.png`
- 图片质量设置：`design-qa-canvas-quality-settings.png`
- 图片质量设置对比：`design-qa-canvas-quality-settings-comparison.png`

## 测试环境与状态

| 场景 | 基准尺寸 | 实现尺寸 | 像素密度 | 状态 |
|---|---:|---:|---:|---|
| 图片节点信息弹窗 | 627 × 693 | 627 × 693 | 1.0 | 图片节点已选中，信息页签打开 |
| 文本节点工具栏 | 831 × 310 | 831 × 310 | 1.0 | 文本节点已选中，节点上方显示 7 个操作 |
| 图片节点缩略图 | — | 582 × 350 | 1.0 | 图片节点已选择本地图片 |
| AI 优化文字节点 | 663 × 441 | 650 × 464 | 1.0 | 100% 缩放、18px 节点字号、已配置推理模型 |
| 图片质量设置 | 594 × 813 | 440 × 560 | 1.0 | 2K 选中、会员最高质量为 4K |

实现截图由 1180 × 800 的 Slint 离屏窗口渲染后按目标区域裁切。对比图左侧为视觉基准，右侧为实现结果。来源截图包含不同的桌面缩放比例，因此对比以组件结构、对齐、间距和状态为准，不把纯密度差异列为问题。

## 视觉核对

- 字体与文案：标题、字段、状态、工具栏中文文案均与基准一致；信息弹窗支持“信息 / JSON”页签。
- 间距与尺寸：弹窗为 520 × 606，字段行、页签、关闭按钮位置与基准对齐；文本工具栏宽 648、高 48，7 个按钮等宽分布。
- 颜色与设计令牌：弹窗使用现有暗色画布令牌；工具栏保留当前主题强调色，因此与基准截图中的蓝色描边存在允许的主题差异。
- 图标：复用项目现有 24 × 24 单色 SVG；个别图标造型与基准略有差别，但语义一致。
- 图片质量：上传图片按 `contain` 等比缩放，完整显示在节点内；节点容器启用裁切，图片不会越过边框。
- 背景层级：信息弹窗打开后隐藏画布缩放条与底部工具栏，避免控件透过遮罩干扰弹窗。
- 本轮节点布局：AI 优化位于左上角、生图位于右上角；两者与正文保持清晰间距。图片设置新增质量行后仍保持与宽高比、生成张数一致的纵向节奏。

## 交互核对

- 图片、文本节点的信息按钮均接入同一个节点信息弹窗。
- 信息弹窗支持关闭按钮、点击遮罩关闭、信息与 JSON 页签切换。
- 文本节点工具栏包含信息、删除、存素材、编辑、生图、缩小、放大；双击文字节点与“编辑”按钮均进入同一编辑状态。
- 缩小/放大只调整当前文字节点字号，每次 1px，限制为 8–72px，并写入画布持久化数据，不改变节点宽高。
- AI 优化复用现有服务端提示词优化任务；优化成功后通过画布更新回调写回当前文字节点，并进入画布撤销历史。
- 图片设置新增 1K、2K、4K 质量选项，遵守会员最高质量限制，摘要同步显示质量、宽高比和生成张数。
- 底部工具栏移除“选择”按钮，默认并持续使用“移动画布”；节点仍可左键选中和拖动。
- 图片选择器仅接收 PNG、JPG/JPEG、WebP；选择后复制到便携数据目录并写入画布持久化数据。
- 画布再次同步时从持久化路径加载缩略图；旧画布数据缺少 `image_path` 时保持兼容。
- 回调、持久化、缩放和渲染结构由自动化测试覆盖；离屏截图覆盖三个目标视觉状态。

## 迭代记录

1. 第一轮发现信息字段标签与数值没有对齐、关闭图标继承了主题强调色，已调整为固定行布局和中性关闭色。
2. 第二轮发现弹窗遮罩后仍能看到画布底部工具栏，已在弹窗打开时隐藏缩放条和工具栏。
3. 本轮第一遍离屏截图发现文字节点靠近底部时，固定工具栏会覆盖节点下缘；这是测试摆位导致的非产品布局问题，已上移测试节点并重新截图。
4. 本轮第二遍对比确认 AI 优化与生图左右分布、7 项工具栏、移动画布默认状态和质量选择层级均符合基准意图。
5. 最终对比未发现 P0、P1、P2 级问题；来源截图与实现截图的桌面密度差异，以及复用既有图标造成的细微造型差异归为 P3。

## 工作台参数下拉与输入区增高

- 视觉基准：`C:\Users\deyx1\AppData\Local\Temp\codex-clipboard-7612a0d3-0591-4054-a241-8f53ac40e5c1.png`
- 实现截图：`C:\Users\deyx1\AppData\Local\Temp\artforge-selector-implementation.png`
- 同屏对比：`C:\Users\deyx1\AppData\Local\Temp\artforge-selector-comparison.png`
- 比例、清晰度、张数已收敛为三个等宽横排按钮，各自点击后打开独立下拉弹窗。
- 比例保留 13 个既有选项；清晰度继续遵守 1K、2K、4K 会员权限；动作序列保留第三个张数按钮但禁用。
- 提示词输入容器在常规窗口增加 90px；最小窗口和参考图状态使用自适应高度上限，不与生成按钮重叠。
- 视觉检查覆盖 2164 × 1397 窗口，按钮间距、文本省略、主题色和输入区纵向节奏均符合现有组件体系。
- 编译和针对性回归测试通过，未发现 P0、P1、P2、P3 级视觉问题。

final result: passed
