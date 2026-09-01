# Design QA — 自由画布工具与导航

## Comparison target

- 缩放控制参考：`C:/Users/deyx1/AppData/Local/Temp/codex-clipboard-fe282f74-888a-45fd-acfe-01b6b12032e2.png`（432 × 105）。
- 竖向工具栏参考：`C:/Users/deyx1/AppData/Local/Temp/codex-clipboard-39f6064c-c544-4ab9-9cd4-55f83b54d3b6.png`（72 × 350）。
- 侧栏参考：`C:/Users/deyx1/AppData/Local/Temp/codex-clipboard-4a6f3702-c9f3-4f6c-b048-ff4d9ecacb75.png`（327 × 1398）。
- 实现截图：`E:/Elunvi Canvas/artifacts/canvas-toolbar-layout/implementation-final.png`（1443 × 931，显示密度 1）。
- 同屏对照：`comparison-zoom.png`、`comparison-toolbar.png`、`comparison-sidebar.png`，均位于 `E:/Elunvi Canvas/artifacts/canvas-toolbar-layout/`。
- 验收状态：从“自由画布”进入“植物生成器”独立画板，画板已加载预置参考图与底部输入区。

## Findings

- 未发现 P0、P1 或 P2 级视觉问题。
- [P3] 参考图的竖向工具栏是深色底，当前实现沿用产品既有的浅色工具栏和紫色选中态；这是有意保持当前设计系统、图标资源与主题适配，而非布局偏差。

## Required fidelity surfaces

- 布局：应用侧栏已从画布页面移除；画布占满主体区域。缩放控制固定在画布右上角，竖向工具栏固定在画布左侧并垂直居中，无重叠、截断或溢出。
- 字体与图标：保留现有字体层级和仓库图标资源，没有引入字符占位、手绘 SVG 或临时图标。
- 间距与尺寸：右上控制条保持 16 px 边距；左侧工具栏为 44 × 310 px 竖向胶囊，与参考图的紧凑比例一致。
- 颜色：沿用应用主题色、面板色、边框色和选中态，保证与画布底色的对比度。
- 交互：实际验证了普通滚轮纵向移动画布、重置视图、模型下拉框、尺寸/清晰度下拉框和左侧图片工具弹层。空格加左键拖动画布、Ctrl 加滚轮缩放由运行时自动化测试覆盖。
- 内容：预置入口继续使用对应的预置图片和内置提示词，不影响独立画板状态。

## Comparison history

- 初次同屏对照：右上缩放条的位置与结构匹配参考；左侧工具栏方向、位置和密度匹配参考；画布页面没有应用侧栏。
- 桌面交互检查：普通滚轮产生纵向位移；重置视图恢复默认位置；模型和尺寸/清晰度菜单可打开并选择。
- 最终结论：无需追加视觉修复。

final result: passed
