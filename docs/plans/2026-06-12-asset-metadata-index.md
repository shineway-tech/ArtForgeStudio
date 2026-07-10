# 绿色版数据目录与文件元数据 Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** 把 ArtAIT 默认改成绿色版数据布局：程序同级 `data/` 下保存配置、图片、提示词、日志、任务历史、缩略图和图片元数据，同时设置页仍允许用户自定义路径。

**Architecture:** 新增统一的 portable data root，默认值为 `<exe_dir>/data`。`PathConfig::default()`、配置文件路径、主题文件、任务历史、缩略图缓存、运行日志和资产索引都从这个根目录派生。图片元数据使用中心化 SQLite 索引，不采用“一张图一个 JSON”；默认图片保存在 `data/out/`，索引保存在 `data/index/asset_index.sqlite`。

**Tech Stack:** Rust 2021, Slint, `artait-model::PathConfig`, `artait-config`, `artait-asset::AssetLibrary`, `artait-task::ResultSaver`, `rusqlite`, `serde/serde_json`, `chrono`.

---

## 推荐结论

默认数据根目录：

```text
<程序所在目录>\data
```

推荐目录结构：

```text
data\
  config\
    app_config.toml
  input\
  out\
    scenes\
    creations\
    ui\
    effects\
    animation_scenes\
    animation_characters\
    character_turnarounds\
    batch\
    storyboards\
  prompt\
  apply_prompt\
  reference_action\
  reference_prompt\
  themes\
    user.toml
  index\
    asset_index.sqlite
  cache\
    thumbnails\
  logs\
    ArtAITRust.log
  tasks\
    task_history.json
```

默认行为：
- 图片默认保存到 `data/out/`。
- 配置默认保存到 `data/config/app_config.toml`。
- 元数据索引默认保存到 `data/index/asset_index.sqlite`。
- 设置页仍可把输入、输出、提示词目录改到任意外部路径。
- 如果用户改了外部输出目录，图片跟随用户设置；元数据索引仍在 `data/index/`，以保持程序数据集中。

不推荐：
- AppData / Documents 作为默认路径：不符合绿色版预期，迁移和打包不直观。
- 每张图一个 JSON：目录污染明显，后期搜索和批量维护也差。
- 单个大 JSON：并发保存和异常退出时更脆弱，图库大了之后查询慢。
- 图片内嵌 metadata 作为主存储：跨格式不稳定，二次压缩或编辑容易丢。

## 核心路径规则

新增统一函数，建议放在 `artait-model`，因为 `artait-config`、`artait-asset`、`artait-app` 都能依赖它：

```rust
pub fn portable_data_dir() -> PathBuf {
    if let Some(raw) = std::env::var_os("ARTAIT_DATA_DIR") {
        return PathBuf::from(raw);
    }
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|dir| dir.join("data")))
        .or_else(|| std::env::current_dir().ok().map(|dir| dir.join("data")))
        .unwrap_or_else(|| PathBuf::from("data"))
}
```

说明：
- 正常用户走 `<exe_dir>/data`。
- `ARTAIT_DATA_DIR` 只给测试、开发调试和打包脚本用。
- 不再用 `%APPDATA%`、`%LOCALAPPDATA%`、`Documents\ArtAIT` 作为新默认值。

## 默认 PathConfig

`PathConfig::default()` 改为：

```rust
let base = portable_data_dir();
Self {
    input_dir: base.join("input"),
    output_dir: base.join("out"),
    prompt_dir: base.join("prompt"),
    apply_prompt_dir: base.join("apply_prompt"),
    reference_action_dir: base.join("reference_action"),
    reference_prompt_dir: base.join("reference_prompt"),
}
```

设置页保存路径时仍写入 `app_config.toml`。如果用户自定义了路径，就使用用户路径，不强制放回 `data`。

## 配置加载策略

新路径：

```text
data\config\app_config.toml
```

兼容策略：
1. 优先读取 `data/config/app_config.toml`。
2. 如果不存在，再探测旧路径 `%APPDATA%\ArtAIT\app_config.toml`。
3. 如果旧配置存在，复制到新路径。
4. 对旧配置中的路径做保守迁移：
   - 如果路径等于旧默认 `Documents\ArtAIT\input/out/prompt/...`，改成新默认 `data/input/out/prompt/...`。
   - 如果路径明显是用户自定义外部路径，保留不改。
5. 如果新旧配置都不存在，创建默认配置并进入首启引导。

这能避免用户已有自定义路径被强行搬动。

## 图片元数据索引

主数据库：

```text
data\index\asset_index.sqlite
```

为什么不放 `data/out/.artait/`：
- 现在目标是绿色版统一数据，不只是输出目录自管理。
- 用户可以在设置里把输出目录改到外部磁盘，索引仍应跟程序数据一起备份。
- 默认情况下图片就在 `data/out/`，数据库和图片仍在同一个 `data/` 大目录里。

数据库中同时保存：
- `portable_rel_path`：如果文件在 `data/` 下，保存相对 `data/` 的路径，用于整体移动后恢复。
- `abs_path`：当前绝对路径，用于快速打开。
- 如果用户设置了外部输出路径，`portable_rel_path` 可为空，依赖 `abs_path`。

## 最小数据模型

### Table: `assets`

```sql
CREATE TABLE assets (
  id TEXT PRIMARY KEY,
  portable_rel_path TEXT,
  abs_path TEXT NOT NULL,
  file_name TEXT NOT NULL,
  domain TEXT NOT NULL,
  kind TEXT NOT NULL,
  mime TEXT NOT NULL,
  bytes INTEGER NOT NULL,
  width INTEGER,
  height INTEGER,
  created_at TEXT NOT NULL,
  modified_at TEXT NOT NULL,
  source_task_id TEXT,
  batch_id TEXT,
  deleted INTEGER NOT NULL DEFAULT 0
);
```

### Table: `generation_metadata`

```sql
CREATE TABLE generation_metadata (
  asset_id TEXT PRIMARY KEY,
  prompt TEXT NOT NULL,
  negative_prompt TEXT,
  mode TEXT NOT NULL,
  quality TEXT,
  aspect_ratio TEXT,
  count INTEGER,
  image_index INTEGER,
  provider_instance_id TEXT,
  provider_id TEXT,
  provider_name TEXT,
  model TEXT,
  endpoint TEXT,
  template_file TEXT,
  reference_images_json TEXT NOT NULL DEFAULT '[]',
  provider_metadata_json TEXT NOT NULL DEFAULT '{}',
  request_metadata_json TEXT NOT NULL DEFAULT '{}',
  FOREIGN KEY(asset_id) REFERENCES assets(id)
);
```

### Table: `schema_migrations`

```sql
CREATE TABLE schema_migrations (
  version INTEGER PRIMARY KEY,
  applied_at TEXT NOT NULL
);
```

第一版必须保存：
- 提示词 `prompt`
- 品质 `quality`
- 宽高比 `aspect_ratio`
- 图片宽高 `width/height`

第一版顺手保存：
- 模式 `mode`
- provider/model
- 任务 ID
- 文件路径、大小、mime

## Task 1: 统一绿色版数据根目录

**Files:**
- Modify: `crates/artait-model/src/paths.rs`
- Modify: `crates/artait-model/src/lib.rs`
- Test: `crates/artait-model`

**Step 1: 写测试**

新增测试覆盖：
- `ARTAIT_DATA_DIR` 存在时优先使用它。
- 无环境变量时返回当前 exe 同级 `data`，测试里只断言结尾是 `data`。
- `PathConfig::default()` 的所有默认目录都在 data 下。

Run:

```powershell
cargo test -p artait-model path
```

**Step 2: 实现 `portable_data_dir()`**

按“核心路径规则”实现，并从 `artait_model` re-export。

**Step 3: 修改 `PathConfig::default()`**

把旧的 `Documents\ArtAIT` 默认目录替换为 `portable_data_dir()`。

## Task 2: 配置文件迁到 `data/config`

**Files:**
- Modify: `crates/artait-config/src/lib.rs`
- Modify: `crates/artait-config/src/legacy.rs` if needed
- Modify: `crates/artait-app/src/migrate.rs`
- Test: `crates/artait-config`

**Step 1: 修改 config dir**

`config_dir()` 返回：

```rust
Ok(artait_model::portable_data_dir().join("config"))
```

`app_config_path()` 保持：

```rust
config_dir()?.join("app_config.toml")
```

**Step 2: 旧配置探测**

新增内部函数：

```rust
fn legacy_appdata_config_path() -> Option<PathBuf>
```

只用于迁移，不作为新默认。

**Step 3: 迁移规则**

当 `data/config/app_config.toml` 不存在但旧 AppData 配置存在：
- 读取旧配置。
- 如果路径仍是旧默认 Documents 目录，改成 `data/` 下的新默认。
- 保存到新配置路径。
- 不删除旧配置。

**Step 4: 测试**

用 `ARTAIT_DATA_DIR` 指向临时目录，验证：
- 新配置写到 `data/config/app_config.toml`。
- 缺配置时返回 `Missing(AppConfig::default())`。
- 旧默认路径会被改写到 data。
- 用户自定义路径不会被改写。

Run:

```powershell
cargo test -p artait-config
```

## Task 3: 其它运行数据迁到 `data`

**Files:**
- Modify: `crates/artait-app/src/main.rs`
- Modify: `crates/artait-app/src/task_history.rs`
- Modify: `crates/artait-app/src/theme.rs`
- Modify: `crates/artait-app/src/onboarding.rs`
- Modify: `crates/artait-asset/src/thumbnail.rs`

**Step 1: 日志路径**

把运行日志从 exe 同级根目录改为：

```text
data\logs\ArtAITRust.log
```

`RuntimeLogWriter` 写入前创建 `data/logs`。

**Step 2: 任务历史**

把任务历史从 AppData 改为：

```text
data\tasks\task_history.json
```

如果旧 AppData 历史存在且新历史不存在，可以复制一份到新路径。

**Step 3: 主题**

把用户主题从系统 config 目录改为：

```text
data\themes\user.toml
```

监听目录也改成 `data/themes`。

**Step 4: 缩略图**

把缩略图缓存从 LocalAppData 改为：

```text
data\cache\thumbnails
```

缩略图可重建，不需要迁移旧缓存。

**Step 5: 首启引导草稿**

把 `.onboarding-draft.toml` 放到：

```text
data\config\.onboarding-draft.toml
```

首启默认输入/输出/提示词路径会自然来自新的 `PathConfig::default()`。

## Task 4: 引入中心化资产元数据索引

**Files:**
- Modify: `Cargo.toml`
- Modify: `crates/artait-asset/Cargo.toml`
- Create: `crates/artait-asset/src/metadata.rs`
- Modify: `crates/artait-asset/src/lib.rs`

**Step 1: 加依赖**

推荐：

```toml
rusqlite = { version = "0.32", features = ["bundled", "chrono"] }
```

`bundled` 对绿色版更合适，减少用户机器缺 SQLite 的问题。

**Step 2: 定义 Store**

```rust
pub struct AssetMetadataStore {
    db_path: PathBuf,
}

impl AssetMetadataStore {
    pub fn open(data_dir: &Path) -> anyhow::Result<Self>;
    pub fn upsert_generated(&self, item: GeneratedAssetMetadata) -> anyhow::Result<()>;
    pub fn find_by_path(&self, path: &Path) -> anyhow::Result<Option<StoredAssetMetadata>>;
}
```

`open()` 使用 `data/index/asset_index.sqlite`，不是 `output_dir/.artait`。

**Step 3: 实现 schema migrations**

执行：

```sql
PRAGMA journal_mode=WAL;
PRAGMA foreign_keys=ON;
```

并创建表和索引：

```sql
CREATE INDEX idx_assets_abs_path ON assets(abs_path);
CREATE INDEX idx_assets_portable_rel ON assets(portable_rel_path);
CREATE INDEX idx_assets_domain_created ON assets(domain, created_at DESC);
CREATE INDEX idx_generation_quality ON generation_metadata(quality);
CREATE INDEX idx_generation_aspect ON generation_metadata(aspect_ratio);
```

**Step 4: 测试**

```powershell
cargo test -p artait-asset metadata_
```

## Task 5: 图片保存成功后写入元数据

**Files:**
- Modify: `crates/artait-task/src/saver.rs`
- Modify: `crates/artait-app/src/main.rs`

**Step 1: 保留 provider metadata**

`SavedAsset` 增加：

```rust
pub provider_metadata: serde_json::Value,
```

`ResultSaver::save()` 从 `GenerationOutput` 中把 metadata 带出来。

**Step 2: 保存后读取宽高**

保存成功后使用 `image::image_dimensions()` 读取宽高。失败只记 warning，不影响图片保存。

**Step 3: 普通生图写入**

在 `on_generate_image` 普通分支保存成功后写入：
- prompt
- quality
- aspect_ratio
- width/height
- mode
- provider/model
- source_task_id
- portable_rel_path

**Step 4: 批量动作写入**

`mode == "action_sequence"` 每行保存独立记录：
- `prompt = line`
- `image_index = line_idx`
- `count = lines.len()`

**Step 5: 分镜写入**

分镜保存后写入：
- `prompt = full_prompt`
- `mode = storyboard`
- `quality = 2K`
- `aspect_ratio = aspect`

**Step 6: 失败策略**

元数据写入失败不能让生图任务失败。图片已保存就是主成功；元数据失败只写运行日志并更新状态栏。

## Task 6: 图库扫描合并元数据

**Files:**
- Modify: `crates/artait-model/src/asset.rs`
- Modify: `crates/artait-asset/src/lib.rs`
- Modify: `crates/artait-app/src/assets.rs`
- Modify: `ui/app-state.slint`

**Step 1: 扩展 `Asset`**

增加可选字段：

```rust
pub prompt: Option<String>,
pub quality: Option<String>,
pub aspect_ratio: Option<String>,
pub provider_id: Option<String>,
pub model: Option<String>,
```

都加 `#[serde(default)]`。

**Step 2: 扫描时合并**

`AssetLibrary::scan()` 仍以文件系统为准：
1. 扫 `data/out` 或用户配置的 output_dir。
2. 对每个文件查 `data/index/asset_index.sqlite`。
3. 查到则合并元数据。
4. 查不到仍显示文件。

**Step 3: UI 模型预留字段**

`AssetItem` 增加：

```slint
prompt: string,
quality: string,
aspect-ratio: string,
model: string,
width: int,
height: int,
```

第一阶段可以先不大改图库 UI，但数据要进入模型。

## Task 7: 旧数据迁移与重建索引

**Files:**
- Modify: `crates/artait-app/src/migrate.rs`
- Create or Modify: `crates/artait-asset/src/rebuild.rs`

**迁移策略：**
- 新用户：直接使用 `data/`。
- 老用户且仍使用旧默认 `Documents\ArtAIT`：可自动复制目录到 `data/`，或首次提示用户迁移。
- 老用户且配置是自定义路径：保留路径，只把配置文件本身迁到 `data/config`。

**重建索引功能：**
- 扫描 output_dir，把旧图片补入 `assets` 表。
- 旧图片没有 prompt/quality/aspect，只补文件路径、尺寸、大小、时间。
- 删除不存在文件对应记录，或标记 `deleted = 1`。

后续设置页可以加按钮：

```text
重建图库索引
```

## 验收标准

必须满足：
- 首次启动默认创建并使用 `<exe_dir>/data`。
- `app_config.toml` 默认在 `data/config/`。
- 图片默认保存到 `data/out/`。
- 运行日志默认在 `data/logs/`。
- 任务历史默认在 `data/tasks/`。
- 缩略图默认在 `data/cache/thumbnails/`。
- 用户主题默认在 `data/themes/user.toml`。
- 图片元数据索引默认在 `data/index/asset_index.sqlite`。
- 设置页仍可改输出目录，改后生图保存到用户指定目录。
- 生成图片后，索引里有 prompt、quality、aspect_ratio、width、height。
- 不在每张图片旁边创建 JSON。

建议验证命令：

```powershell
cargo test -p artait-model path
cargo test -p artait-config
cargo test -p artait-asset metadata_
cargo test -p artait-task save_
cargo check -p artait-app
cargo build -p artait-app --bin ArtAITRust --profile dev-fast
```

手工验证：
1. 删除或换空 `data/` 后启动。
2. 检查 `data/config/app_config.toml` 是否生成。
3. 生成一张图片。
4. 检查图片是否在 `data/out/...`。
5. 检查 `data/index/asset_index.sqlite` 是否有记录。
6. 重启应用，图库仍能显示图片和元数据。

## 推荐实施顺序

1. 先做绿色版 `portable_data_dir()` 和默认路径。
2. 再迁配置、日志、历史、主题、缩略图。
3. 再做 SQLite 元数据索引。
4. 再把普通生图、批量动作、分镜保存接入索引。
5. 最后让图库读取并展示元数据。

这个顺序最稳：先把数据根目录统一，再做图片元数据，否则后面会反复改路径。
