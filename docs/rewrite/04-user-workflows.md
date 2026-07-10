# ArtAIT Rust 重构资料：用户工作流

结论：Rust 重构的验收标准应以工作流是否完整为准，而不是按钮是否存在。下面的流程可直接转成端到端测试或手工验收脚本。新版本是单入口，工作流入口由 `[features].enabled` 决定是否展示。

## 工作流 0：首次启动引导

目标：新用户从零到完成基础配置，能立刻开始生成。

步骤：

1. 用户启动 `ArtAITRust.exe`。
2. 系统检测 `%APPDATA%\ArtAIT\app_config.toml` 不存在，进入引导。
3. 第 1 步：用户选择功能预设（通用美术 / 动画短片 / 全功能 / 自定义）。
4. 第 2 步：用户确认输入 / 输出 / 提示词目录（默认 `<我的文档>\ArtAIT`，旧用户可一键复用旧目录）。
5. 第 3 步：用户选择主题（深色 / 浅色 / 跟随系统）和字体。
6. 第 4 步：用户配置首个 provider 协议族（OpenAI 兼容等），填端点和密钥，连接测试通过；可跳过。
7. 系统写入 `app_config.toml`，API Key 直接保存在本机配置文件。
8. 进入主界面，按 enabled 渲染页签，默认打开第一个启用的页签。

验收点：

- `app_config.toml` 不存在时强制进入引导。
- 任一步退出后下次启动从草稿恢复。
- 跳过 provider 后顶部模型选择显示"未配置"，点击给出提示。
- 旧目录检测能识别当前工作目录里的 `out/`、`input/` 等。
- 完成后自动切换到主路由。

Rust 实现入口：

- `pages/onboarding.slint`
- `services::onboarding`
- `services::config_store::initialize`
- `services::secret_store::write`

## 工作流 1：从文本生成单张场景图

目标：用户输入场景描述，生成一张场景图并在图库中复用。

前置：`scene` 功能已启用，至少配置一个生图 provider。

步骤：

1. 用户进入"创建场景"页签。
2. 用户选择一个场景提示词模板，或直接输入场景描述。
3. 用户选择输出比例和分辨率。
4. 用户确认顶部生图模型。
5. 用户点击生成。
6. 系统校验提示词、provider 实例和输出目录。
7. 系统创建后台生图任务。
8. 页面显示生成中状态。
9. provider 返回结果后，系统保存图片到 `out/scenes`。
10. 图库刷新并选中新图。
11. 用户可右键打开文件、打开目录、添加到参考图、去黑、去背景或删除。

验收点：

- 没有提示词时阻止提交。
- provider 配置缺失时显示可理解错误。
- 成功后文件真实存在。
- 图库不用重启即可看到新结果。

Rust 实现入口：

- callback `create_generation_task`
- `services::generation`
- `artait-task::TaskRunner`
- `services::asset_library::refresh`

## 工作流 2：角色图迭代生成

目标：用户生成一个角色，并把结果作为下一轮参考图继续优化。

前置：`character` 启用，已配生图与推理 provider。

步骤：

1. 用户进入"创建角色"页签。
2. 用户输入角色设定。
3. 用户上传一张或多张风格参考图。
4. 用户可选点击"分析图片"生成风格提示词。
5. 系统调用推理 provider 分析参考图。
6. 用户编辑最终提示词。
7. 用户点击生成角色。
8. 系统调用生图 provider。
9. 输出保存到 `out/creations`（或动画入口配置下的 `out/animation_characters`）。
10. 用户在图库中右键选择"添加到参考图"。
11. 页面将新生成图片加入当前参考图列表。
12. 用户继续调整提示词并生成下一轮。

验收点：

- 多参考图顺序与移除可控。
- 分析任务和生成任务互不阻塞界面。
- 生成结果可成为后续任务输入。

Rust 实现入口：

- `services::reference_images`
- `services::prompt_analysis`
- `services::generation`

## 工作流 3：角色动作序列批处理

目标：用户用一张角色图生成多个动作图，并保留每个动作的提示词。

前置：`action_sequence` 启用，`reference_action/`、`reference_prompt/` 存在。

步骤：

1. 用户进入"动作序列"页签。
2. 用户选择角色图。
3. 系统扫描 `reference_action` 和动作配置，展示可用动作。
4. 用户勾选动作。
5. 用户选择是否强制覆盖提示词、是否强制覆盖图片、是否实际生成图片、是否锁定风格。
6. 用户点击开始生成。
7. 系统上传角色图和所有动作参考图。
8. 系统抽取角色外观 profile。
9. 系统保存 `appearance.json`、`appearance.txt` 和特效适配提示词。
10. 系统并行构造各动作提示词。
11. 若未开启图像生成，流程结束。
12. 若开启图像生成，系统逐个动作调用 provider。
13. 已存在且未强制覆盖的输出被跳过。
14. 用户可点击取消，中止剩余动作。
15. 系统保留已完成动作输出。

验收点：

- 没有角色图时无法开始。
- 没有动作定义时给出明确提示。
- 取消后不继续提交新 provider 请求。
- 提示词和图片分别保存到正确目录。
- 锁定风格影响提示词生成，而不是只影响界面状态。

Rust 实现入口：

- callback `start_action_batch`
- `services::action_discovery`
- `services::appearance_profile`
- `services::prompt_builder`
- `artait-task::TaskRunner`

## 工作流 4：生成动画脚本并拆分分镜包

目标：用户把故事想法和素材转成动画脚本，再拆分为分镜板可消费的包。

前置：`animation_script` 启用，已配推理 provider。

步骤：

1. 用户进入"动画脚本"页签。
2. 用户输入主题或故事想法。
3. 用户上传 `.txt` 或 `.md` 文档。
4. 用户上传参考图。
5. 用户点击生成动画脚本。
6. 系统读取文档文本并编码图片。
7. 系统调用推理 provider 生成 Markdown 脚本。
8. 脚本保存到 `out/animation_scripts`。
9. 文件列表刷新并选中新脚本。
10. 系统解析脚本中的镜头编号或表格。
11. 系统拆分分镜包并保存到 `_packages/<脚本名>`。
12. 右侧分镜包页签显示每个包的镜头范围和预览。

验收点：

- 没有主题、文档、图片时阻止生成。
- `.pdf`、`.docx` 不假装已解析，提示用户转为文本。
- 脚本预览支持 Markdown 基础元素。
- 分镜包可手动重新拆分。

Rust 实现入口：

- callback `generate_animation_script`
- `services::document_reader`
- `services::script_generation`
- `services::storyboard_package`

## 工作流 5：从动画脚本发送到分镜板

目标：用户把脚本或分镜包快速转成分镜图生成输入。

前置：`animation_script` 与 `storyboard` 都启用。

步骤：

1. 用户在动画脚本页选中脚本全文或分镜包。
2. 用户点击"发送到分镜板"。
3. 系统定位分镜板页签。
4. 系统把当前文本填入分镜板提示词输入区。
5. 系统切换到分镜板页。
6. 系统可选设置默认比例为分镜常用比例。
7. 用户点击生成。
8. 结果保存到 `out/storyboards`。

验收点：

- 分镜板页懒加载时也能接收文本。
- 发送后不丢失原脚本文件选择。
- 分镜板复用通用创作工作台能力。

Rust 实现入口：

- callback `send_script_to_storyboard`
- `services::router::activate_tab`
- `CreationMode::Storyboard`

## 工作流 6：高级提示词优化

目标：保存提示词模板时用 Prompt Optimizer Studio 执行多轮评分优化。

前置：Prompt Optimizer 启用且 sidecar 健康。

步骤：

1. 用户打开提示词创建 / 编辑弹窗。
2. 用户输入名称和提示词内容。
3. 用户勾选高级优化。
4. 系统检查 Prompt Optimizer Studio 健康状态。
5. 服务不可用时显示启动提示并停止保存。
6. 服务可用时创建优化 job。
7. 系统轮询 job 状态。
8. 页面显示轮次、评分和优化状态。
9. 优化完成显示最终提示词。
10. 用户确认后保存模板文件。

验收点：

- 服务不可用时不吞错误。
- 轮询超时有明确状态。
- 需要人工确认的 job 能提示用户去服务端继续。
- 不泄露 API Key。

Rust 实现入口：

- `artait-providers::prompt_optimizer::Client`
- `services::prompt_optimization`
- `pages/settings.slint` 中的优化监控

## 工作流 7：配置 provider 实例

目标：用户新增或修改 provider 实例，并让它出现在顶部模型选择中。

步骤：

1. 用户进入"设置"页签的"生图"或"推理"或"生视频"分组。
2. 用户新增 provider 实例。
3. 用户选择协议族（OpenAI 兼容、Gemini 兼容、Wavespeed 兼容、自定义……）。
4. 用户选择适用范围（生图 / 推理 / 视频）。
5. 用户填写 API 端点、模型列表、密钥等配置（按 JSON schema 渲染）。
6. 用户点击连接测试。
7. 系统调用协议族的连接测试能力。
8. 保存设置；API Key 直接写入本机 `app_config.toml`。
9. 顶部模型下拉按实例分组刷新。
10. 用户选择实例下的模型。

验收点：

- 协议族能力不足时不可选对应范围。
- 密钥不明文展示或写日志。
- 模型选择持久化到对应实例，不全局覆盖所有 provider。
- 连接失败时给出可操作错误。

Rust 实现入口：

- callback `create_provider_instance`、`test_provider_connection`、`save_settings`
- `services::config_store`
- `services::secret_store`
- `artait-provider::ProviderRegistry::instantiate_from_family`

## 工作流 8：外部图片导入

目标：用户从资源管理器或二次启动传入图片，应用把图片送到合适页面。

步骤：

1. 应用已运行。
2. 用户再次启动应用并携带图片路径，或外部调用传入路径。
3. 单实例通道接收路径。
4. 系统校验图片扩展名和文件存在。
5. 当前页是创作页 → 图片加入参考图。
6. 当前页是动作序列页 → 第一张图片填入角色图。
7. 当前页不支持图片输入 → 切到场景页（若启用）并加入参考图；场景页未启用则切到第一个启用的支持参考图的页面。
8. 应用窗口被唤起到前台。

验收点：

- 非图片路径被忽略。
- 多张图片不会覆盖已有参考图。
- 应用窗口被唤起。
- 当前禁用的目标页面有 fallback 路径。

Rust 实现入口：

- `services::single_instance`
- callback `import_external_files`
- `services::router::dispatch_import_intent`

## 工作流 9：切换主题

目标：用户在运行时切换主题，立即看到效果。

步骤：

1. 用户点击顶部主题按钮，弹出主题菜单（深色 / 浅色 / 跟随系统 / 用户自定义）。
2. 用户选择目标主题。
3. 系统加载对应主题文件。
4. Rust 端写入 Slint `Theme global`。
5. 全 UI 立即重绘，无重启、无闪烁。
6. 系统持久化 `app_config.toml.ui.theme`。

跟随系统场景：

7. 系统读取 `AppsUseLightTheme` 注册表，决定走 dark/light。
8. 监听 `WM_SETTINGCHANGE`，系统切换深浅色时跟随。

用户自定义场景：

9. 用户在设置页颜色选择器调整字段，实时预览写入 Theme global。
10. 点击"保存"写回 `%APPDATA%\ArtAIT\themes\user.toml`。
11. 也可手动编辑 `user.toml`，`notify` watch 命中后即时重载。

验收点：

- 切换主题响应时间 ≤ 100 ms。
- 跟随系统能正确响应深浅色变化。
- 用户主题文件写错时 fallback 到 dark 不崩。

Rust 实现入口：

- callback `apply_theme`、`update_theme_field`、`save_user_theme`
- `services::theme_manager`
- `services::sys_theme_listener`

## 工作流 10：调整功能开关

目标：用户在不重启的情况下启用或禁用功能模块。

步骤：

1. 用户进入"设置"页签的"功能开关"分组。
2. 用户切换预设或勾选具体模块。
3. 系统更新 `app_config.toml.features`。
4. UI 路由按新 enabled 列表重新渲染页签。
5. 已打开但被禁用的页面被关闭，状态保留在内存。
6. 新启用的页面在被点击时懒加载。

验收点：

- 切换不需要重启。
- 关闭页面不丢失尚未保存的内容（先弹确认）。
- 启用模块后页签出现在正确位置（按声明顺序）。

Rust 实现入口：

- callback `set_feature_enabled`、`apply_feature_preset`
- `services::feature_flags`

## 工作流 11：迁移旧用户数据

目标：旧 Python 版本用户的配置和素材在新版本中可继续使用。

步骤：

1. 用户在新版本第一次启动并进入引导第 2 步。
2. 系统检测当前目录或常见旧目录里是否存在 `config.json`、`out/`、`prompt/`、`apply_prompt/`、`reference_action/`。
3. 检测到 → 显示"使用旧目录"横幅。
4. 用户点击 → 字段自动填为旧路径。
5. 用户进入"关于"页或主菜单的"迁移工具"。
6. 系统输出迁移报告（dry-run）：发现的模板、资产、provider 实例数量。
7. 用户确认导入旧 provider 结构（不含密钥）。
8. 用户在向导中重新输入或确认导入旧密钥到 Credential Manager。
9. 完成后旧资产可在新图库中浏览。

验收点：

- 旧输出在新图库可见。
- 旧模板在新页面可选择。
- 不自动泄露旧密钥。
- 迁移失败有可读报告。

Rust 实现入口：

- `services::legacy_migration`
- `services::config_store::import_legacy_json`
- `pages/settings.slint` 中的迁移工具入口

## 验收策略

- 每条工作流转成端到端 smoke 测试，最少覆盖一次成功路径。
- 关键工作流（1、3、4、9）额外覆盖取消、失败、跳过路径。
- 每个 PR 必须保证关联工作流的 smoke 仍通过。
