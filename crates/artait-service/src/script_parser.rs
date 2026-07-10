//! 剧本解析器。
//!
//! 将原始剧本文本（Markdown 或纯文本）解析为结构化的
//! Episode → SceneContent → DialogueLine 层次。

use anyhow::Result;
use artait_model::script::{DialogueLine, Episode, SceneContent};
use tracing::info;

// ============================================================================
// 主入口
// ============================================================================

/// 解析完整剧本文本为 Episode 列表。
///
/// 支持的格式：
/// - Markdown 格式：`# 剧名` / `## 第N集` / `### 场景头` / 对白行
/// - 纯文本格式：按空行分场，场景头行开头含数字编号
pub fn parse_script(raw: &str) -> Result<Vec<Episode>> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(vec![]);
    }

    // 尝试按 Markdown 结构解析
    let episodes = if has_markdown_structure(trimmed) {
        parse_markdown_script(trimmed)?
    } else {
        parse_plain_script(trimmed)?
    };

    info!(count = episodes.len(), "剧本解析完成");
    Ok(episodes)
}

// ============================================================================
// Markdown 格式解析
// ============================================================================

fn has_markdown_structure(text: &str) -> bool {
    let has_episode_header = text.lines().any(|l| {
        let t = l.trim();
        (t.starts_with("## ") || t.starts_with("## 第"))
            && (t.contains('集') || t.contains("Episode"))
    });
    let has_dialogue = text.lines().any(|l| {
        let t = l.trim();
        t.contains('：') || t.contains(':')
    });
    has_episode_header || (text.contains("## ") && has_dialogue)
}

fn parse_markdown_script(text: &str) -> Result<Vec<Episode>> {
    let mut episodes: Vec<Episode> = Vec::new();
    let mut current_ep: Option<EpisodeBuilder> = None;
    let mut current_scene: Option<SceneBuilder> = None;
    let mut episode_idx: u32 = 0;

    for line in text.lines() {
        let trimmed = line.trim();

        // 跳过空行
        if trimmed.is_empty() {
            continue;
        }

        // 跳过纯注释行
        if trimmed.starts_with("//") || trimmed.starts_with("--") {
            continue;
        }

        // ## 第N集 → 新集开始
        if trimmed.starts_with("## ") || trimmed.starts_with("## 第") {
            // 保存上一个场景
            if let Some(scene) = current_scene.take() {
                if let Some(ref mut ep) = current_ep {
                    ep.scenes.push(scene.build());
                }
            }
            // 保存上一集
            if let Some(ep) = current_ep.take() {
                episodes.push(ep.build());
            }

            let title = trimmed.trim_start_matches("## ").trim().to_string();
            current_ep = Some(EpisodeBuilder::new(episode_idx, title));
            current_scene = None;
            episode_idx += 1;
            continue;
        }

        // ### 场景头 → 新场景开始
        if trimmed.starts_with("### ") || is_scene_header_line(trimmed) {
            if let Some(scene) = current_scene.take() {
                if let Some(ref mut ep) = current_ep {
                    ep.scenes.push(scene.build());
                }
            }
            let header = trimmed.trim_start_matches("### ").trim().to_string();
            current_scene = Some(SceneBuilder::new(header));
            continue;
        }

        // 如果还没有集容器，创建默认集
        if current_ep.is_none() {
            current_ep = Some(EpisodeBuilder::new(0, "默认".into()));
        }
        // 如果还没有场景容器，创建默认场景
        if current_scene.is_none() {
            current_scene = Some(SceneBuilder::new("默认场景".into()));
        }

        let scene = current_scene.as_mut().unwrap();

        // 出场人物行：出场人物：张三, 李四 （必须在 try_parse_dialogue 之前，否则会被误认为对白）
        if trimmed.starts_with("出场人物") || trimmed.starts_with("出场角色") {
            // 安全查找冒号（全角/半角）
            if let Some((colon_byte, colon_char)) = trimmed
                .char_indices()
                .find(|(_, c)| *c == '：' || *c == ':')
            {
                let rest = &trimmed[colon_byte + colon_char.len_utf8()..];
                let names: Vec<String> = rest
                    .split(&[',', '，', '、', ' '][..])
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                scene.characters.extend(names);
            }
            continue;
        }

        // 对白行：角色名：台词 或 角色名: 台词
        if let Some(dialogue) = try_parse_dialogue(trimmed) {
            scene.dialogues.push(dialogue);
            continue;
        }

        // 动作行：△开头
        if trimmed.starts_with('△') {
            let action = trimmed.trim_start_matches('△').trim().to_string();
            if !action.is_empty() {
                scene.actions.push(action);
            }
            continue;
        }

        // 字幕：【字幕内容】
        if trimmed.starts_with('【') && trimmed.contains('】') {
            let sub = trimmed
                .trim_start_matches('【')
                .trim_end_matches('】')
                .to_string();
            if !sub.is_empty() {
                scene.subtitles.push(sub);
            }
            continue;
        }

        // 其他文本 → 内容行
        scene.content_lines.push(trimmed.to_string());
    }

    // 落盘最后的内容
    if let Some(scene) = current_scene.take() {
        if let Some(ref mut ep) = current_ep {
            ep.scenes.push(scene.build());
        }
    }
    if let Some(ep) = current_ep.take() {
        episodes.push(ep.build());
    }

    Ok(episodes)
}

// ============================================================================
// 纯文本格式解析
// ============================================================================

fn parse_plain_script(text: &str) -> Result<Vec<Episode>> {
    // 按场景头或空行分割
    let blocks = split_by_scene_headers(text);
    if blocks.is_empty() {
        return Ok(vec![]);
    }

    let mut scenes: Vec<SceneContent> = Vec::new();

    for block in &blocks {
        let mut builder = SceneBuilder::new("未命名场景".into());
        let lines: Vec<&str> = block.lines().collect();

        for line in &lines {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            // 首行作为场景头
            if builder.scene_header == "未命名场景" && is_scene_header_line(trimmed) {
                builder.scene_header = trimmed.to_string();
                continue;
            }

            if let Some(dialogue) = try_parse_dialogue(trimmed) {
                builder.dialogues.push(dialogue);
            } else if trimmed.starts_with('△') {
                let action = trimmed.trim_start_matches('△').trim().to_string();
                if !action.is_empty() {
                    builder.actions.push(action);
                }
            } else if trimmed.starts_with('【') && trimmed.contains('】') {
                let sub = trimmed
                    .trim_start_matches('【')
                    .trim_end_matches('】')
                    .to_string();
                if !sub.is_empty() {
                    builder.subtitles.push(sub);
                }
            } else {
                builder.content_lines.push(trimmed.to_string());
            }
        }

        scenes.push(builder.build());
    }

    Ok(vec![Episode {
        episode_index: 0,
        title: "剧本".into(),
        scenes,
        ..Default::default()
    }])
}

fn split_by_scene_headers(text: &str) -> Vec<String> {
    let mut blocks: Vec<String> = Vec::new();
    let mut current: Vec<&str> = Vec::new();

    for line in text.lines() {
        let trimmed = line.trim();
        if is_scene_header_line(trimmed) && !current.is_empty() {
            blocks.push(current.join("\n"));
            current = vec![line];
        } else {
            current.push(line);
        }
    }
    if !current.is_empty() {
        blocks.push(current.join("\n"));
    }

    blocks
}

// ============================================================================
// 对白解析
// ============================================================================

/// 尝试解析对白行。
///
/// 支持的格式：
/// - `角色名：台词内容`
/// - `角色名: 台词内容`
/// - `角色名（动作）：台词`
/// - `CharacterName: dialogue text`
fn try_parse_dialogue(line: &str) -> Option<DialogueLine> {
    let trimmed = line.trim();

    // 跳过太短的行
    if trimmed.chars().count() < 3 {
        return None;
    }

    // 查找第一个冒号（全角或半角），使用 char_indices 保证 UTF-8 边界安全
    let colon_info = trimmed
        .char_indices()
        .find(|(_, c)| *c == '：' || *c == ':')?;

    // 冒号不能是第一个字符
    if colon_info.0 == 0 {
        return None;
    }

    let colon_byte = colon_info.0;
    let colon_char = colon_info.1;
    let name_part = trimmed[..colon_byte].trim();
    // 跳过冒号字符自身（可能是 1 或 3 字节）
    let rest_start = colon_byte + colon_char.len_utf8();
    let rest = if rest_start < trimmed.len() {
        trimmed[rest_start..].trim()
    } else {
        ""
    };

    // 名字验证
    if name_part.chars().count() > 15 {
        return None;
    }
    let punct_count = name_part
        .chars()
        .filter(|c| c.is_ascii_punctuation() || *c == '。' || *c == '，')
        .count();
    if punct_count > 1 {
        return None;
    }

    // 提取括号内动作（全角括号）
    let (character, parenthetical) = extract_parenthetical(name_part);

    Some(DialogueLine {
        character: character.to_string(),
        parenthetical,
        line: rest.to_string(),
    })
}

fn extract_parenthetical(name_part: &str) -> (&str, Option<String>) {
    // 使用 char_indices 安全查找全角括号
    let open_byte = name_part
        .char_indices()
        .find(|(_, c)| *c == '（')
        .map(|(i, _)| i);

    if let Some(open) = open_byte {
        let after_open = &name_part[open + 3..]; // '（' 占 3 字节
        if let Some(close) = after_open
            .char_indices()
            .find(|(_, c)| *c == '）')
            .map(|(i, _)| i)
        {
            let name = name_part[..open].trim();
            let action = after_open[..close].trim().to_string();
            return (name, Some(action));
        }
    }
    (name_part, None)
}

// ============================================================================
// 场景头检测
// ============================================================================

/// 判断一行是否为场景头。
///
/// 场景头通常以数字编号开头，如 "1-1 日 内 沪上 张家" 或 "场景1：大厅"。
fn is_scene_header_line(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return false;
    }

    // "1-1" 格式
    if let Some(first_char) = trimmed.chars().next() {
        if first_char.is_ascii_digit() && trimmed.contains('-') {
            return true;
        }
    }

    // "场景N" 或 "第N场"
    if trimmed.starts_with("场景") || trimmed.starts_with("第") && trimmed.contains('场') {
        return true;
    }

    // "Scene N" 格式
    let lower = trimmed.to_lowercase();
    if lower.starts_with("scene ") {
        return true;
    }

    false
}

// ============================================================================
// Builder 结构
// ============================================================================

struct EpisodeBuilder {
    index: u32,
    title: String,
    scenes: Vec<SceneContent>,
}

impl EpisodeBuilder {
    fn new(index: u32, title: String) -> Self {
        Self {
            index,
            title,
            scenes: vec![],
        }
    }

    fn build(self) -> Episode {
        Episode {
            episode_index: self.index,
            title: self.title.clone(),
            raw_content: String::new(),
            scenes: self.scenes,
            ..Default::default()
        }
    }
}

struct SceneBuilder {
    scene_header: String,
    characters: Vec<String>,
    dialogues: Vec<DialogueLine>,
    actions: Vec<String>,
    subtitles: Vec<String>,
    content_lines: Vec<String>,
}

impl SceneBuilder {
    fn new(header: String) -> Self {
        Self {
            scene_header: header,
            characters: vec![],
            dialogues: vec![],
            actions: vec![],
            subtitles: vec![],
            content_lines: vec![],
        }
    }

    fn build(self) -> SceneContent {
        let content = self.content_lines.join("\n");
        SceneContent {
            scene_header: self.scene_header,
            characters: self.characters,
            content,
            dialogues: self.dialogues,
            actions: self.actions,
            subtitles: self.subtitles,
            weather: None,
            time_of_day: None,
        }
    }
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_dialogue_chinese() {
        let d = try_parse_dialogue("张三：你来了。").unwrap();
        assert_eq!(d.character, "张三");
        assert_eq!(d.line, "你来了。");
        assert!(d.parenthetical.is_none());
    }

    #[test]
    fn parse_dialogue_with_parenthetical() {
        let d = try_parse_dialogue("张三（喝酒）：好久不见。").unwrap();
        assert_eq!(d.character, "张三");
        assert_eq!(d.parenthetical.as_deref(), Some("喝酒"));
        assert_eq!(d.line, "好久不见。");
    }

    #[test]
    fn parse_dialogue_english() {
        let d = try_parse_dialogue("Alice: Hello there!").unwrap();
        assert_eq!(d.character, "Alice");
        assert_eq!(d.line, "Hello there!");
    }

    #[test]
    fn reject_non_dialogue() {
        assert!(try_parse_dialogue("这是一段描述文字，不是对白。").is_none());
        assert!(try_parse_dialogue("OK").is_none());
    }

    #[test]
    fn detect_scene_header() {
        assert!(is_scene_header_line("1-1 日 内 沪上 张家"));
        assert!(is_scene_header_line("场景1：大厅"));
        assert!(is_scene_header_line("第3场 夜 外"));
        assert!(is_scene_header_line("Scene 1: Introduction"));
        assert!(!is_scene_header_line("张三：你好"));
        assert!(!is_scene_header_line("普通描述行"));
    }

    #[test]
    fn parse_markdown_episodes() {
        let md = "\
## 第1集 相遇

### 1-1 日 外 街头
张三：今天天气不错。
李四：是啊，去走走？

### 1-2 夜 内 酒馆
出场人物：张三, 李四
张三（倒酒）：来，干一杯！\
";
        let episodes = parse_script(md).unwrap();
        assert_eq!(episodes.len(), 1);
        assert_eq!(episodes[0].title, "第1集 相遇");
        assert_eq!(episodes[0].scenes.len(), 2);
        assert_eq!(episodes[0].scenes[0].dialogues.len(), 2);
        assert_eq!(episodes[0].scenes[1].characters.len(), 2);
    }

    #[test]
    fn parse_plain_text() {
        let text = "\
1-1 日 内 办公室
张三：这份报告你看一下。
李四：好的，我马上处理。

1-2 夜 外 街头
△张三快步走过
李四（追上）：等等我！\
";
        let episodes = parse_script(text).unwrap();
        assert_eq!(episodes.len(), 1);
        assert_eq!(episodes[0].scenes.len(), 2);
        assert_eq!(episodes[0].scenes[0].dialogues.len(), 2);
        assert_eq!(episodes[0].scenes[1].actions.len(), 1);
    }

    #[test]
    fn parse_empty_input() {
        let episodes = parse_script("").unwrap();
        assert!(episodes.is_empty());
    }
}
