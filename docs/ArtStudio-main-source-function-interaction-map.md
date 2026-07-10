# ArtStudio-main 源码功能与交互梳理

源码路径：`D:\ArtForgeStudio\ArtStudio-main`  
当前形态：Windows 桌面应用，Rust + Slint 单体客户端，不是网站/SaaS 后端。

## 1. 本地 Git 状态

- 已在 `D:\ArtForgeStudio\ArtStudio-main` 初始化本地 Git 仓库。
- 已提交初始源码快照：`2dd73ba Initial source snapshot`。
- 这个快照可作为后续修改前的回滚基线。

## 2. 总体架构

项目是 Rust workspace，核心 crate 分工如下：

- `artait-model`：纯数据模型。包含配置、功能开关、Provider、任务、资产、项目、角色、场景、剧本、Seedance 视频参数等。
- `artait-config`：读取/写入本机 `app_config.toml`，处理损坏配置备份、旧配置迁移、目录创建。
- `artait-provider`：Provider trait 抽象，定义图片生成、角色生成、文本分析、视频生成、异步轮询能力。
- `artait-providers`：内置协议族实现：Mock、OpenAI 兼容、火山 Seedance 图像、MemeFast Seedance 视频。
- `artait-task`：异步任务运行器、取消令牌、任务事件、结果保存。
- `artait-asset`：扫描输出目录、缩略图、后处理、生成资产 SQLite 元数据索引。
- `artait-service`：业务层。生成、项目、剧本、角色库、场景库、任务历史、提示词模板、sidecar 等逻辑。
- `artait-app`：Slint UI、AppState 全局状态、页面路由、回调绑定、启动入口。

依赖方向大体是：`app -> service -> task/provider/config/model`，UI 通过 `AppState` callback 进入 Rust 回调。

## 3. 本地数据与落盘

- 便携数据根目录：默认在主程序同级 `data/`，也可用环境变量 `ARTAIT_DATA_DIR` 覆盖。
- 配置：`data/config/app_config.toml`。
- 输出：`data/out/`，按功能分子目录，如 `scenes`、`creations`、`effects`、`storyboards`、`videos` 等。
- 项目：默认 `data/out/projects/<项目名>/project.toml`，项目内会创建 `scripts`、`characters`、`storyboards`、`frames`、`videos`、`scenes`。
- 角色库：`data/characters/character_library.json`。
- 场景库：`data/scenes/scene_library.json`。
- 任务历史：`data/tasks/task_history.json`，只保存已完成/失败/取消任务。
- 资产索引：`data/index/asset_index.sqlite`，保存生成图片/视频的 prompt、provider、model、比例、质量等元数据。
- 剧本索引：`data/index/script_index.sqlite`，缓存 Markdown 剧本列表和解析结果。

## 4. 启动流程

`artait-app/src/main.rs` 启动后会：

1. 初始化日志，日志写到 `data/logs/ArtForgeStudio.log`。
2. 加载 `app_config.toml`，如果不存在，当前代码是使用默认配置并进入主界面。
3. 注册内置 Provider：Mock、OpenAI 兼容、火山 Seedance、MemeFast Seedance。
4. 创建 `TaskRunner`，并发数 4。
5. 加载角色库、场景库、任务历史。
6. 初始化主题和字体。
7. 初始化 AppState：当前页面、功能列表、默认项目路径、provider 信息等。
8. 注册所有 UI callback。
9. 启动资产扫描监听、主题监听、任务事件桥接。

注意：文档里仍写着“首次启动进入 4 步引导”，但当前 `main.rs` 实际是配置缺失时直接进入主界面并保存默认配置。

## 5. Provider 与模型能力

当前内置 Provider：

- Mock Provider：测试用，支持图片生成和文本分析。
- OpenAI 兼容：支持 `/chat/completions`、`/responses`、`/messages`、Gemini `generateContent` 等文本分析；支持 OpenAI/Gemini 风格图片生成和图片编辑。
- 火山 Seedance：支持 `seedancetoimage_v2` 图像生成，异步提交和轮询。
- MemeFast Seedance：支持 Seedance 视频生成，含 T2V/I2V/多模态引用参数结构。

Provider 实例由用户在设置页添加，保存在 `app_config.toml`。默认生图、推理、视频 Provider 分别由 `provider_defaults.generation/analysis/video` 决定。

## 6. 任务与历史

生成类按钮不会阻塞 UI，而是提交到 `TaskRunner`。

任务状态：`validating/uploading/submitted/polling/saving/completed/cancelled/failed`。  
任务事件通过 bridge 推回 UI，刷新任务面板、请求抽屉、图库计数和状态栏。

历史记录包括：任务 ID、类型、状态、provider、model、prompt、输出路径、错误、provider_task_id、retry_source_url 等。支持：

- 取消运行中任务。
- 清除已完成/失败/全部历史。
- 删除单条历史。
- 对支持 `poll_task` 的 Provider 重新获取异步任务结果。
- 对下载失败但有 URL 的任务重试保存。

## 7. 主界面和模式

AppState 有 `workspace-mode`：

- `art`：美术创作模式。
- `film`：影视创作模式。

无项目时，影视模式侧边栏主要显示项目入口；打开项目后，侧边栏切为项目内流程：

`概览 -> 剧本 -> 分镜 -> 角色 -> 场景 -> 视频生成`

欢迎页提供快捷入口、最近资产缩略图和项目入口。

## 8. 游戏/美术工作台

共用页面：`WorkspacePage`，覆盖以下模式：

- 场景：`scene`
- 角色：`character`
- UI 概念：`ui_concept`
- 特效：`effect`
- 动画场景：`animation_scene`
- 动画角色：`animation_character`
- 角色三视图：`character_turnaround`

共用输入能力：

- 文本 prompt。
- 参考图：文件选择、拖拽、剪贴板导入、从图库加入。
- 目录提示词模板。
- prompt 预览。
- 文本优化/图文优化。
- 参考图分析生成提示词。
- 比例：`1:1`、`16:9`、`9:16`。
- 清晰度：`1K`、`2K`、`4K`。
- 张数：1、2、4，代码层限制为 1 到 4。
- 导演控制项：用途、色彩氛围、游戏视角、天气、时间、光照。

生成流程：

1. UI 调 `AppState.generate-image(mode, prompt, aspect, quality, count)`。
2. Rust 读取手写 prompt、目录模板、导演控制项并拼成最终 prompt。
3. 找默认生图 Provider。
4. 每张图提交一个 `TaskKind::Image` 任务。
5. Provider 返回 URL 或 bytes。
6. `ResultSaver` 保存到输出目录。
7. 写入 SQLite 资产元数据。
8. 刷新图库和任务历史。

图库能力：

- 按领域过滤：全部、场景、角色、UI、特效、分镜。
- 预览图片、查看元数据、打开文件、定位文件、复制路径、删除。
- 加入参考图。
- 去黑、去背景、高级去黑等后处理任务。

## 9. 动作序列

`ActionSequencePage` 使用同一个生图回调，但 mode 是 `action_sequence`。

特点：

- prompt 按行拆分，每一行生成一个 batch 子任务。
- 支持 `skip_existing`，如果输出目录已有对应 `batch-N` 文件则跳过。
- 输出到 `out/batch`。

## 10. 视频工作台

视频页调用 `AppState.generate-video(prompt, aspect, resolution, duration, enable_audio)`。

支持参数：

- prompt。
- 比例。
- 分辨率：480p、720p、1080p。
- 时长：代码层限制 4 到 15 秒。
- 是否启用音频。

生成流程：

1. 找默认视频 Provider。
2. 构建 `SeedanceVideoParams`。
3. 提交 `TaskKind::Video`。
4. 调 `VideoGenerator`。
5. 保存视频文件。
6. 写入资产元数据。

当前 UI 暂未把多模态首帧/尾帧/音频/参考视频暴露成完整控件，模型层和 MemeFast Provider 已支持这些字段。

## 11. 项目系统

项目是影视创作的顶层容器。

交互：

- 项目页列出项目。
- 创建项目：输入名称、可选路径。
- 打开项目：设置当前项目 ID 和名称。
- 关闭项目：回到全局模式。
- 项目概览页提供流程入口：剧本、角色、场景、分镜、视频。

实现注意点：

- `create-project(name, desc, path)` 里的描述参数当前被忽略。
- 项目列表显示的描述为空，scene-count 也是 0。
- 自定义项目路径创建后写进配置，但项目列表和重新打开逻辑主要扫描默认 `output/projects`，自定义路径项目后续可能不完整。
- 普通美术生图仍主要输出到全局 `out/<mode>`；视频生成和部分分镜逻辑会使用项目目录。

## 12. 剧本工作台

剧本页是当前影视创作里实现最完整的部分。

页面结构：

- 左侧：剧本库，扫描 `out/animation_scripts/*.md`。
- 中间：当前剧本，支持预览、编辑、解析结果。
- 右侧：AI 创作/导入面板和处理流水线。

创建剧本：

1. 用户输入故事主题。
2. 可添加 `.txt`/`.md` 参考文档。
3. 调默认推理 Provider。
4. AI 按标准剧本格式生成 Markdown。
5. 保存到 `out/animation_scripts`。
6. 自动解析并展示集、场、角色、对白统计。

导入剧本：

1. 用户粘贴完整剧本。
2. 系统规范化文本。
3. 如果格式不完整，会包装成可导入 Markdown。
4. 保存为 `imported-script-时间.md`。
5. 后台解析。

支持的剧本格式：

- Markdown：`## 第N集`、`### 场景头`。
- 标准场景头：`1-1 日 内 地点`。
- 出场人物：`出场人物：张三、李四`。
- 动作：`△动作描写`。
- 对白：`角色名：台词` 或 `角色名（动作）：台词`。
- 字幕：`【字幕：内容】`。

处理流水线：

- 结构解析：统计集、场、角色、对白。
- 导出角色：从对白和出场人物提取角色草稿，写入角色库。
- 导出场景：从场景头和动作提取场景草稿，写入场景库。
- AI 场景校准：用推理 Provider 补环境、光影、色彩、道具、空间布局，再写入场景库。
- 拆分分镜包：按“镜头/Shot/场景头”拆包，默认每包 3 个镜头。
- 发送到分镜板。

实现注意点：

- 页面有“剧本语言、提示词语言、场景数量、分镜数量、视频比例”等 UI 参数，但当前 `script-generate` 回调只把主题和参考文档传给后端，未把这些参数拼入生成请求。
- 剧本正文源文件仍是 Markdown，SQLite 只做列表和解析缓存。

## 13. 分镜板

分镜板接收剧本页拆分出的分镜包。

交互：

- 选择分镜包。
- 自动拆出包内镜头。
- 选择单个镜头。
- 编辑包级或镜头级 prompt。
- 添加/移除参考图。
- 选择比例。
- 生成镜头图。

生成流程：

1. 拼接“分镜描述 + 风格/构图要求 + 镜头参数备注”。
2. 使用默认生图 Provider。
3. mode 为 `storyboard`。
4. 输出到 storyboard 目录。
5. UI 记录当前镜头图片，图库也能从输出目录扫描到结果。

## 14. 角色库

角色库保存到 `character_library.json`。

已实现交互：

- 搜索。
- 创建新角色。
- 选择角色进入详情。
- 编辑：名称、性别、年龄、身份、性格、外貌、中英文视觉提示词、标签。
- 编辑 6 层身份锚点：骨相、五官、唯一标记、颜色、皮肤纹理、发型。
- 删除角色。
- “生成角色图”：当前只是把角色视觉 prompt 填入普通角色工作台并跳转到 `character` 页面。

模型层已有更完整结构：

- 角色状态：草稿/已关联。
- 视图：正面、侧面、背面、3/4 侧。
- 变体/衣柜/阶段造型。
- 负面提示词。
- AI 角色校准结果。

实现注意点：

- `artait-service::character_generation` 里有角色专用生成和变体生成服务，但当前 UI 没有直接调用。
- 普通角色生成完成后，目前没有自动把生成图回写为角色视图或缩略图。

## 15. 场景库

场景库保存到 `scene_library.json`。

已实现交互：

- 创建“新场景”。
- 列表展示名称、地点、时间、氛围、视角数。
- 选择场景，只更新 selected id。

模型层已有更完整结构：

- 场景状态：草稿/已关联。
- 重要性：主要/次要/过渡。
- 视觉 prompt、中英文描述、建筑风格、光影、色彩、关键道具、空间布局、时代细节。
- 多视角联合图、视角切分、文件夹。

实现注意点：

- 场景库 UI 目前没有完整详情编辑页。
- AI 场景校准能把剧本中的场景写入场景库，是当前场景库最主要的数据来源之一。

## 16. 提示词模板和优化

提示词模板：

- 按页面/模式分目录。
- 支持创建、编辑、保存、加载。
- 生成时会把目录模板和手写 prompt 拼接。

优化能力：

- 文本优化：用推理 Provider 优化当前 prompt。
- 图文优化：要求有参考图，结合图像分析优化 prompt。
- 深度优化可走 sidecar Prompt Optimizer。
- 参考图分析可把图片描述写入 prompt 或模板。

sidecar 配置保存在 `AppConfig.sidecar`，默认可使用主程序同目录 `prompt-optimizer-server.exe`。

## 17. 设置页

设置页主要功能：

- 基础设置：主题、字体、字号、输入/输出/模板目录。
- Provider 管理：新增/编辑 OpenAI 兼容实例、快速模板、Mock、测试连接、获取模型、设置默认生图/推理/视频、隐藏/删除实例。
- 图片上传配置：图床 API URL/API Key，大图参考图可自动上传。
- 运行日志：查看、过滤、清空、启停日志、启停 debug 日志。
- 侧边栏功能显示隐藏。

## 18. 当前缺失的网站/SaaS能力

以下是你最初设想的网站能力，但当前源码没有：

- 网站前端和 Web 服务端。
- 用户注册/登录/账号系统。
- 每日赠送积分。
- 积分/流量计费。
- 购买套餐/支付。
- 官方统一模型池与服务端密钥托管。
- 云端生成历史。
- 多用户隔离。
- Web 版和客户端下载分发页。
- 后台管理、订单、账单、用量统计。

当前应用是本地桌面端：用户自己配置 Provider 和 API Key，生成历史和资产都存在本机。

## 19. 后续改造建议入口

如果下一步要按 ArtForgeStudio 网站目标改造，建议先定这几件事：

1. 保留现有 Rust/Slint 客户端，还是把核心功能迁移成 Web。
2. 账号/积分/订单/任务/资产历史放哪种后端数据库。
3. Provider Key 是否全部由官方服务端托管，客户端不再保存 Key。
4. 游戏美术和影视创作是否复用当前 prompt/任务/provider 逻辑，还是重做 API 层。
5. 项目、剧本、角色、场景、分镜、视频这些本地 JSON/Markdown/SQLite 数据如何映射到云端表结构。

