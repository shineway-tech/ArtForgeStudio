# ArtForge Studio — 项目现状与后续计划

> 生成日期：2025-07-14  
> 基于 `docs/rewrite/08-migration-plan.md` 的 13 阶段路线图对标

> 归档说明：本文主体记录早期 `crates/` 模块化方案。当前 workspace 只构建
> `native-client/ArtForgeStudio`，归档 crate 不参与构建或发布。

---

## 一、当前版本信息

| 项 | 值 |
|----|-----|
| 产品名 | ArtForge Studio |
| 当前客户端二进制 | `ArtForgeStudio.exe` |
| 活动 workspace 成员 | `native-client` |
| 所有 crate 版本 | `0.1.0` |
| Rust edition | 2021，MSRV 1.78 |
| UI 框架 | Slint 1.8 |
| 平台 | Windows 10/11 |

---

## 二、Crate 清单

| Crate | 版本 | 职责 | 模块数 |
|-------|------|------|--------|
| `artait-model` | 0.1.0 | 核心数据类型，零 IO | 8 |
| `artait-config` | 0.1.0 | TOML 配置 + secret_store + 旧版迁移 | 4 |
| `artait-provider` | 0.1.0 | Provider trait + Registry + Context + HTTP 抽象 | 5 |
| `artait-providers` | 0.1.0 | 协议族实现（Mock + OpenAI 兼容） | 9 |
| `artait-task` | 0.1.0 | TaskRunner + 取消 + 事件总线 | 2 |
| `artait-asset` | 0.1.0 | 资产懒索引 + 缩略图 + 后处理 | 6 |
| `artait-service` | 0.1.0 | 业务编排（生成 / 脚本 / 提示词优化 / 任务历史） | 13 |
| `artait-app` | 0.1.0 | 归档模块化 Slint UI，不参与当前构建 | 13 |

> 所有业务逻辑已沉入 `artait-service`，`main.rs` 仅 768 行做初始化+注册。

---

## 三、各阶段完成度

### 阶段 0：清点与冻结接口 ✅ 完成
- 10 篇 rewrite 文档全部完成
- 栈、入口、主题、Provider 契约均已锁定

### 阶段 1：Workspace 骨架 ✅ 完成
- 7 个 crate 就位，Cargo workspace 正常
- Release profile（LTO + strip + panic=abort）
- `build.bat` 构建脚本（release / check / dev-fast / clippy / run）
- 注：`artait-service` 已建，业务逻辑已从 `app` 迁出

### 阶段 2：模型层与配置/密钥 ✅ 完成
- `artait-model`：AppConfig、ProviderInstance、Task/Event、Asset、Feature、Theme、Prompt、Paths
- `artait-config`：TOML 加载/保存、默认值、错误恢复、`secret_store`（keyring）
- 旧版 `config.json` 兼容解析（`legacy.rs`）
- 单元测试覆盖

### 阶段 3：Provider Registry + Mock + 真实协议族 ✅ 完成
- `Provider` trait + 能力子 trait（ImageGenerator / CharacterGenerator / Analyzer / VideoGenerator / Pollable）
- `ProviderRegistry` + `ProviderContext` + `HttpClient` 抽象
- Mock provider（用于 UI/任务流验证）
- OpenAI 兼容协议族（generate / analyze / media / poll / upload_cache）
- 连接测试 callback

### 阶段 4：任务运行时与事件总线 ✅ 完成
- `TaskRunner`：Semaphore 控并发、broadcast 推事件、CancellationToken 全链路
- `TaskContext`：进度报告、日志、取消检测
- `ResultSaver`：File / Url / Base64 / AsyncTask 四种输出统一保存
- Slint 桥接：`invoke_from_event_loop` 回 UI 线程
- 单元测试覆盖（完成/取消/失败/超时/并发限制）

### 阶段 5：首启引导 + 主题系统 ✅ 完成
- 4 步引导（功能预设 / 目录 / 主题 / Provider）
- 9 套预设主题（dark / light / cream / cyber / forest / ocean / oled / rose / warm）
- 用户自定义主题（`user.toml` + notify watch 即时生效）
- 系统深浅色跟随（Windows 注册表 + `WM_SETTINGCHANGE`）
- 自建组件层 11 个（Button / Input / Card / Sidebar / TopBar / StatusBar …）

### 阶段 6：单图生成闭环 ✅ 完成
- `workspace.slint`：7 种 CreationMode 复用同一页面
- 提示词输入 + 反向提示词 + 参考图上传/拖放 + 比例/品质选择
- Provider 实例 + 模型下拉选择
- 生成 → 保存 → 元数据持久化 → Gallery 回显全链路
- 错误/取消/重试/re-acquire（重新轮询）

### 阶段 7：通用创作页面扩展 ✅ 完成
- 7 种模式：场景 / 角色 / UI 概念 / 特效 / 动画场景 / 动画角色 / 角色三视图
- 提示词模板创建/编辑/加载
- 参考图分析生成提示词（Analyzer）
- 普通提示词优化（文本级，通过 Analyzer 重写）
- 功能开关动态显隐页签

### 阶段 8：图库 + 后处理 + 视频

| 子项 | 状态 |
|------|------|
| 图库浏览（Tab 分类 + 缩略图网格 + 大图预览 + 右键菜单） | ✅ |
| 懒扫描 + notify 增量刷新 | ✅ |
| 缩略图缓存（`data/cache/thumbnails/`） | ✅ |
| 元数据读写（SQLite） | ✅ |
| 去黑（unmult） | ✅ |
| 去背景（Rembg / PhotoRoom HTTP 服务） | ✅ |
| 高级去黑（perfect_unmult） | ✅ |
| 视频生成 | 🔜 后续 |
| 视频播放（目前走外部播放器 `open_with_default`） | ⚠️ 仅外部打开 |

### 阶段 9：动作序列批处理 ✅ 完成
- `ActionSequencePage`：角色图上传、动作卡片网格、运行选项
- 批量任务：跳过已存在 / 强制覆盖 / 取消 / 并发限制
- 外观分析 + 动作提示词构建
- 上传策略（base64 / multipart）

### 阶段 10：动画脚本与分镜包 ✅ 完成
- `AnimationScriptPage`：主题输入、文档上传、参考图
- 脚本生成（Analyzer 调用）→ Markdown 落盘
- 脚本文件列表、打开、重命名、刷新
- `StoryboardPage`：分镜包解析（按 `## 镜头 N` 分组）、预览
- 发送到分镜板 → 逐镜头生图

### 阶段 11：Prompt Optimizer 集成 🟡 简单 API 接口

- 当前状态：已有基于 Analyzer 的**文本级提示词优化**（`artait-service/src/prompt_template.rs`）
- Prompt Optimizer 本质是一个标准 API 调用（非 sidecar），通过 Analyzer trait 即可接入
- 缺失内容：
  - 专用的 Prompt Optimizer 提示词模板调优
  - 设置页服务状态显示
- 预计工作量：小（复用已有 `run_analysis` 基础设施）

### 阶段 12：兼容迁移工具 ✅ 完成
- `artait-migrate` 二进制已就位
- 旧配置结构映射、资产索引注入、迁移报告

---

## 四、后续工作优先级

### P0 — 代码质量

**1. 提取 `artait-service` crate** ✅ 已完成

> `main.rs` 4542 行 → 768 行 (-83%)。业务逻辑全部沉入 `artait-service` (13 模块，145 测试)。

已迁入 `artait-service` 的模块：
```
artait-service/ (13 modules)
├── assets.rs           ✅ read_asset_metadata
├── generation.rs       ✅ run_image_generation + 8 tests
├── onboarding.rs       ✅
├── page_routing.rs     ✅ is_workspace_page / initial_page_from_config + 6 tests
├── prompt_template.rs  ✅
├── provider.rs         ✅ create_provider / edit_provider / validate_endpoint
├── provider_helpers.rs ✅ run_analysis / run_connection_test + 16 tests
├── script.rs           ✅ generate_script_via_provider
├── settings.rs         ✅ apply_basic_settings / SettingsSaveOutcome + 5 tests
├── task_filter.rs      ✅ clear_task_label / task_matches_clear_filter + 4 tests
├── task_history.rs     ✅
└── utils.rs            ✅ short / short_safe / mime_for_path / is_image_path + 4 tests
```

全部迁移完毕，`main.rs` 仅做初始化 + callback 注册。

---

### P1 — 功能补全

**2. Prompt Optimizer（阶段 11）**

- 新建 `artait-providers::prompt_optimizer` 模块
- Sidecar 进程管理（启动/健康检查/重连）
- Job 生命周期（创建→轮询→取消→人工接管）
- 设置页状态面板

**3. 更多 Provider 协议族**

`openai_compatible` 模块是一个智能协议路由器——根据端点 URL 和 `api_style` 设置自动选择正确的 API 格式。
以下协议族全部通过同一个 `OpenAiCompatibleProvider` 实例 + 不同配置覆盖，无需独立模块：

| 协议族 | 状态 | 接入方式 |
|--------|------|---------|
| OpenAI 兼容 | ✅ | 原生 `/v1/chat/completions` + `/v1/images/generations` |
| Gemini 兼容 | ✅ | 端点自动检测 `v1beta`，切换到 `:generateContent` + `x-goog-api-key` 鉴权 |
| Anthropic 兼容 | ✅ | 自动路由到 `/v1/messages`（Messages API） |
| DeepSeek | ✅ | OpenAI 兼容端点，填 URL + Key 即可 |
| Wavespeed / TokenHub | ✅ | 多前缀 fallback：`/v1` → `/openai/v1` → `/api/v1` |
| 中转代理 (newapi/cpa/sub2api) | ✅ | `api_style` 选择对应参数格式 |
| Mock | ✅ | 测试用，返回固定结果 |
| Volcengine Seedance | ✅ | 独立模块，HMAC-SHA256 V4 签名 + 异步轮询 |

---

### P2 — 后续功能

**4. 视频生成**

- 数据模型 + `VideoGenerator` trait 已就位
- 需要：Provider 实现 + UI 视频创作页 + 内嵌播放

**5. 持续打磨**

- 性能：验证 exe ≤ 12 MB、启动 ≤ 200 ms
- 测试：补充集成测试、provider 契约测试（wiremock）
- 体积：`cargo bloat` 检查

---

## 五、依赖版本速查

| 依赖 | 版本 |
|------|------|
| slint | 1.8 |
| tokio | 1 |
| reqwest | 0.12 |
| serde / serde_json | 1 |
| toml | 0.8 |
| tracing | 0.1 / 0.3 / 0.2 |
| rusqlite | 0.32 |
| image | 0.25 |
| notify | 6 |
| chrono | 0.4 |
| uuid | 1 |
| async-trait | 0.1 |
| base64 | 0.22 |
| pulldown-cmark | 0.12 |
| keyring | 3 |
| rfd | 0.15 |
| directories | 5 |
| winreg | 0.52 |
| futures | 0.3 |
