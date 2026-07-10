# ArtAIT Rust 重构资料：Provider 合约

结论：provider 合约是 Rust 重构的第一优先级。Rust 版本采用 **单一 Provider trait + 能力查询方法 + 协议族驱动 + JSON schema 配置**。这样既能让 `Arc<dyn Provider>` 在 Registry 里安全分发，又支持用户在不发版的情况下通过配置新增协议兼容实例。

## 设计原则

1. **单一 trait**：`Arc<dyn Provider>` 不需要 downcast，能力通过 `as_xxx()` 方法返回 `Option<&dyn Capability>`。
2. **协议族**：内置若干"协议族"实现（OpenAI 兼容、Gemini 兼容、Wavespeed 兼容等），用户在配置里选协议族 + 端点 + 模型即可，不发版。
3. **schema 驱动 UI**：provider 不返回 UI 组件，只暴露 JSON schema，前端按 schema 渲染表单。
4. **HTTP 抽象**：trait 不直接依赖 `reqwest`，通过 `HttpClient` 抽象暴露最小面，便于测试 mock。
5. **结果归一化**：provider 只返回 `GenerationOutput`，保存逻辑在 `ResultSaver` 单独处理。

## 当前能力矩阵

| 能力标识 | 含义 |
|---------|------|
| `generate` | 普通图片生成 |
| `generate_character` | 多参考图角色生成 |
| `generate_video` | 视频生成 |
| `analyze` | 图片或文本推理分析 |
| `test_connection` | 连接测试 |
| `quota` | 余额/额度查询 |
| `upload_binary` | provider 专属上传 |
| `poll_task` | 轮询异步任务 |

旧版包含 OpenAI / Gemini / DeepSeek / Wavespeed / ToAPI / IkunCode / Volcengine Seedance / Prompt Optimizer / Rembg / PhotoRoom 等。新版按协议族整理。

## ProviderMeta

```rust
pub struct ProviderMeta {
    pub id: &'static str,
    pub display_name: &'static str,
    pub family: ProviderFamily,
    pub capabilities: ProviderCapabilities,
    pub default_models: DefaultModels,
    pub config_schema: &'static str,  // JSON schema 字面量
    pub is_legacy: bool,
}

pub struct ProviderCapabilities {
    pub generate: bool,
    pub generate_character: bool,
    pub generate_video: bool,
    pub analyze: bool,
    pub test_connection: bool,
    pub quota: bool,
    pub upload_binary: bool,
    pub poll_task: bool,
}

pub struct DefaultModels {
    pub generation: &'static [&'static str],
    pub analysis: &'static [&'static str],
    pub video: &'static [&'static str],
}

pub enum ProviderFamily {
    OpenAICompatible,
    GeminiCompatible,
    WavespeedCompatible,
    VolcengineSeedance,
    DeepSeek,
    Ikuncode,
    Rembg,
    PhotoRoom,
    Custom(&'static str),
}
```

## Provider trait（单一 trait + 能力查询）

```rust
pub trait Provider: Send + Sync {
    fn meta(&self) -> &ProviderMeta;

    fn test_connection<'a>(&'a self, ctx: &'a ProviderContext)
        -> BoxFuture<'a, Result<ConnectionStatus, ProviderError>>;

    fn as_image_generator(&self) -> Option<&dyn ImageGenerator> { None }
    fn as_character_generator(&self) -> Option<&dyn CharacterGenerator> { None }
    fn as_analyzer(&self) -> Option<&dyn Analyzer> { None }
    fn as_video_generator(&self) -> Option<&dyn VideoGenerator> { None }
    fn as_pollable(&self) -> Option<&dyn Pollable> { None }
    fn as_uploader(&self) -> Option<&dyn Uploader> { None }
}
```

能力子 trait（不再继承 `Provider`，避免无法 downcast）：

```rust
pub trait ImageGenerator: Send + Sync {
    fn generate<'a>(&'a self, req: ImageGenerationRequest, ctx: &'a ProviderContext)
        -> BoxFuture<'a, Result<GenerationOutput, ProviderError>>;
}

pub trait CharacterGenerator: Send + Sync {
    fn generate_character<'a>(&'a self, req: CharacterGenerationRequest, ctx: &'a ProviderContext)
        -> BoxFuture<'a, Result<GenerationOutput, ProviderError>>;
}

pub trait Analyzer: Send + Sync {
    fn analyze<'a>(&'a self, req: AnalysisRequest, ctx: &'a ProviderContext)
        -> BoxFuture<'a, Result<AnalysisOutput, ProviderError>>;
}

pub trait VideoGenerator: Send + Sync {
    fn generate_video<'a>(&'a self, req: VideoGenerationRequest, ctx: &'a ProviderContext)
        -> BoxFuture<'a, Result<VideoOutput, ProviderError>>;
}

pub trait Pollable: Send + Sync {
    fn poll<'a>(&'a self, provider_task_id: &'a str, ctx: &'a ProviderContext)
        -> BoxFuture<'a, Result<Option<GenerationOutput>, ProviderError>>;
}

pub trait Uploader: Send + Sync {
    fn upload<'a>(&'a self, file: &'a [u8], mime: &'a str, ctx: &'a ProviderContext)
        -> BoxFuture<'a, Result<UploadedRef, ProviderError>>;
}
```

调用方写法：

```rust
let provider = registry.get(&instance.provider_id)?;
let generator = provider.as_image_generator()
    .ok_or(ProviderError::UnsupportedCapability)?;
let output = generator.generate(req, &ctx).await?;
```

## ProviderContext

不暴露 `reqwest`，只暴露最小面：

```rust
pub struct ProviderContext {
    pub instance_id: String,
    pub provider_id: String,
    pub config: ProviderInstanceConfig,
    pub secret: SecretSnapshot,
    pub output_path: PathBuf,
    pub run_dir: Option<PathBuf>,
    pub cancellation: CancellationToken,
    pub http: Arc<dyn HttpClient>,
    pub logger: Arc<dyn ProviderLogger>,
}
```

`HttpClient` 是抽象 trait，默认实现包 `reqwest`，测试可换 `wiremock`。`ProviderLogger` 自动脱敏。

## 请求模型

### ImageGenerationRequest

- `prompt`
- `negative_prompt`
- `reference_images`
- `aspect_ratio`
- `resolution`
- `size`
- `mode` — 标识当前页面（场景/UI/特效/分镜板等）
- `action_name` — 动作序列时使用
- `metadata` — 自由扩展

### CharacterGenerationRequest

- `prompt`
- `reference_images`
- `aspect_ratio`
- `resolution`
- `metadata`

### AnalysisRequest

- `system_prompt`
- `user_prompt`
- `images`
- `model`
- `response_format` — 指定 plain / json

### VideoGenerationRequest

- `prompt`
- `image`
- `duration`
- `aspect_ratio`
- `resolution`
- `generate_audio`
- `metadata`

## 输出模型

```rust
pub enum GenerationOutput {
    File { path: PathBuf, metadata: serde_json::Value },
    Url { url: String, metadata: serde_json::Value },
    Base64 { data: String, mime: String, metadata: serde_json::Value },
    AsyncTask { provider_task_id: String, metadata: serde_json::Value },
}

pub struct AnalysisOutput {
    pub text: String,
    pub structured: Option<serde_json::Value>,
    pub usage: Option<TokenUsage>,
}

pub struct VideoOutput {
    pub kind: GenerationOutput,  // 复用四种形态
    pub duration_seconds: Option<f32>,
    pub has_audio: bool,
}

pub struct UploadedRef {
    pub url: String,
    pub provider_file_id: Option<String>,
}
```

`AsyncTask` 由 `TaskRunner` 通过 `Pollable::poll` 轮询，UI 层不直接调 provider 轮询。保存逻辑由 `ResultSaver` 统一处理，不散落在页面或 provider。

## 上传策略

```rust
pub enum UploadMode {
    ExternalImageHost,   // 走图床
    ProviderBinary,      // provider 专属上传
    InlineBase64,        // 内联 base64
    LocalFile,           // 本地文件路径
}
```

provider 在 `ProviderMeta` 声明默认上传方式；调用方按需要覆盖。

## 错误模型

```rust
#[derive(thiserror::Error, Debug)]
pub enum ProviderError {
    #[error("invalid config: {0}")]
    InvalidConfig(String),
    #[error("missing secret: {0}")]
    MissingSecret(String),
    #[error("connection failed: {0}")]
    ConnectionFailed(String),
    #[error("rate limited")]
    RateLimited { retry_after: Option<Duration> },
    #[error("provider rejected: {0}")]
    ProviderRejected(String),
    #[error("provider timeout")]
    ProviderTimeout,
    #[error("invalid response: {0}")]
    InvalidResponse(String),
    #[error("task cancelled")]
    TaskCancelled,
    #[error("save failed: {0}")]
    SaveFailed(String),
    #[error("unsupported capability")]
    UnsupportedCapability,
}
```

UI 展示规则：

- 用户看到简短可操作信息。
- 详细请求信息进入 `tracing` 调试日志。
- 所有密钥脱敏。

## Provider Registry

```rust
pub struct ProviderRegistry {
    providers: HashMap<String, Arc<dyn Provider>>,
}

impl ProviderRegistry {
    pub fn list_for_capability(&self, cap: Capability) -> Vec<&ProviderMeta>;
    pub fn get(&self, provider_id: &str) -> Option<Arc<dyn Provider>>;
    pub fn default_for(&self, scope: ProviderScope) -> Option<String>;
    pub fn instantiate_from_family(
        &self,
        family: ProviderFamily,
        config: ProviderInstanceConfig,
    ) -> Result<Arc<dyn Provider>, ProviderError>;
}
```

`instantiate_from_family` 是配置驱动的核心：用户在 TOML 里写

```toml
[[provider_instances]]
id = "my-openai"
name = "我的 OpenAI 兼容服务"
family = "openai-compatible"
endpoint = "https://example.com/v1"
secret_ref = "artait/my-openai/api_key"
generation_models = ["gpt-image-1"]
```

应用启动时按 family 实例化对应 provider，注册到 Registry。新增协议族需要发版，新增**实例**不需要。

## 配置 schema

provider 不返回 Slint 组件。改为提供 JSON schema：

```rust
pub fn config_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "endpoint": { "type": "string", "format": "uri" },
            "api_key": { "type": "string", "secret": true },
            "model_options": {
                "type": "array",
                "items": { "type": "string" }
            }
        },
        "required": ["endpoint", "api_key"]
    })
}
```

字段扩展属性：

- `secret`：true 表示走 secret input，写入 Credential Manager。
- `label`、`help`：UI 提示。
- `default`：默认值。
- `options`：枚举选项。

`pages/settings.slint` 的 `ProviderInstanceEditor` 按 schema 动态渲染。

## Prompt Optimizer 合约

主应用通过 `artait-providers::prompt_optimizer::Client` 调用本地 Axum 服务：

- `GET /api/health`
- `POST /api/jobs`
- `GET /api/jobs/{id}`
- `POST /api/jobs/{id}/cancel`
- `POST /api/jobs/{id}/resume-auto`
- `POST /api/jobs/{id}/resume-step`
- `POST /api/jobs/{id}/steering`
- `GET /api/settings`
- `POST /api/settings/update`
- `POST /api/settings/test-connection`

UI 不直接知道服务细节，由 `artait-service::prompt_optimization` 封装。Slint 通过 callback 触发。

## 新增 provider 检查表

新增协议族时：

- 声明 `ProviderMeta`。
- 实现 `Provider` 与所需能力子 trait。
- 提供 JSON schema。
- 实现连接测试。
- 实现请求构造与响应解析。
- 支持取消检查（`ctx.cancellation`）。
- 密钥脱敏。
- 写单元测试 + `wiremock` 契约测试。
- 写至少一个集成 smoke 测试。

新增实例（用户行为）：

- 在设置页选协议族。
- 填 endpoint / 模型 / 密钥。
- 连接测试。
- 保存并选用。

## 与旧版的兼容

- 旧 `config.json` 的 provider 列表迁移到新 `app_config.toml` 时，按 provider_id 映射到对应协议族。
- 旧的 `chivesAI`、`prompt_optimizer.py`、`Rembg`、`PhotoRoom` 在新版本内置为协议族。
- 旧的 ad-hoc Python provider 不强制迁移；用户在新版下重建实例。
