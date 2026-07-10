//! 剧本数据模型。
//!
//! 定义剧本的完整结构化表示：剧集信息、场次、镜头、对白、
//! 场景原始内容、剧级元数据等。供剧本解析、角色提取、分镜生成使用。

use serde::{Deserialize, Serialize};

// ============================================================================
// 剧本顶层结构
// ============================================================================

/// 完整剧本 —— 多集剧本的顶层容器。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Script {
    /// 剧级元数据
    #[serde(default)]
    pub meta: SeriesMeta,
    /// 各集原始/解析后内容
    #[serde(default)]
    pub episodes: Vec<Episode>,
}

/// 剧级元数据 —— 所有集共享的核心信息。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SeriesMeta {
    /// 剧名
    #[serde(default)]
    pub title: String,
    /// 一句话概括
    #[serde(default)]
    pub logline: Option<String>,
    /// 故事大纲（100-500 字）
    #[serde(default)]
    pub outline: Option<String>,
    /// 主线矛盾
    #[serde(default)]
    pub central_conflict: Option<String>,
    /// 主题关键词
    #[serde(default)]
    pub themes: Vec<String>,

    // 世界观
    /// 时代背景：古代/现代/未来/民国/唐朝…
    #[serde(default)]
    pub era: Option<String>,
    /// 类型：武侠/商战/爱情/悬疑…
    #[serde(default)]
    pub genre: Option<String>,
    /// 精确时间线设定，如 "2022年夏天"
    #[serde(default)]
    pub timeline_setting: Option<String>,
    /// 故事开始年份
    #[serde(default)]
    pub story_start_year: Option<i32>,
    /// 故事结束年份
    #[serde(default)]
    pub story_end_year: Option<i32>,
    /// 总集数
    #[serde(default)]
    pub total_episodes: Option<u32>,

    // 角色体系（从剧本中提取，Phase 2 接入 calibrator 后自动填充）
    /// 角色列表
    #[serde(default)]
    pub characters: Vec<String>,
    /// 人物小传（自由文本）
    #[serde(default)]
    pub character_bios: Option<String>,

    // 视觉系统
    /// 视觉风格 ID
    #[serde(default)]
    pub style_id: Option<String>,
    /// 常驻场景名列表
    #[serde(default)]
    pub recurring_locations: Vec<String>,
    /// 全剧主色调
    #[serde(default)]
    pub color_palette: Option<String>,

    /// 世界观补充（自由文本）
    #[serde(default)]
    pub world_notes: Option<String>,
}

// ============================================================================
// 集
// ============================================================================

/// 一集剧本。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Episode {
    /// 集索引（0-based）
    pub episode_index: u32,
    /// 集标题
    #[serde(default)]
    pub title: String,
    /// 集大纲/摘要
    #[serde(default)]
    pub synopsis: Option<String>,
    /// 本集关键事件
    #[serde(default)]
    pub key_events: Vec<String>,
    /// 原始完整文本内容
    #[serde(default)]
    pub raw_content: String,
    /// 解析后的场景列表
    #[serde(default)]
    pub scenes: Vec<SceneContent>,
    /// 季节（春/夏/秋/冬）
    #[serde(default)]
    pub season: Option<String>,
    /// 分镜生成状态
    #[serde(default)]
    pub shot_generation_status: ShotGenerationStatus,
}

impl Episode {
    pub fn new(index: u32, title: String) -> Self {
        Self {
            episode_index: index,
            title,
            synopsis: None,
            key_events: vec![],
            raw_content: String::new(),
            scenes: vec![],
            season: None,
            shot_generation_status: ShotGenerationStatus::Idle,
        }
    }
}

/// 分镜生成状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ShotGenerationStatus {
    #[default]
    Idle,
    Generating,
    Completed,
    Error,
}

// ============================================================================
// 场景原始内容
// ============================================================================

/// 场景原始内容 —— 解析自剧本原始文本的一场戏。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SceneContent {
    /// 场景头，如 "1-1 日 内 沪上 张家"
    #[serde(default)]
    pub scene_header: String,
    /// 出场人物名列表
    #[serde(default)]
    pub characters: Vec<String>,
    /// 完整场景内容（对白+动作+字幕等）
    #[serde(default)]
    pub content: String,
    /// 解析后的对白列表
    #[serde(default)]
    pub dialogues: Vec<DialogueLine>,
    /// 动作描写列表（△ 开头的行）
    #[serde(default)]
    pub actions: Vec<String>,
    /// 字幕列表（【】包裹的内容）
    #[serde(default)]
    pub subtitles: Vec<String>,
    /// 天气（晴/雨/雪/雾/阴）
    #[serde(default)]
    pub weather: Option<String>,
    /// 时间（日/夜/晨/暮）
    #[serde(default)]
    pub time_of_day: Option<String>,
}

impl SceneContent {
    /// 获取该场景中所有说话的角色名（去重）。
    pub fn speaking_characters(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self
            .dialogues
            .iter()
            .map(|d| d.character.as_str())
            .collect();
        names.sort_unstable();
        names.dedup();
        names
    }

    /// 该场景中出场的所有角色（含场景头标注 + 对白角色）。
    pub fn all_characters(&self) -> Vec<&str> {
        let mut all: Vec<&str> = self
            .characters
            .iter()
            .map(|s| s.as_str())
            .chain(self.dialogues.iter().map(|d| d.character.as_str()))
            .collect();
        all.sort_unstable();
        all.dedup();
        all
    }
}

// ============================================================================
// 对白
// ============================================================================

/// 单条对白。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DialogueLine {
    /// 说话角色名
    pub character: String,
    /// 括号内动作/情绪，如 "喝酒" → （喝酒）
    #[serde(default)]
    pub parenthetical: Option<String>,
    /// 台词内容
    #[serde(default)]
    pub line: String,
}

// ============================================================================
// 镜头（分镜生成后）
// ============================================================================

/// 单个镜头 —— 分镜生成后的最小单元。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Shot {
    /// 唯一 ID
    pub id: String,
    /// 镜头编号（如 1, 2, 3...）
    pub number: u32,
    /// 所属集索引
    pub episode_index: u32,
    /// 所属场景头
    #[serde(default)]
    pub scene_header: String,

    // 镜头描述
    /// 动作/画面描述
    #[serde(default)]
    pub action: String,
    /// 对白列表
    #[serde(default)]
    pub dialogues: Vec<DialogueLine>,
    /// 出场角色
    #[serde(default)]
    pub characters: Vec<String>,

    // 电影语言参数
    /// 景别：特写/近景/中景/全景/远景…
    #[serde(default)]
    pub shot_size: Option<String>,
    /// 机位角度：平视/俯视/仰视/鸟瞰…
    #[serde(default)]
    pub camera_angle: Option<String>,
    /// 镜头运动：固定/推/拉/摇/移/跟…
    #[serde(default)]
    pub camera_movement: Option<String>,
    /// 焦距：广角/标准/长焦…
    #[serde(default)]
    pub focal_length: Option<String>,

    // 生成相关
    /// AI 生成用的英文提示词
    #[serde(default)]
    pub visual_prompt_en: Option<String>,
    /// AI 生成用的中文提示词
    #[serde(default)]
    pub visual_prompt_zh: Option<String>,
    /// 生成的图片路径
    #[serde(default)]
    pub generated_image_path: Option<String>,
    /// 生成状态
    #[serde(default)]
    pub generation_status: ShotGenerationStatus,
}

// ============================================================================
// 分镜包（从镜头分组）
// ============================================================================

/// 分镜包 —— 连续镜头组，用于批量生成。
///
/// 与 `artait-service::script::StoryboardPackage` 语义相同但包含更多结构化数据。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShotPackage {
    /// 包索引（0-based）
    pub index: usize,
    /// 标签，如 "镜头 1–3"
    pub label: String,
    /// 包内镜头数
    pub shot_count: usize,
    /// 包内镜头列表
    pub shots: Vec<Shot>,
    /// Markdown 格式文本（兼容现有 storyboard 页面）
    #[serde(default)]
    pub markdown: String,
}

// ============================================================================
// 辅助：从原始文本检测信息
// ============================================================================

/// 场景头解析结果。
#[derive(Debug, Clone)]
pub struct SceneHeaderParsed {
    /// 原始场景头文本
    pub raw: String,
    /// 集号
    pub episode_number: Option<u32>,
    /// 场次号
    pub scene_number: Option<u32>,
    /// 时间：日/夜/晨/暮
    pub time_of_day: Option<String>,
    /// 内外景：内/外
    pub interior_exterior: Option<String>,
    /// 地点描述
    pub location: Option<String>,
}

/// 尝试解析中文剧本场景头格式，如 "1-1 日 内 沪上 张家"。
pub fn parse_scene_header(header: &str) -> SceneHeaderParsed {
    let raw = header.trim().to_string();
    let parts: Vec<&str> = raw.split_whitespace().collect();

    let mut result = SceneHeaderParsed {
        raw: raw.clone(),
        episode_number: None,
        scene_number: None,
        time_of_day: None,
        interior_exterior: None,
        location: None,
    };

    if parts.is_empty() {
        return result;
    }

    // 解析 "1-1" 格式的集-场编号
    if let Some(ep_scene) = parts.first() {
        if let Some(hyphen) = ep_scene.find('-') {
            result.episode_number = ep_scene[..hyphen].parse().ok();
            result.scene_number = ep_scene[hyphen + 1..].parse().ok();
        }
    }

    // 解析时间（日/夜/晨/暮）
    for &part in &parts {
        match part {
            "日" | "白天" => result.time_of_day = Some("日".into()),
            "夜" | "晚" | "夜晚" => result.time_of_day = Some("夜".into()),
            "晨" | "早" | "清晨" => result.time_of_day = Some("晨".into()),
            "暮" | "黄昏" | "傍晚" => result.time_of_day = Some("暮".into()),
            _ => {}
        }
    }

    // 解析内外景
    for &part in &parts {
        match part {
            "内" => result.interior_exterior = Some("内".into()),
            "外" => result.interior_exterior = Some("外".into()),
            _ => {}
        }
    }

    // 剩余部分作为地点
    let known = [
        "日", "夜", "晨", "暮", "内", "外", "白天", "夜晚", "清晨", "黄昏", "傍晚", "早", "晚",
    ];
    let loc_parts: Vec<&str> = parts
        .iter()
        .skip(1) // 跳过编号
        .filter(|p| !known.contains(p) && !p.parse::<u32>().is_ok())
        .copied()
        .collect();

    if !loc_parts.is_empty() {
        result.location = Some(loc_parts.join(" "));
    }

    result
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_standard_scene_header() {
        let h = parse_scene_header("1-1 日 内 沪上 张家");
        assert_eq!(h.episode_number, Some(1));
        assert_eq!(h.scene_number, Some(1));
        assert_eq!(h.time_of_day.as_deref(), Some("日"));
        assert_eq!(h.interior_exterior.as_deref(), Some("内"));
        assert!(h.location.unwrap().contains("沪上"));
    }

    #[test]
    fn parse_night_scene() {
        let h = parse_scene_header("3-5 夜 外 京城 街头");
        assert_eq!(h.episode_number, Some(3));
        assert_eq!(h.scene_number, Some(5));
        assert_eq!(h.time_of_day.as_deref(), Some("夜"));
        assert_eq!(h.interior_exterior.as_deref(), Some("外"));
    }

    #[test]
    fn parse_minimal_header() {
        let h = parse_scene_header("1 日 内");
        assert_eq!(h.time_of_day.as_deref(), Some("日"));
        assert_eq!(h.interior_exterior.as_deref(), Some("内"));
    }

    #[test]
    fn parse_empty_header() {
        let h = parse_scene_header("");
        assert!(h.episode_number.is_none());
    }

    #[test]
    fn all_characters_dedupes() {
        let scene = SceneContent {
            characters: vec!["张三".into(), "李四".into()],
            dialogues: vec![
                DialogueLine {
                    character: "张三".into(),
                    parenthetical: None,
                    line: "你好".into(),
                },
                DialogueLine {
                    character: "王五".into(),
                    parenthetical: None,
                    line: "来了".into(),
                },
            ],
            ..Default::default()
        };
        let all = scene.all_characters();
        assert_eq!(all.len(), 3);
        assert!(all.contains(&"张三"));
        assert!(all.contains(&"李四"));
        assert!(all.contains(&"王五"));
    }

    #[test]
    fn episode_defaults_to_idle() {
        let ep = Episode::new(0, "第一集".into());
        assert_eq!(ep.shot_generation_status, ShotGenerationStatus::Idle);
    }

    #[test]
    fn script_serialization_roundtrip() {
        let script = Script {
            meta: SeriesMeta {
                title: "测试剧本".into(),
                genre: Some("武侠".into()),
                era: Some("古代".into()),
                ..Default::default()
            },
            episodes: vec![Episode::new(0, "序幕".into())],
        };
        let json = serde_json::to_string(&script).unwrap();
        let back: Script = serde_json::from_str(&json).unwrap();
        assert_eq!(back.meta.title, "测试剧本");
        assert_eq!(back.episodes.len(), 1);
    }
}
