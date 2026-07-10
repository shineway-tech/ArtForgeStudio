# ArtAIT Rust 重构资料：索引

本目录是 ArtAIT 用 Rust + Slint 重构的开发文档。

## 锁定决策（不再变动）

| 决策 | 选项 |
|------|------|
| UI 框架 | **Slint** |
| 后端语言 | **Rust** |
| 二进制名 | `ArtAITRust.exe`（单二进制） |
| 平台 | **仅 Windows**（Win10/11） |
| 入口 | **单入口 + 功能开关**（合并旧版两个 exe） |
| Provider 扩展性 | **配置驱动 + 编译期注册**（协议族 + JSON schema） |
| 数据持久化 | **文件 + 内存懒索引**（暂不上 SQLite） |
| 配置格式 | **TOML** |
| 密钥存储 | **本机 TOML 配置文件**（API Key 可在设置页回显） |
| 主题预设 | **3 套**（深色 / 浅色 / 跟随系统）+ 用户 TOML 自定义 |
| 首启 | **4 步引导**（功能预设 / 目录 / 主题 / provider） |
| 异步 | `tokio` |
| HTTP | `reqwest` + `rustls-tls` |
| 错误 | `thiserror`（库）+ `anyhow`（应用） |
| 日志 | `tracing` + `tracing-appender` |

非功能预算：

- exe 体积 ≤ 12 MB
- 启动到首屏 ≤ 200 ms
- 主题切换 ≤ 100 ms

## 文档清单

| 文件 | 内容 | 主要读者 |
|------|------|---------|
| `00-index.md` | 本文件，决策锁定与导航 | 所有人 |
| `01-product-overview.md` | 产品定位、新旧形态对比、功能域划分 | 项目经理 / 新成员 |
| `02-ui-map.md` | 单入口 UI 地图、功能开关、主题、路由 | 前端 / UX |
| `03-ui-feature-spec.md` | 每个页面的功能规格（输入、控件、交互、输出） | 前端 / QA |
| `04-user-workflows.md` | 11 条端到端工作流，作为验收脚本 | QA / 前后端 |
| `05-data-model.md` | 核心数据模型（AppConfig / Provider / Task / Asset 等） | 后端 |
| `06-provider-contract.md` | Provider trait、能力查询、协议族、错误模型 | provider 作者 |
| `07-rust-architecture.md` | Workspace 骨架、crate 边界、ADR | 架构 / 后端 |
| `08-migration-plan.md` | 13 阶段迁移计划、风险清单、首批任务 | 全员 |
| `09-ui-theming.md` | Slint 主题机制、Theme global、用户自定义 | 前端 |
| `10-onboarding.md` | 4 步引导、草稿恢复、旧目录检测 | 前端 / UX |
| `11-ui-framework-guidelines.md` | 主界面框架、面板职责、弹层层级、组件密度规范 | 前端 / UX |
| `12-v1.5-game-asset-control.md` | V1.5 游戏资产导演级控制产品设计草案 | 产品 / 前端 / 后端 |

## 推荐阅读顺序

**新成员上手**：`00 → 01 → 07 → 08`，再按角色挑细节。

**前端开发**：`00 → 02 → 03 → 09 → 10 → 11`。

**后端开发**：`00 → 05 → 06 → 07`。

**provider 作者**：`00 → 06 → 05`（找 `ProviderInstance` / `ProviderMeta`）。

**QA**：`00 → 04 → 03`。

## Workspace 骨架（首批 8 个 crate）

```text
ArtAITRust/
├── Cargo.toml                     workspace 根
├── crates/
│   ├── artait-model/              数据类型，零 IO
│   ├── artait-config/             AppConfig + secret_store
│   ├── artait-provider/           Provider trait + Registry
│   ├── artait-providers/          协议族实现
│   ├── artait-task/               TaskRunner + 取消 + 事件总线
│   ├── artait-asset/              资产懒索引 + 缩略图
│   ├── artait-service/            业务编排
│   └── artait-app/                slint UI + main，唯一二进制
├── ui/                            .slint 文件
├── schemas/                       provider JSON schema
├── themes/                        预设主题 TOML
├── assets/                        图标、字体
└── docs/rewrite/                  本文档
```

依赖方向（自顶向下，禁止反向）：

```
app → service → task → provider → config → model
                  ↘ asset ↗
```

## 13 阶段路线图

简表，详见 `08-migration-plan.md`：

0. 清点与冻结接口（已完成）
1. Workspace 骨架
2. 模型层 + 配置 / 密钥
3. Provider Registry + mock provider
4. 任务运行时 + 事件总线
5. 首启引导 + 主题系统
6. 单图生成闭环（创建场景）
7. 通用创作页面扩展
8. 图库 + 后处理 + 视频
9. 动作序列批处理
10. 动画脚本 + 分镜包
11. Prompt Optimizer 集成与打包
12. 兼容迁移工具

MVP = 阶段 1–8。第二版本 = 阶段 9–12。

## 重要原则

- **先功能闭环，再视觉细节。**
- **先共享能力，再具体页面。**
- **先 mock provider，再真实 provider。**
- **配置与密钥必须分离。**
- **Provider 配置驱动**：内置协议族，用户加实例不发版。
- **不引入 std-widgets**：自建轻量组件层，主题运行时切换。
- **单二进制**：所有 crate 编译进 `ArtAITRust.exe`，sidecar 仅 Prompt Optimizer。

## 接下来的动作

按 `08-migration-plan.md` 的"首批开发任务建议"开始：

1. 创建 workspace + 8 个 crate 骨架。
2. 配置 release profile 与 Windows 元信息。
3. `artait-model` 写核心类型与单测。
4. `artait-config` 写 TOML 读写、默认值、错误恢复、`secret_store`。
5. `artait-provider` 写 trait、`ProviderMeta`、`ProviderContext`、`Registry`、`HttpClient`。
6. `artait-providers::mock` 实现 mock provider。
7. `artait-app` 弹一个最小 Slint 窗口。
8. 接入 Theme global 原型（dark/light 切换）。
9. `artait-task::TaskRunner` 跑 mock 任务，UI 显示状态。
10. 写迁移报告 CLI（不输出密钥）。
