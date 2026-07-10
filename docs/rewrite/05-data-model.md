# ArtAIT Rust 重构资料：数据模型

结论：Rust 版本先稳定数据模型，再做页面迁移。旧版状态散落在 UI 对象、`config.json`、本地目录和 provider 返回值里；重构目标是把这些状态显式建模并集中到 `artait-model` crate。配置走 TOML，API Key 直接保存到本机配置文件。

## 本地目录模型

旧目录语义：

- `prompt/` 提示词模板资源。
- `reference_action/` 动作参考图。
- `reference_prompt/` 动作提示词模板。
- `input/` 用户输入素材和历史素材。
- `out/` 生成结果。
- `apply_prompt/` 动作序列运行时生成的提示词和外观分析。
- `Back/data/` Prompt Optimizer Studio 的 SQLite 数据。

新目录建议（首启引导让用户选择，默认在 `<我的文档>\ArtAIT\`）：

```text
%USERPROFILE%\Documents\ArtAIT\
  input\           用户素材
  out\             生成结果
    scenes\
    creations\
    effects\
    ui\
    animation_scenes\
    animation_characters\
    character_turnarounds\
    animation_scripts\
      _packages\
    storyboards\
    <角色名>\
  prompt\          模板（含内置 + 用户自定义）
  apply_prompt\    动作序列运行结果（兼容旧路径）
  reference_action\
  reference_prompt\

%APPDATA%\ArtAIT\
  app_config.toml
  themes\
    user.toml
  .onboarding-draft.toml

%LOCALAPPDATA%\ArtAIT\
  thumbnails\
  logs\
```

兼容策略：

- 第一阶段沿用旧目录结构。
- 引导第 2 步可选择旧目录路径或默认新路径。
- 业务层不直接拼路径字符串，统一通过 `PathConfig`。

## AppConfig

整体保存到 `%APPDATA%\ArtAIT\app_config.toml`。

```rust
pub struct AppConfig {
    pub schema_version: u32,
    pub paths: PathConfig,
    pub ui: UiConfig,
    pub features: FeatureConfig,
    pub providers: Vec<ProviderInstance>,
    pub provider_defaults: ProviderDefaults,
    pub image_processing: ImageProcessingConfig,
    pub prompt_optimizer: PromptOptimizerConfig,
    pub video_player: VideoPlayerConfig,
    pub last_main_tab: Option<String>,
    pub migrated_from: Option<PathBuf>,
}

pub struct PathConfig {
    pub input_dir: PathBuf,
    pub output_dir: PathBuf,
    pub prompt_dir: PathBuf,
    pub apply_prompt_dir: PathBuf,
    pub reference_action_dir: PathBuf,
    pub reference_prompt_dir: PathBuf,
}

pub struct UiConfig {
    pub theme: ThemeId,
    pub font_family: String,
    pub font_size: u32,
    pub locale: String,
}

pub struct FeatureConfig {
    pub preset: FeaturePreset,
    pub enabled: Vec<FeatureId>,
}

pub enum FeaturePreset {
    General,
    Animation,
    Full,
    Custom,
}

pub enum FeatureId {
    UiConcept,
    Scene,
    Character,
    Effect,
    ActionSequence,
    AssetBrowser,
    AnimationScene,
    AnimationCharacter,
    CharacterTurnaround,
    AnimationScript,
    Storyboard,
}

pub struct ProviderDefaults {
    pub generation: Option<String>,   // instance id
    pub analysis: Option<String>,
    pub video: Option<String>,
}
```

注意：

- 旧 `config.json` 中存在敏感信息和解析风险，重构时不复制真实密钥内容。
- 普通配置和密钥分离：TOML 只存 `secret_ref`。
- JSON / TOML 解析失败进入恢复模式，给默认值并记录错误。

## ThemeId 与主题

```rust
pub enum ThemeId {
    Dark,
    Light,
    System,
    User,
}

pub struct LoadedTheme {
    pub id: String,
    pub display_name: String,
    pub is_dark: bool,
    pub palette: ThemePalette,
    pub shape: ThemeShape,
    pub typography: ThemeTypography,
    pub spacing: ThemeSpacing,
    pub motion: ThemeMotion,
}
```

详见 `09-ui-theming.md`。

## ProviderInstance

用户选择的 provider 配置实例。

```rust
pub struct ProviderInstance {
    pub id: String,
    pub name: String,
    pub provider_id: String,         // 协议族实现 ID（编译期注册）
    pub family: ProviderFamily,
    pub scopes: Vec<ProviderScope>,
    pub show_in_main_ui: bool,
    pub models: ProviderModelConfig,
    pub endpoint: Option<String>,
    pub secret_ref: Option<String>,  // Credential Manager 凭据键名
    pub extra: serde_json::Value,    // 协议族专属配置
}

pub struct ProviderModelConfig {
    pub generation_model: Option<String>,
    pub generation_model_options: Vec<String>,
    pub analysis_model: Option<String>,
    pub analysis_model_options: Vec<String>,
    pub video_model: Option<String>,
    pub video_model_options: Vec<String>,
}

pub enum ProviderScope {
    Generation,
    Analysis,
    Video,
}
```

TOML 形式：

```toml
[[providers]]
id = "openai-1"
name = "OpenAI"
provider_id = "openai-compatible"
scopes = ["generation", "analysis"]
show_in_main_ui = true
endpoint = "https://api.openai.com/v1"
secret_ref = "artait/openai-1/api_key"

[providers.models]
generation_model = "gpt-image-1"
generation_model_options = ["gpt-image-1"]
analysis_model = "gpt-4o-mini"
analysis_model_options = ["gpt-4o", "gpt-4o-mini"]
```

## ProviderMeta

静态描述 provider 协议族能力。详见 `06-provider-contract.md`。

字段：`id`、`display_name`、`family`、`capabilities`、`default_models`、`config_schema`、`is_legacy`。

能力集合：`generate`、`generate_character`、`generate_video`、`analyze`、`test_connection`、`quota`、`upload_binary`、`poll_task`。

## PromptTemplate

用户可选择、编辑、复用的提示词模板。

```rust
pub enum PromptTemplateFormat {
    Txt,
    Json,
}

pub enum PromptDomain {
    UiConcept,
    Scene,
    Character,
    Effect,
    AnimationScene,
    AnimationCharacter,
    CharacterTurnaround,
    Storyboard,
    ActionSequence,
}

pub struct PromptTemplate {
    pub name: String,
    pub domain: PromptDomain,
    pub path: PathBuf,
    pub format: PromptTemplateFormat,
    pub positive_prompt: String,
    pub negative_prompt: Option<String>,
    pub reference_images: Vec<PathBuf>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
```

模板格式：

- `.txt` 纯正向提示词。
- `.json` 兼容 `ai_prompts.positive_prompt` 和 `ai_prompts.negative_prompt`。

## ReferenceImage

页面输入中的参考图。

```rust
pub struct ReferenceImage {
    pub local_path: PathBuf,
    pub display_name: String,
    pub mime_type: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub uploaded_url: Option<String>,
    pub upload_cache_key: Option<String>,
    pub source: ReferenceImageSource,
}

pub enum ReferenceImageSource {
    UserPicked,
    DragAndDrop,
    SingleInstanceImport,
    AddedFromAssetBrowser,
}
```

## GenerationTask

一次图像或视频生成任务。

```rust
pub struct GenerationTask {
    pub id: String,
    pub kind: TaskKind,
    pub mode: CreationMode,
    pub provider_instance_id: String,
    pub provider_id: String,
    pub model: String,
    pub prompt: String,
    pub negative_prompt: Option<String>,
    pub reference_images: Vec<ReferenceImage>,
    pub aspect_ratio: Option<String>,
    pub resolution: Option<(u32, u32)>,
    pub output_path: PathBuf,
    pub provider_task_id: Option<String>,
    pub status: TaskStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub error: Option<String>,
}

pub enum TaskKind {
    Image,
    Character,
    Video,
    Analysis,
    PromptOptimization,
    ActionBatch,
    ScriptGeneration,
}

pub enum TaskStatus {
    Idle,
    Validating,
    Uploading,
    Submitted,
    Polling,
    Saving,
    Completed,
    Cancelling,
    Cancelled,
    Failed,
}

pub enum CreationMode {
    Ui,
    Scene,
    Character,
    Effect,
    AnimationScene,
    AnimationCharacter,
    CharacterTurnaround,
    Storyboard,
    ActionSequence,
}
```

## ActionDefinition

可批处理的动作。

```rust
pub struct ActionDefinition {
    pub name: String,
    pub display_name: String,
    pub reference_path: PathBuf,
    pub prompt_template_path: Option<PathBuf>,
    pub grid: Option<String>,
    pub direction: Option<String>,
    pub frame_count: Option<u32>,
    pub overrides: serde_json::Value,
}
```

来源：`reference_action/`、`reference_prompt/`、可能的动作配置 JSON。

## AppearanceProfile

由角色图分析得到的结构化外观信息。

```rust
pub struct AppearanceProfile {
    pub body_shape: Option<String>,
    pub head: Option<String>,
    pub hair: Option<String>,
    pub face: Option<String>,
    pub clothing: Option<String>,
    pub accessories: Vec<String>,
    pub palette: Vec<String>,
    pub materials: Vec<String>,
    pub style: Option<String>,
    pub silhouette: Option<String>,
    pub notes: Vec<String>,
    pub raw: serde_json::Value,
}
```

落盘：

- `apply_prompt/<角色名>/appearance.json`
- `apply_prompt/<角色名>/appearance.txt`

## ActionBatchJob

```rust
pub struct ActionBatchJob {
    pub id: String,
    pub character_path: PathBuf,
    pub character_name: String,
    pub selected_actions: Vec<String>,
    pub force_prompt: bool,
    pub force_image: bool,
    pub generate_images: bool,
    pub lock_style: bool,
    pub prompt_output_dir: PathBuf,
    pub image_output_dir: PathBuf,
    pub appearance_profile: Option<AppearanceProfile>,
    pub items: Vec<ActionBatchItem>,
    pub status: TaskStatus,
}

pub struct ActionBatchItem {
    pub action_name: String,
    pub prompt_path: PathBuf,
    pub reference_path: PathBuf,
    pub image_output_path: PathBuf,
    pub status: TaskStatus,
    pub error: Option<String>,
}
```

## AnimationScript

```rust
pub struct AnimationScript {
    pub id: String,
    pub title: String,
    pub source_theme: String,
    pub source_docs: Vec<PathBuf>,
    pub reference_images: Vec<ReferenceImage>,
    pub markdown: String,
    pub path: PathBuf,
    pub created_at: DateTime<Utc>,
}
```

输出目录：`out/animation_scripts`。

## StoryboardPackage

```rust
pub struct StoryboardPackage {
    pub id: String,
    pub source_script_path: PathBuf,
    pub index: u32,
    pub shot_numbers: Vec<u32>,
    pub panel_count: u32,
    pub label: String,
    pub markdown: String,
    pub path: PathBuf,
}
```

输出目录：`out/animation_scripts/_packages/<脚本名>`。

## Asset

统一描述生成结果和输入素材。

```rust
pub struct Asset {
    pub id: String,
    pub path: PathBuf,
    pub kind: AssetKind,
    pub domain: AssetDomain,
    pub created_at: DateTime<Utc>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub duration_secs: Option<f32>,
    pub source_task_id: Option<String>,
    pub tags: Vec<String>,
}

pub enum AssetKind {
    Image,
    Video,
    Prompt,
    Script,
    StoryboardPackage,
}

pub enum AssetDomain {
    Scene,
    Character,
    Ui,
    Effect,
    AnimationScene,
    AnimationCharacter,
    CharacterTurnaround,
    Storyboard,
    ActionSequence,
    AnimationScript,
}
```

## TaskEvent

让后台任务向 UI 推进度。

```rust
pub enum TaskEvent {
    TaskStarted     { task_id: String, kind: TaskKind },
    TaskProgress    { task_id: String, fraction: f32 },
    TaskLog         { task_id: String, level: LogLevel, message: String },
    TaskRoundUpdate { task_id: String, round: u32, score: Option<f32> },
    TaskOutputCreated { task_id: String, asset: Asset },
    TaskCompleted   { task_id: String },
    TaskFailed      { task_id: String, error: String },
    TaskCancelled   { task_id: String },
}
```

`artait-task::TaskRunner` 通过 `tokio::sync::broadcast` 广播；`artait-app` 订阅并写入 Slint `AppState global`。

## 密钥模型

- API Key 直接保存到本机 `app_config.toml`，用于本机调试和设置页回显。
- `secret_ref` 仅作为旧凭据迁移兼容字段保留。
- 日志统一脱敏（基于 `tracing` 的 layer）。
- 导出诊断信息时默认不包含密钥。
- 旧凭据键命名约定：`artait/<instance_id>/<field>`。

迁移规则：

- 不自动把旧 `config.json` 的密钥写入新文件。
- 首次迁移时由"导入旧密钥"向导提示用户重新录入或确认导入。

## 模型层 crate 边界

`artait-model` 只放：

- `enum`、`struct`、`trait` 定义。
- `serde::Serialize / Deserialize` 实现。
- 简单转换（如 `ProviderScope::as_str`）。

不放：

- 异步代码。
- 文件 IO。
- HTTP 调用。
- Slint 绑定。

这样保证 `artait-model` 零依赖（只依赖 `serde`、`thiserror`、`chrono`），所有上层 crate 都可以无负担引用。
