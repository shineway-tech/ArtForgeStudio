# ArtForgeStudio 安装包功能拆解

分析对象：`D:\ArtForgeStudio\ArtForgeStudio.exe`  
分析日期：2026-06-20  
分析方式：静态字符串提取与归类，未执行安装包。  

## 结论摘要

当前安装包不是源码包，属于已经编译后的 Windows 桌面客户端。可见技术栈高度指向 Rust/Tauri/Wry，本地侧包含配置、任务、素材库、项目、剧本解析、角色库、场景库、分镜、视频生成等模块。

从功能上看，它已经不是单一“生图工具”，而是一套本地创作工作台：

- 游戏美术：场景、角色、UI、特效、动画素材、批量生图、角色三视图、提示词模板与优化。
- 影视创作：项目、剧本生成/导入/解析、角色草稿、场景校准、分镜包、镜头图生成、Seedance 视频生成。
- 基础系统：Provider 配置、模型列表、任务面板、历史记录、素材图库、运行日志、主题/目录设置、Prompt Optimizer sidecar。

需要注意：当前安装包更像“本地桌面版创作工具”，没有明显看到用户账号、积分、购买、流量计费、云端会员体系等 SaaS 商业闭环功能。

## 证据来源

本次生成了可复查的中间文件：

- `analysis/exe/summary.json`：字符串提取统计。
- `analysis/exe/strings_business_low_noise.txt`：过滤后的业务相关字符串。
- `analysis/exe/tokens_business.json`：业务 token 统计。
- `tools/extract_exe_features.py`：提取脚本。

关键证据包括：

- 模块路径：`crates\artait-app`、`crates\artait-service`、`crates\artait-provider`、`crates\artait-providers`、`crates\artait-asset`、`crates\artait-config`、`crates\artait-task`。
- UI 路由/页面：`welcome`、`settings`、`tasks`、`runtime_log`、`project_overview`、`project_script`、`project_characters`、`project_scenes`、`project_storyboard`、`project_video`、`asset_browser`、`character_library`、`scene_library`、`scripts`、`storyboards`、`frames`、`videos`。
- 本地文件：`app_config.toml`、`task_history.json`、`project.toml`、`character_library.json`、`scene_library.json`、`uploaded_images.json`、`prompt-optimization.toml`、`prompt-optimizer.db`。

## 整体信息架构

### 一级模块

1. 欢迎/初次设置
2. 美术创作工作台
3. 影视创作工作台
4. 素材图库
5. 角色库
6. 场景库
7. 剧本库
8. 分镜板
9. 视频生成
10. 任务面板
11. 设置
12. 运行日志

### 当前客户端定位

客户端主文案中出现：

- “美术创作 / 影视创作”
- “从提示词、参考图到素材图库的一体化工作台”
- “从剧本到角色、场景、分镜和视频的统一生产面板”
- “至少配一个 provider 才能开始生成”
- “本地内存 provider，不发任何网络请求，1 秒内可用”

因此当前版本支持先用 Mock Provider 跑通流程，再在设置里接真实服务。

## 欢迎与初次设置

### 已确认功能

- 首次打开会询问“你想用 ArtForge Studio 做什么？”
- 可选择创作能力：
  - 场景 / 角色 / 特效 / 动作序列
  - 通用美术
  - 脚本 / 分镜 / 角色三视图
  - 动画短片
  - 启用全部模块
  - 手动选择需要的模块
- 引导配置 AI 服务：
  - 至少配置一个 Provider 才能开始生成。
  - 可以跳过，后续到设置页再配置。
  - 支持 Mock 实例，用于无网络本地试流程。
  - 推荐 OpenAI 兼容 Provider。
- 可配置：
  - API 端点
  - 生图模型
  - 推理模型
  - 自动验证连通性

### 与目标网站的关系

这部分可迁移为网页版欢迎页和首次 onboarding，但当前安装包没有看到注册、登录、每日赠送积分、客户端下载 CTA、购买套餐等商业入口。

## 美术创作工作台

### 核心描述

客户端文案显示，美术创作是“从提示词、参考图到素材图库的一体化工作台”。用户先选择创作类型，再生成、预览和处理结果图。

### 创作类型

已确认内置类型包括：

| 类型标识 | 中文含义 | 说明 |
|---|---|---|
| `scene_concept` | 场景概念图 | 环境、背景、场景概念图 |
| `tileset` / `tile_set` | TileSet / 地编参考 | 模块化、可复用地编素材 |
| `level_design_reference` | 关卡设计参考 | 清晰空间布局、可导航场景 |
| `promo_art` | 宣传图 | 营销级 key art |
| `loading_art` | Loading 图 | 载入页插图 |
| `mini_map` | 小地图 | 俯视简化地标 |
| `building_kit` | 建筑套件 | 可复用建筑部件 |
| `character_portrait` | 角色立绘 | 全身角色设计 |
| `character_turnaround` | 三视图 | 正面、侧面、背面 |
| `eight_direction` | 8 方向 | 多方向角色视图 |
| `sprite_sheet` | SpriteSheet | 动画帧表 |
| `spine_parts` | Spine 拆件 | 动画拆件素材 |
| `npc_avatar` | NPC 头像 | 对话头像 |
| `character_poster` | 角色海报 | 角色 key art |
| `skill_effect` | 技能特效 | 技能释放效果 |
| `buff_effect` | Buff 特效 | 状态光环 |
| `explosion` | 爆炸 | 冲击、粒子层 |
| `scene_effect` | 场景特效 | 环境气氛特效 |
| `ui_effect` | UI 特效 | 界面反馈光效 |
| `weapon_trail` | 武器拖尾 | 攻击轨迹 |
| `hud` | HUD | 游戏 HUD 概念 |
| `main_menu` | 主界面 | 游戏主菜单 |
| `inventory` | 背包 | 道具格、分类 |
| `shop` | 商城 | 商品卡、价格层级 |
| `icon` | 图标 | 游戏 icon |
| `loading_ui` | Loading 界面 | 带进度区域 |
| `dialog` | 弹窗 | 对话/确认弹窗 |

### 输入与控制项

已确认字段：

- `prompt`
- `negative_prompt`
- `aspect_ratio`
- `resolution`
- `quality`
- `count`
- `reference_images`
- `image_upload_api_url`
- `image_upload_api_key`
- `mode`
- `model`
- `provider_id`

UI 文案确认支持：

- 拖入角色、场景或风格参考图。
- 选择目录提示词或手动输入提示词。
- 每行一个提示词批量生成。
- 输出保存到 `out/batch/`。
- 生成结果进入素材图库。
- 查看提示词历史。

### 角色三视图

已确认功能：

- “基于参考图生成角色三视图，保持角色设计一致，输出正面、侧面、背面完整设定图。”
- 状态文案包括：
  - 已提交三视图生成任务
  - 三视图生成完成
  - 三视图生成失败

## 影视创作工作台

### 项目化流程

影视创作是项目制，页面结构包括：

1. `project_overview`：项目概览
2. `project_script`：剧本
3. `project_characters`：角色
4. `project_scenes`：场景
5. `project_storyboard`：分镜
6. `project_video`：视频

项目文案：

- “项目将资产组织到独立目录中，方便管理和导出。”
- “项目名称，例如：我的动画短片。”
- “描述（可选）：简短描述项目内容。”
- 本地项目文件：`project.toml`。
- 项目目录下有 `storyboards/`、`videos/` 等输出目录。

### 剧本工作台

已确认功能：

- 输入创作主题生成标准剧本。
- 导入现有剧本。
- 最近生成和导入的剧本会自动保存到本地。
- 重启后默认打开最新剧本。
- 可打开目录、删除、解析、保存、发送到分镜板。
- 有“格式示例”和“加参考”功能。

剧本推荐格式：

- 剧名
- 大纲
- 人物小传
- 第 X 集
- `1-1 日/夜 内/外 地点`
- 人物
- 动作
- 角色对白

剧本处理流水线：

1. 规范化剧本。
2. 统计集、场、角色和对白。
3. 解析结构。
4. 补全环境、光影、色彩、道具，写入场景库。
5. 从对白和出场人物生成角色库草稿。
6. 按镜头标题拆分分镜包，发送到分镜板。

相关内部对象：

- `ScriptSceneSummary`，6 个字段。
- `ScriptCharacterSummary`，5 个字段。
- 脚本索引表 `scripts`，包含：
  - `episode_count`
  - `scene_count`
  - `character_count`
  - `dialogue_count`
  - `structure_json`
  - `parse_status`

### 角色库

已确认角色字段包括：

- `gender`
- `personality`
- `role`
- `traits`
- `skills`
- `key_actions`
- `appearance`
- `relationships`
- `identity_anchors`
- `negative_prompt`
- `views`
- `variations`
- `reference_images`
- `hair`
- `skin`
- `lips`
- `view_type`
- `video`

UI 字段：

- 角色名，例如：林月
- 性别：女 / 男 / 未知
- 年龄：20 岁 / 少年 / 中年
- 身份：主角、剑客、机械师
- 中文提示词：用于中文模型或人工检查
- 英文提示词：用于图像生成，留空则回退中文
- 高级一致性锚点
- 返回生成封面
- 可用生图模型生成角色封面

本地文件：

- `character_library.json`

操作：

- 新建角色
- 创建角色
- 删除角色
- 加载角色库
- 解析失败时重建空库

### 场景库

已确认场景字段包括：

- `time_of_day`
- `atmosphere`
- `visual_prompt_zh`
- `visual_prompt_en`
- `architecture_style`
- `lighting_design`
- `color_palette`
- `key_props`
- `spatial_layout`
- `era_details`
- `contact_sheet_image`
- `viewpoint`
- `tags`
- `notes`
- `thumbnail_url`
- `style_id`
- `folder_id`
- `project_id`
- `status`
- `linked_episode_id`
- `episode_numbers`
- `appearance_count`
- `importance`
- `created_at`
- `updated_at`
- `parent_id`
- `is_auto_created`
- `draft`
- `linked`

本地文件：

- `scene_library.json`

场景校准提示词说明：

- 角色：影视前期美术指导。
- 目标：把剧本中抽取出的场景草稿校准为可用于场景库和生图的一致视觉设定。
- 约束：不新增、删除或重排场景，只按输入 index 返回增强结果。
- 输出：地点、时间、氛围、构图层次、光影、材质、关键道具、中文视觉提示词、可选英文提示词。

### 分镜板

已确认功能：

- 从剧本页拆分并发送分镜包后，进入制作队列。
- 用户逐包检查镜头正文、补充画面要求，再生成分镜图。
- 镜头图生成完成后会自动替换占位图。
- 分镜图保存到当前项目的 `storyboards/` 目录。
- 支持生成选中镜头图。

分镜字段：

- `shot_ids`
- `grid_index`
- `image_url`
- `generated_at`
- `secondary`
- `transition`
- `name_en`

UI 提示建议用户补充：

- 景别
- 机位
- 运动
- 构图
- 光线
- 角色状态
- 角色/场景/风格参考图
- 画幅比例
- 生图模型

### 视频生成

已确认功能：

- 通过 Seedance 视频 API 生成短视频。
- 支持 T2V、I2V、多模态引用。
- 可逐镜头调用视频 API。
- 保存到项目 `videos/` 目录或全局 `out/videos/`。
- 可选择静音音频。
- 有视频任务队列。

相关状态/错误：

- 请先输入视频提示词。
- 未设置默认视频 provider。
- 已提交视频生成任务。
- 视频生成完成。
- 调用视频 provider。
- 已保存视频。
- 不支持视频生成。
- 视频引用超过限制。
- 提交 Seedance 视频任务。
- 视频任务已提交。
- 未找到视频 URL。
- 缺少 Seedance 参数。
- 写入视频元数据失败。
- 打开视频元数据索引失败。

## Provider 与模型系统

### Provider 类型

已确认出现：

- `openai-compatible`
- `openai`
- `gemini`
- `deepseek`
- `volcengine`
- `volcengine-seedance`
- `memefast`
- `toapis`
- `sub2api`
- `newapi`
- `cpa`
- `mock`

UI 文案显示支持：

- OpenAI 兼容
- Gemini
- Wavespeed 等兼容端点
- DeepSeek
- Seedance

### Provider 能力类型

内部枚举/字段显示：

- `generation`
- `analysis`
- `both`
- `generation_model`
- `analysis_model`
- `video_model`
- `generation_model_options`
- `analysis_model_options`
- `video_model_options`
- `api_style`
- `base_url`
- `api_key`
- `secret_ref`

支持接口风格：

- `chat`
- `responses`
- `messages`
- `images`
- `openai_images`
- `openai_images_edits`
- `gemini`
- `toapis`
- `embeddings`
- `rerank`

### 模型与尺寸

安装包中可见的模型/族：

- `gpt-image-2`
- `dall-e-2`
- `dall-e-3`
- `gemini-3.1-flash-image`
- `gemini-3-pro-image`
- `gemini-2.5-flash-image`
- `nano-banana`
- `nano-banana-pro`
- `imagen`
- `flux`
- `sdxl`
- `stable`
- `midjourney`
- `seedream`
- `kling`
- `seedance`
- `deepseek-chat`

可见清晰度/质量：

- `low`
- `high`
- `standard`
- `hd`
- `480p`
- `720p`
- `1080p`
- `2K`
- `4K`

可见比例/尺寸：

- `1:1`
- `2:3`
- `3:2`
- `3:4`
- `4:3`
- `4:5`
- `5:4`
- `9:16`
- `16:9`
- `21:9`
- `1024x1024`
- `1536x864`
- `864x1536`
- `1536x1024`
- `1024x1536`
- `2048x2048`
- `2048x1152`
- `1152x2048`
- `2880x2880`
- `3840x2160`
- `2160x3840`

### 凭据管理

可见文案：

- API Key 保存到本机配置文件。
- 系统凭据 API Key 已同步到配置文件。
- provider missing secret_ref。
- provider secret missing in credential manager。
- provider secret read failed。

推断：当前版本至少支持配置文件保存 API Key，也可能接入了系统凭据管理器。

## 提示词模板与优化

### 模板系统

已确认模板类型：

- `scene_prompt`
- `storyboard_prompt`
- `effect_prompt`
- `character_turnaround_prompt`
- `create_character_prompt`
- `ui_prompt`
- `prompt-optimization.toml`

模板变量：

- `{image_instruction}`
- `{has_images}`
- `{page}`
- `{preset_prompt}`
- `{user_prompt}`
- `{director_controls}`
- `{final_prompt_preview}`

功能：

- 自定义提示词模板。
- 目录提示词。
- 不使用目录提示词。
- 提示词历史。
- 提示词名称不能为空。
- 提示词内容不能为空。
- 创建 `prompts/` 目录。
- 可上传风格参考图并分析图片生成风格提示词。

### Prompt Optimizer

已确认包含一个 sidecar：

- `prompt-optimizer-server.exe`
- `Prompt Optimizer sidecar`
- `PROMPT_OPTIMIZER_SERVER_ADDR`
- `PROMPT_OPTIMIZER_DB_PATH`
- `prompt-optimizer.db`

优化逻辑文案：

- 角色：专业 AI 生图提示词优化师。
- 输入：预设提示词、用户输入提示词、游戏开发导演控制。
- 输出：严格 JSON。
- 字段：
  - `optimized_prompt`
  - `summary`
  - `changes`
- 要求：
  - 优先使用英文美术描述。
  - 中文总结改写点，最多 80 字。
  - 不输出 CFG、Steps、Sampler、Scheduler、Clip Skip、Denoise 等底层采样参数。
  - 不输出私密思维链。

### 参考图分析

已确认能力：

- 用参考图片反推场景美术风格和特征。
- 用参考图片反推特效美术风格。
- 用参考图片反推人物特征。
- 用参考图片反推界面美术风格。
- 单轮分析提示词优化。
- 参考图分析完成后提示词填入模板。

## 素材图库与本地存储

### 素材库

已确认组件：

- `AssetLibrary`
- `asset_browser`
- `artait_asset`
- `thumbnail`
- 素材目录实时监听

素材字段：

- `asset_id`
- `width`
- `height`
- `created_at`
- `modified_at`
- `source_task_id`
- `batch_id`
- `deleted`

生成元数据：

- `prompt`
- `negative_prompt`
- `mode`
- `quality`
- `aspect_ratio`
- `count`
- `image_index`
- `reference_images_json`
- `provider_metadata_json`
- `request_metadata_json`
- `provider_id`
- `model`

支持：

- 缩略图生成。
- 缩略图失败时使用原图。
- 分类为空时提示“去创作页生成素材，或刷新输出目录”。

### 上传缓存

已确认：

- `uploaded_images.json`
- `image_urls`
- `video_urls`
- `mtime`
- `cache_key`
- 大参考图会自动上传。
- 超过 10MB 的参考图可自动上传 ImgBB 获取公网 URL，需要 API Key。
- 大参考图命中上传缓存。

## 任务与历史

### 任务状态

已确认状态：

- `running`
- `validating`
- `uploading`
- `submitted`
- `polling`
- `saving`
- `completed`
- `failed`
- `cancelled`
- `waiting`

UI 状态：

- 保存中
- 轮询中
- 上传中
- 准备中
- 已取消
- 失败
- 完成
- 运行中

### 任务记录

本地文件：

- `task_history.json`

任务历史字段：

- `progress`
- `last_log`
- `finished_at`
- `output_path`
- `provider_instance_id`
- `provider_id`
- `model`
- `provider_task_id`
- `endpoint`
- `extra_json`
- `retry_source_url`
- `project`

功能：

- 任务面板。
- 可查看所有任务记录。
- 清空失败任务。
- 清空完成任务。
- 取消生成。
- 重试失败任务。
- 任务记录不存在时创建。
- 历史解析失败时重建空历史 JSON。

## 设置与运行日志

### 设置页

可配置：

- 工作目录
- 输出目录
- 输入素材目录
- 提示词模板目录
- 图床设置
- 界面外观
- 主题
- 字体
- Provider
- 模型列表
- 默认供应商
- Prompt Optimizer 路径/端口
- 日志开关
- 调试日志开关

配置文件：

- `app_config.toml`

配置字段：

- `schema_version`
- `features`
- `providers`
- `provider_defaults`
- `remove_background`
- `image_upload`
- `runtime`
- `last_main_tab`
- `migrated_from`
- `last_workspace`
- `prompt_history`
- `last_project`
- `log_enabled`
- `debug_log_enabled`
- `prompt_optimizer_path`
- `prompt_optimizer_port`
- `prompt_optimizer_idle_timeout_secs`

### 运行日志

UI 包含：

- 运行日志
- 刷新运行日志
- 清空运行日志

## 当前安装包没有明显覆盖的目标网站能力

你最初描述的目标网站包含完整 SaaS 商业闭环。基于当前安装包静态分析，以下能力没有明显看到：

- 用户注册 / 登录 / 找回密码。
- 每日赠送积分。
- 积分余额、积分明细。
- 购买套餐。
- 按流量计费。
- 官方统一模型池。
- 云端生成历史同步。
- Web 版入口。
- 客户端下载页。
- 管理后台。
- 支付订单。
- 发票/订阅/退款。
- 团队空间或多人协作。
- 内容安全审核。
- 生成结果云端 CDN。

这说明当前客户端可以作为“创作能力原型/桌面端 MVP”的基础，但要变成你设想的 ArtForgeStudio 网站，还需要新增账号、计费、模型网关、云端任务、历史同步和运营后台。

## 可复用到新版网站的功能资产

建议优先复用/照搬的产品设计：

1. 美术创作类型 taxonomy：已经覆盖场景、角色、UI、特效、动画素材。
2. Provider 抽象：生成、推理、视频三类能力拆分合理。
3. Prompt 模板系统：适合迁移为官方模板库。
4. Prompt Optimizer：可以作为“智能优化”卖点。
5. 影视项目流水线：剧本 -> 角色 -> 场景 -> 分镜 -> 视频。
6. 角色库/场景库结构：可作为数据库表设计基础。
7. 任务历史字段：可迁移为云端任务记录。
8. 参考图上传缓存：可迁移为云对象存储资源表。

## 建议的新网站功能拆分

### 第一阶段：前端 MVP

- 欢迎页
- 注册/登录占位
- 每日积分说明
- Web 工作台
- 游戏美术 Tab
- 影视创作 Tab
- 假数据生成历史
- 项目创建与影视流水线 UI
- 模型/比例/清晰度/张数选择

### 第二阶段：真实生成闭环

- 用户系统
- 积分账户
- 官方 Provider 网关
- 任务队列
- 生成历史
- 资源上传
- 对象存储
- 结果预览/下载

### 第三阶段：商业化

- 套餐
- 充值
- 订单
- 用量账单
- 模型价格策略
- 管理后台
- 内容审核

## 可信度说明

高可信：

- 页面结构、主要模块、创作类型、任务状态、本地文件名、Provider 类型、剧本/角色/场景/分镜/视频流程。

中可信：

- 部分字段之间的精确数据关系。
- 部分模型列表和尺寸是否全部暴露给 UI。
- 凭据是否同时写入配置文件和系统凭据管理器。

低可信/待源码确认：

- 完整数据库 schema。
- 每个 Provider 的完整请求参数。
- 所有 UI 交互细节。
- 生成失败重试策略。
- 视频元数据索引的具体结构。

