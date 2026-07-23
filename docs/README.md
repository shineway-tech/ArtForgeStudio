# Documentation

这里是 ArtForge Studio 客户端仓库的当前文档入口。文档面向客户端开发和发布维护，以 `native-client` 源码、`scripts/` 和 `.github/workflows/` 为事实来源；文档与代码不一致时，先以代码为准，再同步修正文档。

| Document | Purpose |
|---|---|
| [PRODUCT.md](PRODUCT.md) | Product scope and client/server authority |
| [ARCHITECTURE.md](ARCHITECTURE.md) | Active native-client architecture and data flow |
| [DEVELOPMENT.md](DEVELOPMENT.md) | Local development, tests, and troubleshooting |
| [RELEASE.md](RELEASE.md) | Packaging, CI, signing, and release verification |
| [MIGRATION.md](MIGRATION.md) | Migration from provider-direct client versions |

## Reading order

- 新维护者：`PRODUCT.md` → `ARCHITECTURE.md` → `DEVELOPMENT.md`。
- 发布人员：`RELEASE.md`，需要排查构建时再读 `DEVELOPMENT.md`。
- 迁移支持：`MIGRATION.md` → `PRODUCT.md`。

## Documentation rules

- 当前客户端只维护上表中的专题文档，根 `README.md` 只提供快速入口。
- 已完成的实施计划、按日期记录的状态快照和临时 TODO 不作为当前事实保留。
- 后端数据库表、服务端内部实现和部署细节属于服务端仓库。
- 模型、会员、积分包、赠送额度、折扣和操作价格是动态商业配置，不在客户端文档中固化。
- 旧架构需要追溯时使用 Git 历史，不在仓库中建立文档归档副本。
