//! ArtAIT 业务编排层。
//!
//! 本 crate 负责把底层能力（provider / task / asset / config）编排成
//! 面向 UI 的业务方法。`artait-app` 通过本层调用，避免在 callback
//! handler 里直接写业务逻辑。
//!
//! 模块一览：
//! - `character_calibrator` — AI 角色校准流水线（提取→统计→批量校准→锚点补全）
//! - `character_prompt` — 角色提示词构建（6 层锚点翻译、角色表、变体）
//! - `character_store` — 角色库存储（JSON 持久化、CRUD、搜索筛选）
//! - `generation` — 生图任务构建、元数据持久化、模式配置映射
//! - `script` — 动画脚本生成、分镜包拆分
//! - `prompt_template` — 提示词模板管理与 Prompt 优化
//! - `task_history` — 任务历史持久化（JSON 文件）
//! - `provider_helpers` — Provider 辅助函数（密钥、模型、规范化）
//! - `onboarding` — 首启引导草稿数据类型

pub mod assets;
pub mod character_calibrator;
pub mod character_generation;
pub mod character_prompt;
pub mod character_store;
pub mod director_prompt;
pub mod director_store;
pub mod generation;
pub mod onboarding;
pub mod page_routing;
pub mod project_store;
pub mod prompt_template;
pub mod provider;
pub mod provider_helpers;
pub mod scene_prompt;
pub mod scene_store;
pub mod script;
pub mod script_index;
pub mod script_parser;
pub mod script_pipeline;
pub mod settings;
pub mod sidecar;
pub mod task_filter;
pub mod task_history;
pub mod utils;

/// 从任务闭包传给桥接层的元数据。
#[derive(Debug, Clone, Default)]
pub struct TaskMeta {
    pub output_path: String,
    pub provider_instance_id: String,
    pub provider_id: String,
    pub model: String,
    pub prompt: String,
    pub provider_task_id: String,
    pub endpoint: String,
    pub extra_json: String,
    /// 原始生图输出的 URL，下载失败时可重新下载
    pub retry_source_url: String,
}
