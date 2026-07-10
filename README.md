# ArtAIT Rust

桌面端 AI 美术生产套件（Rust + Slint 重构版）。

详见 `docs/rewrite/00-index.md`。

## 构建

```sh
cargo build --release
```

二进制输出：`target/release/ArtAITRust.exe`。

## 项目结构

- `crates/artait-model` — 数据类型，零 IO
- `crates/artait-config` — TOML 配置 + secret_store
- `crates/artait-provider` — Provider trait + Registry
- `crates/artait-providers` — 协议族实现
- `crates/artait-task` — TaskRunner + 事件总线
- `crates/artait-asset` — 资产懒索引
- `crates/artait-service` — 业务编排
- `crates/artait-app` — Slint UI + main，唯一二进制

## 平台

仅 Windows 10 / 11。
