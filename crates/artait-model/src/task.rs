//! 任务、状态机与事件类型。

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::asset::Asset;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    Image,
    Character,
    Video,
    Analysis,
    PromptOptimization,
    ActionBatch,
    ScriptGeneration,
}

impl TaskKind {
    /// 从历史持久化字符串反序列化，未知值默认为 Image。
    pub fn from_history_str(s: &str) -> Self {
        match s {
            "image" => TaskKind::Image,
            "character" => TaskKind::Character,
            "video" => TaskKind::Video,
            "analysis" => TaskKind::Analysis,
            "prompt_optimization" | "promptoptimization" => TaskKind::PromptOptimization,
            "action_batch" => TaskKind::ActionBatch,
            "script_generation" => TaskKind::ScriptGeneration,
            _ => TaskKind::Image,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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

impl TaskStatus {
    /// 转为 UI 端字符串标签。
    pub fn label(self) -> &'static str {
        match self {
            TaskStatus::Idle => "idle",
            TaskStatus::Validating => "validating",
            TaskStatus::Uploading => "uploading",
            TaskStatus::Submitted => "submitted",
            TaskStatus::Polling => "polling",
            TaskStatus::Saving => "saving",
            TaskStatus::Completed => "completed",
            TaskStatus::Cancelling => "cancelling",
            TaskStatus::Cancelled => "cancelled",
            TaskStatus::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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

impl CreationMode {
    /// 从页面路由字符串解析。
    pub fn from_route(route: &str) -> Self {
        match route {
            "scene" => Self::Scene,
            "character" => Self::Character,
            "ui_concept" => Self::Ui,
            "effect" => Self::Effect,
            "animation_scene" => Self::AnimationScene,
            "animation_character" => Self::AnimationCharacter,
            "character_turnaround" => Self::CharacterTurnaround,
            "action_sequence" => Self::ActionSequence,
            "storyboard" => Self::Storyboard,
            _ => Self::Scene,
        }
    }

    /// 反向映射：枚举 → 路由字符串。
    pub fn route_id(self) -> &'static str {
        match self {
            Self::Scene => "scene",
            Self::Character => "character",
            Self::Ui => "ui_concept",
            Self::Effect => "effect",
            Self::AnimationScene => "animation_scene",
            Self::AnimationCharacter => "animation_character",
            Self::CharacterTurnaround => "character_turnaround",
            Self::Storyboard => "storyboard",
            Self::ActionSequence => "action_sequence",
        }
    }

    /// 中文展示名。
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Scene => "创建场景",
            Self::Character => "创建角色",
            Self::Ui => "UI 概念",
            Self::Effect => "特效",
            Self::AnimationScene => "动画场景",
            Self::AnimationCharacter => "动画角色",
            Self::CharacterTurnaround => "角色三视图",
            Self::Storyboard => "分镜板",
            Self::ActionSequence => "动作序列",
        }
    }

    /// 文件系统输出子目录。
    pub fn output_subdir(self) -> &'static str {
        match self {
            Self::Ui => "ui",
            Self::Scene => "scenes",
            Self::Character => "creations",
            Self::Effect => "effects",
            Self::AnimationScene => "animation_scenes",
            Self::AnimationCharacter => "animation_characters",
            Self::CharacterTurnaround => "character_turnarounds",
            Self::ActionSequence => "batch",
            Self::Storyboard => "storyboards",
        }
    }

    /// 资产领域字符串。
    pub fn domain_str(self) -> &'static str {
        match self {
            Self::Character => "character",
            Self::Ui => "ui",
            Self::Effect => "effect",
            Self::AnimationScene => "animation_scene",
            Self::AnimationCharacter => "animation_character",
            Self::CharacterTurnaround => "character_turnaround",
            Self::Storyboard => "storyboard",
            Self::ActionSequence => "action_sequence",
            Self::Scene => "scene",
        }
    }

    /// 是否为生图模式（不含动作序列）。
    pub fn is_gen(self) -> bool {
        !matches!(self, Self::ActionSequence)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReferenceImage {
    pub local_path: PathBuf,
    pub display_name: String,
    pub mime_type: String,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
    #[serde(default)]
    pub uploaded_url: Option<String>,
    #[serde(default)]
    pub upload_cache_key: Option<String>,
    pub source: ReferenceImageSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceImageSource {
    UserPicked,
    DragAndDrop,
    SingleInstanceImport,
    AddedFromAssetBrowser,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationTask {
    pub id: String,
    pub kind: TaskKind,
    pub mode: CreationMode,
    pub provider_instance_id: String,
    pub provider_id: String,
    pub model: String,
    pub prompt: String,
    #[serde(default)]
    pub negative_prompt: Option<String>,
    #[serde(default)]
    pub reference_images: Vec<ReferenceImage>,
    #[serde(default)]
    pub aspect_ratio: Option<String>,
    #[serde(default)]
    pub resolution: Option<(u32, u32)>,
    pub output_path: PathBuf,
    #[serde(default)]
    pub provider_task_id: Option<String>,
    pub status: TaskStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TaskEvent {
    TaskStarted {
        task_id: String,
        kind: TaskKind,
    },
    TaskProgress {
        task_id: String,
        fraction: f32,
    },
    TaskLog {
        task_id: String,
        level: LogLevel,
        message: String,
    },
    TaskRoundUpdate {
        task_id: String,
        round: u32,
        score: Option<f32>,
    },
    TaskOutputCreated {
        task_id: String,
        asset: Asset,
    },
    TaskCompleted {
        task_id: String,
    },
    TaskFailed {
        task_id: String,
        error: String,
    },
    TaskCancelled {
        task_id: String,
    },
}

/// 判断任务状态字符串是否表示运行中状态（非终态）。
///
/// 终态包括 `completed`、`cancelled`、`failed`。
/// 运行中包括 `running`、`validating`、`uploading`、`submitted`、`polling`、`saving`。
pub fn is_active_task_status(status: &str) -> bool {
    matches!(
        status,
        "running" | "validating" | "uploading" | "submitted" | "polling" | "saving"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_task_status_recognizes_running() {
        assert!(is_active_task_status("running"));
        assert!(is_active_task_status("validating"));
        assert!(is_active_task_status("uploading"));
        assert!(is_active_task_status("submitted"));
        assert!(is_active_task_status("polling"));
        assert!(is_active_task_status("saving"));
    }

    #[test]
    fn active_task_status_rejects_terminal() {
        assert!(!is_active_task_status("completed"));
        assert!(!is_active_task_status("cancelled"));
        assert!(!is_active_task_status("failed"));
        assert!(!is_active_task_status("idle"));
        assert!(!is_active_task_status(""));
    }
}
