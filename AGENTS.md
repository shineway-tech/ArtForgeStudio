# ArtForge Studio — Agent 指南

## 项目定位

ArtForge Studio 是桌面端 AI 美术生产套件。`native-client` 是唯一活动客户端，
也是 workspace 的唯一成员，输出 `ArtForgeStudio.exe`。

`crates/` 是早期模块化迁移源码，已排除在 workspace 之外，不参与构建、测试
或发布。除非任务明确要求恢复迁移，不要修改或重新接入这些归档 crate。

## 架构

```
crates/
├── artait-model/         数据类型，零 IO、零异步
├── artait-config/         TOML 配置 + secret_store（keyring）
├── artait-provider/       Provider trait + Registry + HTTP 抽象
├── artait-providers/      协议族实现（Mock / OpenAI兼容 / Volcengine Seedance）
├── artait-task/           TaskRunner + 取消 + 事件总线
├── artait-asset/          资产懒索引 + 缩略图 + 后处理
├── artait-service/        业务编排（生成 / 脚本 / 提示词优化 / 任务历史）
└── artait-app/            归档 UI 源码（无应用二进制目标）

native-client/             当前产品客户端（ArtForgeStudio）
```

**依赖方向（禁止反向）**：
```
app → service → task → provider → config → model
                  ↘ asset ↗
```

- `model` 零依赖；`provider` 不感知 `service`/`task`；`app` 唯一引入 slint。
- `artait-service` 是业务编排层，`app` 的 callback handler 只做参数转发。
- `artait-providers` 内每个协议族是独立模块，通过 `ProviderRegistry` 注册。

## 构建

```sh
cargo build --release -p artforge-studio-native --bin ArtForgeStudio
cargo check -p artforge-studio-native
cargo test -p artforge-studio-native
cargo clippy -p artforge-studio-native
```

`build.bat` 封装了以上常用命令（菜单式选择），只处理 `ArtForgeStudio`。

## 技术栈

| 项 | 值 |
|----|-----|
| Rust edition | 2021，MSRV 1.78 |
| UI | Slint 1.8（自建组件，不引入 std-widgets） |
| 异步 | tokio 1 |
| HTTP | reqwest 0.12 + rustls |
| 序列化 | serde 1 + serde_json 1 + toml 0.8 |
| 错误 | thiserror（库）+ anyhow（应用） |
| 日志 | tracing 0.1 + tracing-subscriber 0.3 + tracing-appender 0.2 |

## Slint 代码组织

```
ui/
├── main.slint               AppShell（路由、模态框）
├── app-state.slint           AppState global（所有双向绑定属性+回调）
├── theme.slint               Theme global（颜色/圆角/字体绑定）
├── components/               自建轻量组件（11 个）
│   ├── sidebar.slint         侧边栏（导航+功能开关）
│   ├── top-bar.slint         顶栏（页面标题+模型选择）
│   ├── status-bar.slint      底栏（状态文本+进度）
│   ├── button.slint / input.slint / card.slint ...
│   ├── asset-grid.slint      资产网格（缩略图+右键菜单）
│   └── image-preview.slint   大图预览
├── pages/                    页面（全部真实实现，无占位）
│   ├── workspace.slint       通用创作工作台（7 种 mode 复用）
│   ├── asset-browser.slint   图库浏览
│   ├── tasks.slint           任务面板
│   ├── settings.slint        设置（外观/Provider/目录/关于）
│   ├── storyboard.slint      分镜板
│   ├── script.slint          动画脚本
│   ├── action-sequence.slint 动作序列批处理
│   ├── runtime-log.slint     运行日志
│   ├── welcome.slint         欢迎页
│   ├── onboarding.slint      首启引导（3 步）
│   └── onboarding-step{1,2}.slint + onboarding-step4.slint
└── assets/icons/             27 个 SVG 图标（24×24，单色）
```

### Slint ↔ Rust 桥接规则

1. 全局状态用 `AppState` global，Rust 通过 `app.global::<AppState>()` 读写
2. 用户事件通过 `Callback<Args, Ret>`，Rust 在 `main.rs` / `callbacks/` 注册 handler
3. 后台任务事件通过 `invoke_from_event_loop` 回到 UI 线程
4. handler 不直接写业务逻辑，转发到 `artait-service`
5. 长列表用 `for` + `VecModel`，增量更新

## Provider 架构

```
Provider trait（arait-provider/src/lib.rs）
  ├── ImageGenerator      → generate()
  ├── CharacterGenerator   → generate_character()
  ├── Analyzer            → analyze()
  ├── VideoGenerator      → generate_video()   [trait 就位，无实现]
  └── Pollable            → poll()

ProviderRegistry（HashMap<provider_id, Arc<dyn Provider>>）
  └── build_registry() in artait-app/src/providers.rs
```

### 已有协议族

| 协议族 | 模块 | 能力 |
|--------|------|------|
| Mock | `mock.rs` | 生成+分析（返回固定结果） |
| OpenAI 兼容 | `openai_compatible/` | 生成+分析+媒体上传，内建 Gemini API 支持 |
| Volcengine Seedance | `volcengine/` | 图片生成（异步提交+轮询），HMAC-SHA256 V4 签名 |

### 添加新协议族

1. 在 `crates/artait-providers/src/` 下新建模块目录
2. 实现 `Provider` trait + 至少一个能力子 trait
3. 在 `artait-providers/src/lib.rs` 注册 `pub mod` + `pub use`
4. 在 `artait-providers/Cargo.toml` 添加需要的依赖
5. 在 `artait-app/src/providers.rs::build_registry()` 调用 `reg.register()`
6. 如需快速添加模板，在 `settings.slint` 加按钮 + `main.rs` 加 `quick-add-provider` 分支

## UI 图标

- 统一使用 SVG，放在 `ui/assets/icons/`，Slint 中通过 `@image-url(...)` 引用
- 单色功能图标用 `Image.colorize` 接入 `Theme.palette`，确保深浅主题一致
- 优先复用已有 SVG；确需新增时保持 24×24 viewBox 和单色路径
- 禁止 emoji 或纯文字作为功能图标

## 主题

- 9 套预设 TOML（`themes/`） + 用户自定义（`user.toml`，notify watch 即时生效）
- Slint 端通过 `Theme` global 单例绑定颜色/圆角/字体/间距
- 主题切换：Rust 加载 TOML → 写入 `Theme` global → 全 UI 立即重绘
- 系统深浅色跟随：读 Windows `AppsUseLightTheme` 注册表

## 首启引导

3 步流程（`ui/pages/onboarding.slint`）：
1. 功能预设（通用美术 / 动画短片 / 全功能 / 自定义）
2. 工作目录 + 界面外观（4 张主题卡）
3. Provider 配置（可跳过）

引导写完后生成 `app_config.toml`，下次启动直接进主界面。

## 配置与密钥

- 配置格式 TOML，绿色版存放（`portable_data_dir()`）
- 密钥通过 `secret_ref` 引用，可存系统凭据管理器（keyring）或 TOML 内 `api_key`
- Provider 密钥在日志/错误中始终脱敏
- `normalize_provider_secrets()` 启动时同步 keyring → 内存

## 关键约定

- **先功能闭环，再视觉细节。**
- **模块间用 trait 解耦，不直接依赖具体实现。**
- **Slint callback handler 不放业务逻辑，转发到 `artait-service`。**
- **新增页面在 `main.slint` 的 `AppShell` 中注册路由。**
- **Provider 实例由用户通过设置页配置，协议族编译期注册。**
- **文件路径使用 `portable_data_dir()` 保证绿色版可移动。**
- **错误信息不泄露 API Key。**

## 重构原则

- **渐进下沉，不一次全搬。** 每次修一个高频模块，顺手把业务逻辑沉到 `artait-service`。
- **不从 UI 拆起，从数据源拆起。** settings service 和 asset metadata service 收益最大、风险最低。
- **回调要轻。** UI callback 只做"取输入 → 调 service → 展示结果"，重的回调容易出现 RefCell 借用冲突、状态串场。
- **统一状态来源。** 图库元数据、任务历史、运行中任务各自明确主数据源（如 metadata store、task_history.json），UI 只是显示缓存。
- **watcher / runtime / event loop 边界要清。** 文件监听、TaskRunner、Slint 事件循环应封装为 service 负责启动/停止/刷新/错误返回。
- **补充关键路径测试。** 配置保存、元数据读写、下载保存、任务历史——这些已经暴露过问题，service 层拆出来后天然更好测。
- **保持文档与代码一致。** 架构文档里的模块如果不存在或已变化，及时同步。
