# ArtAIT Rust 重构资料：页面功能规格

结论：页面规格按"功能定位、用户输入、主要控件、核心交互、状态反馈、输出结果、Slint 映射"记录。新版本是单入口 + 功能开关，每个页面都是可被启用 / 禁用的模块。多数创作页面共享同一套通用工作台规格。

## 通用创作工作台规格

适用页面（每个页面对应一个 `CreationMode`，由 `[features].enabled` 控制是否显示）：

- UI 概念
- 创建场景
- 创建角色
- 特效
- 动画场景
- 动画角色
- 角色三视图
- 分镜板

### 功能定位

让用户把文字描述、提示词模板和参考图组合成一次 AI 生成任务，并在同一页面完成预览、复用和结果管理。

### 用户输入

- 文字提示词。
- 提示词模板：从 `prompt/<domain>` 中选择 `.txt` 或 `.json` 模板。
- 参考图：文件选择、拖拽、外部导入。
- 图像参数：比例、分辨率、模型实例。
- 可选高级动作：提示词分析、普通优化、高级优化、视频生成。

### 主要控件

- 提示词模板选择器
- 提示词编辑器
- 参考图列表 + 上传按钮
- 生成参数控件
- 生成按钮 + 取消按钮
- 当前任务状态区
- 输出图库 + 预览区
- 右键菜单

### 核心交互

1. 用户选择或输入提示词。
2. 用户添加参考图。
3. 系统按当前 provider 实例和模型构造请求。
4. 任务进入后台。
5. 页面显示运行状态、日志或任务卡片。
6. provider 返回结果后，系统保存到输出目录。
7. 图库刷新，用户可继续把结果加入参考图或做二次处理。

### 状态反馈

- `idle` 无任务，允许编辑。
- `validating` 检查输入和配置。
- `uploading` 上传参考图或编码图片。
- `generating` provider 生图中。
- `polling` 异步轮询中。
- `completed` 已保存并加入图库。
- `failed` 显示错误并允许重试。
- `cancelled` 用户或任务取消。

### 输出结果

- 图片文件保存到对应 `out/<subdir>`。
- 任务信息：任务 ID、provider、模型、输出路径、错误信息。
- 可选提示词文件保存到模板或运行目录。

### Slint 映射

- `pages/workspace.slint`（按 `CreationMode` 复用）
- `components/PromptEditor.slint`
- `components/ReferenceList.slint`
- `components/TaskCard.slint`
- callback `create_generation_task`
- `models::GenerationTask`
- `services::asset_library`

## UI 概念页规格

启用 flag：`ui_concept`。

定位：生成游戏 HUD、应用面板、后台工具界面、UI 概念图。

输入：UI 类型和布局描述、UI 风格模板、可选参考图。

交互：

- 用户描述界面主题、信息层级和组件风格。
- 系统作为图像生成任务而非可交互 UI 代码生成任务。
- 输出进入图库供后续筛选。

输出：`out/ui`。

`CreationMode::Ui`。

## 创建场景页规格

启用 flag：`scene`。

定位：环境、背景、场景概念图。

输入：场景描述、场景模板、参考图。

交互：

- 用户选择场景风格或直接输入需求。
- 结果进入场景图库。

输出：`out/scenes`。

`CreationMode::Scene`。

## 创建角色页规格

启用 flag：`character`。

定位：可作为后续动作、特效、动画流程基础素材的角色图。

输入：角色描述、角色模板、多张参考图、输出比例和分辨率。

交互：

- 可从模板开始或直接写设定。
- 可上传风格参考图，调用推理 provider 生成提示词。
- 生成结果可立即加入参考图列表，形成迭代创作。
- 结果可作为动作序列页输入角色图。

输出：`out/creations`。

`CreationMode::Character`。

## 特效页规格

启用 flag：`effect`。

定位：技能特效、附魔效果、精灵图效果、动画帧参考。

输入：特效描述、特效模板、可选参考图、视角/风格/动作帧约束。

交互：

- 从特效模板中选择固定格式。
- 结果可做去黑或去背景，便于合成。
- 可作为动作序列或游戏素材后处理来源。

输出：`out/effects`。

`CreationMode::Effect`，支持 `ImagePostprocessAction::Unmult` 和 `RemoveBackground`。

## 动画场景页规格

启用 flag：`animation_scene`。

定位：为动画短片生成统一风格的场景图。

与通用场景页同构，但默认提示词模板和输出目录面向动画项目。

输出：`out/animation_scenes`。

`CreationMode::AnimationScene`。

## 动画角色页规格

启用 flag：`animation_character`。

定位：为动画短片生成角色设定或角色图。

与创建角色页同构，默认提示词强调动画角色可用性。

输出：`out/animation_characters`。

`CreationMode::AnimationCharacter`。

## 角色三视图页规格

启用 flag：`character_turnaround`。

定位：将角色设定标准化为正面、侧面、背面等可持续使用的角色参考。

输入：角色设定文本、可选角色参考图。

输出：`out/character_turnarounds`。

`CreationMode::CharacterTurnaround`。

## 动作序列页规格

启用 flag：`action_sequence`。

### 功能定位

根据一张角色图和一组动作参考，批量生成该角色的动作图。

### 用户输入

- 角色原图路径。
- 动作选择列表。
- 强制覆盖提示词。
- 强制覆盖图像。
- 是否生成图像。
- 是否锁定风格。

### 主要控件

- 角色图路径输入框 + 选择按钮。
- 动作卡片网格。
- 运行选项复选框。
- provider / 模型信息。
- 执行日志。
- 进度条。
- 开始 / 取消 / 打开输出目录按钮。

### 核心交互

1. 系统扫描动作定义和参考图（来源：`reference_action/`、`reference_prompt/`）。
2. 用户勾选要生成的动作。
3. 系统上传角色图和动作参考图。
4. 系统通过推理 provider 抽取角色外观结构。
5. 系统为每个动作生成提示词。
6. 若开启图像生成，逐个动作调用生图 provider。
7. 提示词和图片分别落盘。

### 状态反馈

- 日志显示当前阶段：准备 / 上传 / 外观分析 / 提示词生成 / 图片生成 / 完成 / 错误。
- 取消后停止后续动作并保留已完成结果。
- 输出已存在且未强制覆盖时显示跳过。

### 输出结果

- `apply_prompt/<角色名>/appearance.json`
- `apply_prompt/<角色名>/appearance.txt`
- `apply_prompt/<角色名>/<动作名>.txt`
- `out/<角色名>/<动作名>_<网格>.jpg`

### Slint 映射

- `pages/action-sequence.slint`
- callback `start_action_batch`
- `models::ActionBatchJob`
- `models::ActionDefinition`
- `models::AppearanceProfile`
- `services::action_batch`
- `services::appearance_profile`
- `services::prompt_builder`

## 动作预览 / 图库页规格

启用 flag：`asset_browser`。

### 功能定位

浏览本地输出图像和视频，并执行复用、打开和后处理动作。

### 用户输入

- 输出子目录选择。
- 选中的图片或视频。
- 右键菜单操作。

### 核心交互

- 刷新目录（手动 + `notify` 自动）。
- 选择图片显示大图。
- 选择视频时调用 MPV 播放。
- 右键菜单：打开位置、添加到参考图、去黑、去背景、删除。

### 输出结果

- 不直接生成新 AI 内容。
- 可能产生后处理覆盖文件或删除本地文件。

### Slint 映射

- `pages/asset-browser.slint`
- `components/AssetGrid.slint`
- callback `postprocess_asset`、`open_asset`、`delete_asset`、`add_to_reference`
- `services::asset_library`
- `services::image_postprocess`
- `services::video_player`

## 动画脚本页规格

启用 flag：`animation_script`。

### 功能定位

把主题、故事想法、文本资料和参考图转成动画脚本 Markdown，并自动拆分分镜包。

### 用户输入

- 主题 / 故事想法。
- `.txt`、`.md` 文档。
- 参考图。

### 主要控件

- 主题输入框。
- 上传文档按钮。
- 上传参考图按钮。
- 文件列表。
- 生成动画脚本按钮。
- 脚本文件列表。
- 脚本全文预览（Markdown）。
- 分镜包预览。
- 打开 / 重命名 / 刷新 / 打开目录按钮。
- 发送到分镜板按钮。

### 核心交互

1. 用户输入主题或上传资料。
2. 系统读取文本资料、编码参考图。
3. 推理 provider 生成 Markdown 脚本。
4. 脚本保存到 `out/animation_scripts`。
5. 系统识别脚本中的镜头编号和表格，拆分为分镜包。
6. 用户可选分镜包预览或发送到分镜板。

### 输出结果

- `out/animation_scripts/<脚本名>.md`
- `out/animation_scripts/_packages/<脚本名>/*.md`

### Slint 映射

- `pages/script.slint`
- `components/MarkdownView.slint`（基于 `pulldown-cmark` + Slint 文本块）
- callback `generate_animation_script`、`split_storyboard_packages`、`send_to_storyboard`
- `models::AnimationScript`
- `models::StoryboardPackage`
- `services::script_generation`
- `services::storyboard_package`

## 分镜板页规格

启用 flag：`storyboard`。

### 功能定位

把动画脚本片段、镜头需求或分镜包转为分镜板视觉图。

### 用户输入

- 镜头需求文本。
- 动画脚本页发送来的分镜包内容。
- 参考图。
- 风格选项。

### 核心交互

- 接收脚本文本后自动切换到分镜板页。
- 将镜头需求填入提示词输入区。
- 默认使用适合分镜的比例。
- 调用通用创作工作台生成分镜图。

### 输出结果

`out/storyboards`。

### Slint 映射

- `pages/storyboard.slint`
- `CreationMode::Storyboard`
- callback `send_script_to_storyboard`

## 提示词创建 / 编辑弹窗规格

通用组件，所有创作页可触发。

### 功能定位

管理提示词模板，并可基于参考图分析生成模板内容。

### 用户输入

- 提示词名称。
- 保存格式：`txt` 或 `json`。
- 参考图。
- 正向提示词。
- 反向提示词。
- 是否启用高级优化。

### 核心交互

- 上传参考图后可调用推理 provider 分析图片风格。
- 保存为 `txt` 或 `ai_prompts.{positive_prompt,negative_prompt}` 的 JSON。
- 启用 Prompt Optimizer Studio 时先调用服务创建优化任务，再回写或提示用户。

### Slint 映射

- `components/PromptTemplateEditor.slint`
- `models::PromptTemplate`
- callback `save_prompt_template`、`analyze_reference_for_prompt`、`optimize_prompt`

## 设置页规格

定位：替代旧版独立设置弹窗，作为主路由的一个页签。

### 分组

- 基础（输入/输出/提示词目录、主题、字体、MPV 路径）
- 功能开关（模块勾选 + 预设切换）
- 生图（provider 实例）
- 生视频
- 推理
- 图床
- 图像处理
- 提示词优化（Prompt Optimizer Studio 状态、地址、间隔、超时）
- 去背景（Rembg、PhotoRoom）
- 关于（版本、日志路径、迁移工具入口）

### 核心交互

- provider 支持多个实例。每实例有名称、协议族、适用范围、能力勾选、专属配置（按 schema 渲染）。
- 设置保存后刷新顶部模型选择。
- 连接测试由协议族实现，UI 不写死逻辑。
- API Key 字段直接绑定本机配置文件，编辑节点时回显。
- 切换功能开关立即生效，不重启。

### Slint 映射

- `pages/settings.slint`
- `components/ProviderInstanceEditor.slint`
- `components/SchemaForm.slint`（按 JSON schema 动态渲染）
- callback `save_settings`、`test_provider_connection`、`set_feature_enabled`、`apply_theme`
- `services::config_store`
- `services::secret_store`

## 首启引导规格

详见 `10-onboarding.md`。

`pages/onboarding.slint` 是独立路由，与主页签互斥；完成后写入 `app_config.toml` 并切换到主路由。

## 主题与字体规格

详见 `09-ui-theming.md`。

`Theme global` 通过 callback `apply_theme` 在运行时切换；`notify` 监听用户主题文件变化。
