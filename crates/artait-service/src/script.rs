//! 动画脚本生成服务：读取文档 + 调用 Analyzer + 落盘 Markdown。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use artait_model::scene::{Scene, SceneImportance, SceneStatus};
use artait_model::{Character, CharacterStats, CharacterStatus, ReferenceImage};
use artait_provider::{
    request::{AnalysisRequest, AnalysisResponseFormat},
    Analyzer, ProviderContext,
};
use serde_json::Value;
use uuid::Uuid;

use crate::{script_parser, script_pipeline};

/// 一个分镜包：连续镜头的 Markdown 片段。
#[derive(Debug, Clone)]
pub struct StoryboardPackage {
    pub index: usize,
    pub label: String,
    pub shot_count: usize,
    pub markdown: String,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ScriptParseReport {
    pub episode_count: usize,
    pub scene_count: usize,
    pub character_count: usize,
    pub dialogue_count: usize,
    pub summary: String,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ScriptSceneSummary {
    pub id: String,
    pub episode: String,
    pub label: String,
    pub characters: String,
    pub action_preview: String,
    pub dialogue_count: usize,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ScriptCharacterSummary {
    pub name: String,
    pub role: String,
    pub scene_count: usize,
    pub dialogue_count: usize,
    pub sample: String,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ScriptStructureSummary {
    pub scenes: Vec<ScriptSceneSummary>,
    pub characters: Vec<ScriptCharacterSummary>,
}

/// 从脚本 Markdown 拆分出分镜包。
///
/// 按 `## 镜头 N` 二级标题分组；每个包的大小 = `shots_per_package`（默认 3）。
pub fn split_storyboard_packages(
    markdown: &str,
    shots_per_package: usize,
) -> Vec<StoryboardPackage> {
    let per = shots_per_package.max(1);
    // 按行扫描，找到镜头标题或标准剧本场景头。
    let mut shots: Vec<(usize, Vec<&str>)> = Vec::new();
    let mut cur_shot: Option<Vec<&str>> = None;
    let mut shot_no = 0usize;

    for line in markdown.lines() {
        let trimmed = line.trim();
        let is_shot_header = is_storyboard_unit_header(trimmed);

        if is_shot_header {
            if let Some(prev) = cur_shot.take() {
                shots.push((shot_no, prev));
                shot_no += 1;
            }
            cur_shot = Some(vec![line]);
        } else if let Some(ref mut cur) = cur_shot {
            cur.push(line);
        }
    }
    if let Some(last) = cur_shot {
        shots.push((shot_no, last));
    }

    if shots.is_empty() {
        // 没找到镜头标题 → 把全文作为一个包
        return vec![StoryboardPackage {
            index: 0,
            label: "全文".into(),
            shot_count: 1,
            markdown: markdown.to_string(),
        }];
    }

    // 按 per 分组
    shots
        .chunks(per)
        .enumerate()
        .map(|(pkg_idx, chunk)| {
            let first = chunk.first().map(|(n, _)| *n).unwrap_or(0);
            let last = chunk.last().map(|(n, _)| *n).unwrap_or(first);
            let label = if first == last {
                format!("镜头 {}", first + 1)
            } else {
                format!("镜头 {}–{}", first + 1, last + 1)
            };
            let md = chunk
                .iter()
                .flat_map(|(_, lines)| lines.iter().copied())
                .collect::<Vec<_>>()
                .join("\n");
            StoryboardPackage {
                index: pkg_idx,
                label,
                shot_count: chunk.len(),
                markdown: md,
            }
        })
        .collect()
}

fn is_storyboard_unit_header(trimmed: &str) -> bool {
    let normalized = trimmed
        .trim_start_matches('#')
        .trim()
        .trim_matches('*')
        .trim();

    normalized.starts_with("镜头") || normalized.starts_with("Shot") || is_scene_heading(normalized)
}

fn is_scene_heading(text: &str) -> bool {
    let mut chars = text.chars().peekable();
    let mut first_digits = 0usize;
    while chars.peek().is_some_and(|c| c.is_ascii_digit()) {
        first_digits += 1;
        chars.next();
    }
    if first_digits == 0 || chars.next() != Some('-') {
        return false;
    }
    let mut second_digits = 0usize;
    while chars.peek().is_some_and(|c| c.is_ascii_digit()) {
        second_digits += 1;
        chars.next();
    }
    second_digits > 0
}

/// 把 Markdown 转换为可在 Slint TextInput / Text 里渲染的纯文本（段落换行）。
/// 后续第二刀再改为真正的 richtext 渲染。
pub fn markdown_to_plain(md: &str) -> String {
    use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
    let mut out = String::with_capacity(md.len());
    let mut in_code_block = false;
    let parser = Parser::new_ext(md, Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH);
    for event in parser {
        match event {
            Event::Text(t) => out.push_str(&t),
            Event::Code(c) => {
                out.push('`');
                out.push_str(&c);
                out.push('`');
            }
            Event::SoftBreak | Event::HardBreak => out.push('\n'),
            Event::Start(Tag::Heading { .. }) => out.push_str("\n\n"),
            Event::End(TagEnd::Heading(_)) => out.push('\n'),
            Event::Start(Tag::Paragraph) => out.push_str("\n\n"),
            Event::End(TagEnd::Paragraph) => {}
            Event::Start(Tag::CodeBlock(_)) => {
                in_code_block = true;
                out.push_str("\n```\n");
            }
            Event::End(TagEnd::CodeBlock) => {
                in_code_block = false;
                out.push_str("\n```\n");
            }
            Event::Start(Tag::Item) => out.push_str("\n• "),
            Event::Rule => out.push_str("\n---\n"),
            _ => {}
        }
        let _ = in_code_block; // suppress unused
    }
    // 去除开头多余的换行
    out.trim_start_matches('\n').to_string()
}

pub fn screenplay_format_example() -> &'static str {
    r#"**《未命名短剧》**

**大纲：**
一句话概括核心故事、主角目标和主要冲突。

**人物小传：**
主角（年龄）：身份，性格特征，核心欲望
对手（年龄）：身份，阻碍主角的方式

---

**第一集：标题**

---

**1-1 日 内 地点**
人物：主角、对手

△动作/环境描写，以可拍摄画面为主。

主角：（情绪或动作）对白内容。
对手：对白内容。

【字幕：时间或地点信息】

**1-2 夜 外 地点**
人物：主角

△下一场动作描述。
"#
}

pub fn build_importable_markdown(raw: &str) -> String {
    let normalized = script_pipeline::normalize_script(raw);
    if normalized.is_empty() {
        return String::new();
    }
    if normalized.contains("**1-")
        || normalized.contains("\n1-")
        || normalized.contains("### ")
        || normalized.contains("## 镜头")
    {
        normalized
    } else {
        format!("**《导入剧本》**\n\n---\n\n**第一集：默认**\n\n---\n\n{normalized}")
    }
}

pub fn save_imported_script(raw: &str, output_dir: &Path) -> Result<PathBuf> {
    let markdown = build_importable_markdown(raw);
    anyhow::ensure!(!markdown.trim().is_empty(), "剧本内容为空");
    std::fs::create_dir_all(output_dir)
        .with_context(|| format!("创建目录 {} 失败", output_dir.display()))?;
    let ts = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let dest = output_dir.join(format!("imported-script-{ts}.md"));
    std::fs::write(&dest, markdown).with_context(|| format!("写入 {} 失败", dest.display()))?;
    Ok(dest)
}

pub fn save_script(raw: &str, selected_path: Option<&Path>, output_dir: &Path) -> Result<PathBuf> {
    let markdown = script_pipeline::normalize_script(raw);
    anyhow::ensure!(!markdown.trim().is_empty(), "剧本内容为空");
    std::fs::create_dir_all(output_dir)
        .with_context(|| format!("创建目录 {} 失败", output_dir.display()))?;
    let dest = selected_path
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("md"))
        .map(Path::to_path_buf)
        .unwrap_or_else(|| {
            let ts = chrono::Local::now().format("%Y%m%d-%H%M%S");
            output_dir.join(format!("edited-script-{ts}.md"))
        });
    std::fs::write(&dest, markdown).with_context(|| format!("写入 {} 失败", dest.display()))?;
    Ok(dest)
}

pub fn analyze_script(raw: &str) -> Result<ScriptParseReport> {
    let normalized = script_pipeline::normalize_script(raw);
    let episodes = script_parser::parse_script(&normalized).context("剧本解析失败")?;
    let character_stats = script_pipeline::extract_characters_from_episodes(&episodes);
    let episode_count = episodes.len();
    let scene_count: usize = episodes.iter().map(|ep| ep.scenes.len()).sum();
    let dialogue_count: usize = episodes
        .iter()
        .flat_map(|ep| ep.scenes.iter())
        .map(|scene| scene.dialogues.len())
        .sum();
    let character_count = character_stats.len();
    let summary = format!(
        "解析完成：{} 集 · {} 场 · {} 个角色 · {} 条对白",
        episode_count, scene_count, character_count, dialogue_count
    );
    Ok(ScriptParseReport {
        episode_count,
        scene_count,
        character_count,
        dialogue_count,
        summary,
    })
}

pub fn summarize_script_structure(raw: &str) -> Result<ScriptStructureSummary> {
    let normalized = script_pipeline::normalize_script(raw);
    let episodes = script_parser::parse_script(&normalized).context("剧本解析失败")?;
    let character_stats = script_pipeline::extract_characters_from_episodes(&episodes);
    let mut scenes = Vec::new();
    for ep in &episodes {
        for (idx, scene) in ep.scenes.iter().enumerate() {
            let label = if scene.scene_header.trim().is_empty() {
                format!("场景 {}", idx + 1)
            } else {
                scene.scene_header.clone()
            };
            let action_preview = scene
                .actions
                .first()
                .cloned()
                .or_else(|| {
                    scene
                        .content
                        .lines()
                        .find(|l| !l.trim().is_empty())
                        .map(str::to_string)
                })
                .or_else(|| {
                    scene
                        .dialogues
                        .first()
                        .map(|d| format!("{}：{}", d.character, d.line))
                })
                .unwrap_or_else(|| "暂无动作描述".into());
            scenes.push(ScriptSceneSummary {
                id: format!("ep{}-sc{}", ep.episode_index + 1, idx + 1),
                episode: ep.title.clone(),
                label,
                characters: scene.all_characters().join("、"),
                action_preview: action_preview.chars().take(120).collect(),
                dialogue_count: scene.dialogues.len(),
            });
        }
    }
    let characters = character_stats
        .into_iter()
        .map(|stats| ScriptCharacterSummary {
            role: if stats.scene_count >= 5 {
                "主要角色".into()
            } else if stats.scene_count >= 2 {
                "配角".into()
            } else {
                "临时角色".into()
            },
            sample: stats.dialogue_samples.first().cloned().unwrap_or_default(),
            name: stats.name,
            scene_count: stats.scene_count as usize,
            dialogue_count: stats.dialogue_count as usize,
        })
        .collect();
    Ok(ScriptStructureSummary { scenes, characters })
}

pub fn extract_character_drafts(raw: &str, project_id: Option<&str>) -> Result<Vec<Character>> {
    let normalized = script_pipeline::normalize_script(raw);
    let episodes = script_parser::parse_script(&normalized).context("剧本解析失败")?;
    let stats = script_pipeline::extract_characters_from_episodes(&episodes);
    Ok(stats
        .into_iter()
        .map(|s| character_from_stats(s, project_id))
        .collect())
}

pub fn extract_scene_drafts(raw: &str, project_id: Option<&str>) -> Result<Vec<Scene>> {
    let normalized = script_pipeline::normalize_script(raw);
    let episodes = script_parser::parse_script(&normalized).context("剧本解析失败")?;
    let mut scenes = Vec::new();
    for ep in &episodes {
        for (idx, content) in ep.scenes.iter().enumerate() {
            let parsed = artait_model::script::parse_scene_header(&content.scene_header);
            let name = parsed
                .location
                .clone()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| {
                    if content.scene_header.trim().is_empty() {
                        format!("第 {} 集场景 {}", ep.episode_index + 1, idx + 1)
                    } else {
                        content.scene_header.clone()
                    }
                });
            let mut scene = Scene::new(Uuid::new_v4().to_string(), name);
            scene.location = parsed
                .location
                .unwrap_or_else(|| content.scene_header.clone());
            scene.time_of_day = parsed.time_of_day.or_else(|| content.time_of_day.clone());
            scene.atmosphere = Some(scene_atmosphere_hint(content));
            scene.visual_prompt_zh = Some(scene_visual_prompt(content));
            scene.project_id = project_id.map(str::to_string);
            scene.status = SceneStatus::Linked;
            scene.linked_episode_id = Some(format!("episode-{}", ep.episode_index + 1));
            scene.episode_numbers = vec![ep.episode_index + 1];
            scene.appearance_count = 1;
            scene.importance = Some(SceneImportance::Secondary);
            scene.tags = content
                .all_characters()
                .into_iter()
                .take(4)
                .map(str::to_string)
                .collect();
            scenes.push(scene);
        }
    }
    Ok(scenes)
}

pub async fn calibrate_scene_drafts(
    analyzer: &dyn Analyzer,
    pctx: &ProviderContext,
    raw: &str,
    project_id: Option<&str>,
) -> Result<Vec<Scene>> {
    let mut scenes = extract_scene_drafts(raw, project_id)?;
    anyhow::ensure!(!scenes.is_empty(), "剧本中没有可校准的场景");

    let req = AnalysisRequest {
        system_prompt: Some(scene_calibration_system_prompt()),
        user_prompt: build_scene_calibration_user_prompt(&scenes, raw),
        images: vec![],
        model: None,
        response_format: AnalysisResponseFormat::Json,
    };

    let output = analyzer
        .analyze(req, pctx)
        .await
        .map_err(|e| anyhow::anyhow!("AI 场景校准失败: {e}"))?;
    let calibrated = parse_scene_calibration_response(&output.text)
        .with_context(|| "解析 AI 场景校准结果失败")?;
    merge_scene_calibration(&mut scenes, calibrated);
    Ok(scenes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_shots_into_packages() {
        let md = "# 脚本\n## 镜头 1\n内容1\n## 镜头 2\n内容2\n## 镜头 3\n内容3\n";
        let pkgs = split_storyboard_packages(md, 2);
        assert_eq!(pkgs.len(), 2);
        assert_eq!(pkgs[0].shot_count, 2);
        assert_eq!(pkgs[1].shot_count, 1);
    }

    #[test]
    fn splits_standard_scene_headings_into_packages() {
        let md = "**第一集：测试**\n**1-1 日 内 客厅**\n△动作1\n张三：对白1\n**1-2 夜 外 街道**\n△动作2\n李四：对白2\n**1-3 日 外 山路**\n△动作3\n";
        let pkgs = split_storyboard_packages(md, 2);
        assert_eq!(pkgs.len(), 2);
        assert_eq!(pkgs[0].label, "镜头 1–2");
        assert_eq!(pkgs[0].shot_count, 2);
        assert_eq!(pkgs[1].label, "镜头 3");
        assert_eq!(pkgs[1].shot_count, 1);
    }

    #[test]
    fn no_shots_returns_single_package() {
        let md = "# 无镜头标题\n只是一段文字";
        let pkgs = split_storyboard_packages(md, 3);
        assert_eq!(pkgs.len(), 1);
        assert_eq!(pkgs[0].label, "全文");
    }

    #[test]
    fn markdown_to_plain_preserves_text() {
        let md = "# 标题\n\n段落文字\n\n- 列表项";
        let out = markdown_to_plain(md);
        assert!(out.contains("标题"));
        assert!(out.contains("段落文字"));
        assert!(out.contains("列表项"));
    }

    #[test]
    fn parses_scene_calibration_response_from_json_block() {
        let json = r#"```json
{
  "scenes": [
    {
      "index": 0,
      "name": "雨夜球场",
      "location": "城市街头篮球场",
      "visual_prompt_zh": "雨夜中的街头篮球场，冷暖对比光",
      "key_props": ["篮球", "铁丝网"],
      "importance": "main"
    }
  ]
}
```"#;
        let patches = parse_scene_calibration_response(json).unwrap();
        assert_eq!(patches.len(), 1);
        assert_eq!(patches[0].index, 0);
        assert_eq!(patches[0].name.as_deref(), Some("雨夜球场"));
        assert_eq!(patches[0].key_props, vec!["篮球", "铁丝网"]);
        assert_eq!(patches[0].importance, Some(SceneImportance::Main));
    }

    #[test]
    fn merge_scene_calibration_updates_visual_fields() {
        let mut scene = Scene::new("s1".into(), "旧场景".into());
        scene.location = "旧地点".into();
        scene.visual_prompt_zh = Some("旧提示词".into());
        let patch = SceneCalibrationPatch {
            index: 0,
            name: Some("校准场景".into()),
            location: Some("校准地点".into()),
            atmosphere: Some("紧张、潮湿、霓虹反光".into()),
            visual_prompt_zh: Some("校准后的中文视觉提示词".into()),
            lighting_design: Some("低角度逆光和雨水反射".into()),
            key_props: vec!["篮球".into()],
            importance: Some(SceneImportance::Main),
            ..Default::default()
        };
        let mut scenes = vec![scene];
        merge_scene_calibration(&mut scenes, vec![patch]);
        assert_eq!(scenes[0].name, "校准场景");
        assert_eq!(scenes[0].location, "校准地点");
        assert_eq!(
            scenes[0].atmosphere.as_deref(),
            Some("紧张、潮湿、霓虹反光")
        );
        assert_eq!(
            scenes[0].visual_prompt_zh.as_deref(),
            Some("校准后的中文视觉提示词")
        );
        assert_eq!(
            scenes[0].lighting_design.as_deref(),
            Some("低角度逆光和雨水反射")
        );
        assert_eq!(scenes[0].key_props, vec!["篮球"]);
        assert_eq!(scenes[0].importance, Some(SceneImportance::Main));
        assert_eq!(scenes[0].status, SceneStatus::Linked);
    }
}

/// 读取 .txt / .md 文件内容（不支持 pdf/docx）。
pub fn read_doc(path: &Path) -> Result<String> {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "txt" | "md" => {
            let s = std::fs::read_to_string(path)
                .with_context(|| format!("读取 {} 失败", path.display()))?;
            Ok(s)
        }
        other => anyhow::bail!("不支持 .{other} 格式，请转换为 .txt 或 .md"),
    }
}

/// 构建系统提示词。
fn system_prompt() -> String {
    "你是专业短剧编剧和分镜前期导演。根据用户提供的故事主题和参考资料，生成一份可导入生产流水线的标准剧本。\
     必须严格遵守以下格式：\
     1. 顶部包含 `**《剧名》**`、`**大纲：**`、`**人物小传：**`；\
     2. 每集使用 `**第一集：标题**` 这类标题；\
     3. 每场使用 `**1-1 日 内 地点**`、`**1-2 夜 外 地点**` 这种场景头；\
     4. 出场人物行使用 `人物：角色A、角色B`；\
     5. 动作/环境描写以 `△` 开头；\
     6. 对白使用 `角色名：台词` 或 `角色名：（动作）台词`；\
     7. 字幕/转场使用 `【字幕：内容】`；\
     8. 每场都要可拍摄，包含明确动作、镜头画面和必要对白；\
     9. 直接输出 Markdown，不要解释。"
        .into()
}

/// 把文档内容 + 图片引用组装成用户提示词。
fn build_user_prompt(theme: &str, docs: &[(PathBuf, String)]) -> String {
    let mut parts = Vec::with_capacity(docs.len() + 1);
    parts.push(format!("## 故事主题\n{theme}"));
    for (path, content) in docs {
        let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("doc");
        parts.push(format!("## 参考文档：{name}\n{content}"));
    }
    parts.join("\n\n---\n\n")
}

fn character_from_stats(stats: CharacterStats, project_id: Option<&str>) -> Character {
    let mut character = Character::new(Uuid::new_v4().to_string(), stats.name.clone());
    character.project_id = project_id.map(str::to_string);
    character.status = CharacterStatus::Linked;
    character.description = Some(format!(
        "从剧本解析：出场 {} 场，对白 {} 条",
        stats.scene_count, stats.dialogue_count
    ));
    character.role = Some(if stats.scene_count >= 5 {
        "主要角色".into()
    } else if stats.scene_count >= 2 {
        "配角".into()
    } else {
        "临时角色".into()
    });
    character.tags = vec!["剧本导入".into()];
    if !stats.dialogue_samples.is_empty() {
        character.personality = Some(format!("对白样本：{}", stats.dialogue_samples.join(" / ")));
    }
    character
}

fn scene_atmosphere_hint(scene: &artait_model::script::SceneContent) -> String {
    if let Some(subtitle) = scene.subtitles.first() {
        return subtitle.clone();
    }
    if let Some(action) = scene.actions.first() {
        return action.chars().take(80).collect();
    }
    "待校准".into()
}

fn scene_visual_prompt(scene: &artait_model::script::SceneContent) -> String {
    let mut parts = Vec::new();
    if !scene.scene_header.trim().is_empty() {
        parts.push(scene.scene_header.clone());
    }
    if !scene.actions.is_empty() {
        parts.push(
            scene
                .actions
                .iter()
                .take(3)
                .cloned()
                .collect::<Vec<_>>()
                .join("；"),
        );
    }
    if !scene.characters.is_empty() {
        parts.push(format!("出场人物：{}", scene.characters.join("、")));
    }
    if parts.is_empty() {
        scene.content.chars().take(160).collect()
    } else {
        parts.join("。")
    }
}

#[derive(Debug, Clone, Default)]
struct SceneCalibrationPatch {
    index: usize,
    name: Option<String>,
    location: Option<String>,
    time_of_day: Option<String>,
    atmosphere: Option<String>,
    visual_prompt_zh: Option<String>,
    visual_prompt_en: Option<String>,
    architecture_style: Option<String>,
    lighting_design: Option<String>,
    color_palette: Option<String>,
    key_props: Vec<String>,
    spatial_layout: Option<String>,
    era_details: Option<String>,
    tags: Vec<String>,
    notes: Option<String>,
    importance: Option<SceneImportance>,
}

fn scene_calibration_system_prompt() -> String {
    r#"你是影视前期美术指导，负责把剧本中抽取出的场景草稿校准为可用于场景库和生图的一致视觉设定。

要求：
1. 不新增、删除或重排场景，只按输入 index 返回每个场景的增强结果。
2. 补充环境、氛围、光影、色彩、空间布局、关键道具、时代质感。
3. visual_prompt_zh 要可直接用于中文生图提示词，包含地点、时间、氛围、构图层次、光影、材质和关键道具。
4. visual_prompt_en 可为空；如果输出英文，必须是同一画面的简洁英文提示词。
5. 返回纯 JSON，不要 Markdown，不要解释。

JSON 结构：
{
  "scenes": [
    {
      "index": 0,
      "name": "场景名称",
      "location": "地点",
      "time_of_day": "日/夜/晨/暮",
      "atmosphere": "氛围",
      "visual_prompt_zh": "中文视觉提示词",
      "visual_prompt_en": "English visual prompt",
      "architecture_style": "建筑/空间风格",
      "lighting_design": "光影设计",
      "color_palette": "色彩基调",
      "key_props": ["道具1", "道具2"],
      "spatial_layout": "空间布局",
      "era_details": "时代细节",
      "tags": ["标签1", "标签2"],
      "notes": "校准说明",
      "importance": "main|secondary|transition"
    }
  ]
}"#
        .into()
}

fn build_scene_calibration_user_prompt(scenes: &[Scene], raw: &str) -> String {
    let mut out = String::new();
    out.push_str("## 剧本片段\n");
    out.push_str(&raw.chars().take(6000).collect::<String>());
    out.push_str("\n\n## 待校准场景\n");
    for (idx, scene) in scenes.iter().enumerate() {
        out.push_str(&format!(
            "\n- index: {idx}\n  name: {}\n  location: {}\n  time_of_day: {}\n  atmosphere: {}\n  visual_prompt_zh: {}\n  tags: {}\n",
            scene.name,
            scene.location,
            scene.time_of_day.clone().unwrap_or_default(),
            scene.atmosphere.clone().unwrap_or_default(),
            scene.visual_prompt_zh.clone().unwrap_or_default(),
            scene.tags.join("、")
        ));
    }
    out
}

fn parse_scene_calibration_response(text: &str) -> Result<Vec<SceneCalibrationPatch>> {
    let json = extract_json_block(text);
    let value: Value = serde_json::from_str(&json).context("JSON 解析失败")?;
    let arr = value
        .get("scenes")
        .and_then(Value::as_array)
        .context("缺少 scenes 数组")?;
    let mut patches = Vec::new();
    for item in arr {
        let index = item
            .get("index")
            .and_then(Value::as_u64)
            .context("场景缺少 index")? as usize;
        patches.push(SceneCalibrationPatch {
            index,
            name: json_string(item, "name"),
            location: json_string(item, "location"),
            time_of_day: json_string(item, "time_of_day"),
            atmosphere: json_string(item, "atmosphere"),
            visual_prompt_zh: json_string(item, "visual_prompt_zh"),
            visual_prompt_en: json_string(item, "visual_prompt_en"),
            architecture_style: json_string(item, "architecture_style"),
            lighting_design: json_string(item, "lighting_design"),
            color_palette: json_string(item, "color_palette"),
            key_props: json_string_array(item, "key_props"),
            spatial_layout: json_string(item, "spatial_layout"),
            era_details: json_string(item, "era_details"),
            tags: json_string_array(item, "tags"),
            notes: json_string(item, "notes"),
            importance: json_string(item, "importance").and_then(|s| match s.as_str() {
                "main" => Some(SceneImportance::Main),
                "transition" => Some(SceneImportance::Transition),
                "secondary" => Some(SceneImportance::Secondary),
                _ => None,
            }),
        });
    }
    Ok(patches)
}

fn merge_scene_calibration(scenes: &mut [Scene], patches: Vec<SceneCalibrationPatch>) {
    for patch in patches {
        let Some(scene) = scenes.get_mut(patch.index) else {
            continue;
        };
        assign_if_some(&mut scene.name, patch.name);
        assign_if_some(&mut scene.location, patch.location);
        assign_opt_if_some(&mut scene.time_of_day, patch.time_of_day);
        assign_opt_if_some(&mut scene.atmosphere, patch.atmosphere);
        assign_opt_if_some(&mut scene.visual_prompt_zh, patch.visual_prompt_zh);
        assign_opt_if_some(&mut scene.visual_prompt_en, patch.visual_prompt_en);
        assign_opt_if_some(&mut scene.architecture_style, patch.architecture_style);
        assign_opt_if_some(&mut scene.lighting_design, patch.lighting_design);
        assign_opt_if_some(&mut scene.color_palette, patch.color_palette);
        assign_opt_if_some(&mut scene.spatial_layout, patch.spatial_layout);
        assign_opt_if_some(&mut scene.era_details, patch.era_details);
        assign_opt_if_some(&mut scene.notes, patch.notes);
        if !patch.key_props.is_empty() {
            scene.key_props = patch.key_props;
        }
        if !patch.tags.is_empty() {
            scene.tags = patch.tags;
        }
        if let Some(importance) = patch.importance {
            scene.importance = Some(importance);
        }
        scene.status = SceneStatus::Linked;
        scene.updated_at = chrono::Utc::now();
    }
}

fn extract_json_block(text: &str) -> String {
    let text = text.trim();
    if let Some(start) = text.find("```json") {
        let after = &text[start + 7..];
        if let Some(end) = after.find("```") {
            return after[..end].trim().to_string();
        }
    }
    if let Some(start) = text.find("```") {
        let after = &text[start + 3..];
        if let Some(end) = after.find("```") {
            return after[..end].trim().to_string();
        }
    }
    text.to_string()
}

fn json_string(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn json_string_array(v: &Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn assign_if_some(target: &mut String, value: Option<String>) {
    if let Some(value) = value.filter(|s| !s.trim().is_empty()) {
        *target = value;
    }
}

fn assign_opt_if_some(target: &mut Option<String>, value: Option<String>) {
    if let Some(value) = value.filter(|s| !s.trim().is_empty()) {
        *target = Some(value);
    }
}

/// 生成动画脚本 Markdown 并保存到 `output_dir/<safe_title>_<timestamp>.md`。
pub async fn generate_and_save(
    analyzer: &dyn Analyzer,
    pctx: &ProviderContext,
    theme: &str,
    doc_paths: &[PathBuf],
    ref_images: &[ReferenceImage],
    output_dir: &Path,
) -> Result<PathBuf> {
    // 读取所有文档
    let mut docs: Vec<(PathBuf, String)> = Vec::new();
    for p in doc_paths {
        let content = read_doc(p).context("文档读取失败")?;
        docs.push((p.clone(), content));
    }

    let user_prompt = build_user_prompt(theme, &docs);

    let req = AnalysisRequest {
        system_prompt: Some(system_prompt()),
        user_prompt,
        images: ref_images.to_vec(),
        model: None,
        response_format: AnalysisResponseFormat::Plain,
    };

    let output = analyzer
        .analyze(req, pctx)
        .await
        .map_err(|e| anyhow::anyhow!("Analyzer 失败: {e}"))?;

    // 生成文件名：safe 标题 + 时间戳
    let safe_title = theme
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | ' '))
        .take(40)
        .collect::<String>()
        .trim()
        .replace(' ', "_");
    let ts = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let filename = format!(
        "{}-{ts}.md",
        if safe_title.is_empty() {
            "script".to_string()
        } else {
            safe_title
        }
    );

    std::fs::create_dir_all(output_dir)
        .with_context(|| format!("创建目录 {} 失败", output_dir.display()))?;
    let dest = output_dir.join(&filename);
    std::fs::write(&dest, &output.text).with_context(|| format!("写入 {} 失败", dest.display()))?;

    Ok(dest)
}

/// 扫描 `output_dir/*.md` 并返回路径列表（按 mtime 倒序）。
pub fn list_scripts(output_dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(output_dir) else {
        return Vec::new();
    };
    let mut paths: Vec<(PathBuf, std::time::SystemTime)> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("md"))
        .filter_map(|p| {
            let mtime = std::fs::metadata(&p).and_then(|m| m.modified()).ok()?;
            Some((p, mtime))
        })
        .collect();
    paths.sort_by(|a, b| b.1.cmp(&a.1));
    paths.into_iter().map(|(p, _)| p).collect()
}

/// 通过 provider 生成脚本：加载密钥 → 调用分析 → 保存。供 TaskRunner closure 使用。
pub async fn generate_script_via_provider(
    inst: &artait_model::ProviderInstance,
    theme: &str,
    doc_paths: &[PathBuf],
    output_dir: &Path,
    registry: &artait_provider::ProviderRegistry,
    http: std::sync::Arc<dyn artait_provider::HttpClient>,
    ctx: &artait_task::TaskContext,
) -> Result<PathBuf, artait_task::TaskError> {
    use crate::provider_helpers::load_provider_secret;

    ctx.progress(0.1);
    ctx.check_cancelled()
        .map_err(|_| artait_task::TaskError::Cancelled)?;

    let provider = registry.get(&inst.provider_id).ok_or_else(|| {
        artait_task::TaskError::Failed(format!("未找到 provider {}", inst.provider_id))
    })?;
    let analyzer = provider.as_analyzer().ok_or_else(|| {
        artait_task::TaskError::Failed(format!("{} 不支持文本推理", inst.provider_id))
    })?;

    let mut pctx = artait_provider::ProviderContext::with_http(
        inst.id.clone(),
        inst.provider_id.clone(),
        http,
    );
    pctx.endpoint = inst.endpoint.clone();
    pctx.extra = inst.extra.clone();
    pctx.cancellation = ctx.cancel.clone();
    pctx.secret = load_provider_secret(inst, Some(ctx))?;

    ctx.info("调用 chat/completions 生成脚本");
    ctx.progress(0.3);

    generate_and_save(analyzer, &pctx, theme, doc_paths, &[], output_dir)
        .await
        .map_err(|e| artait_task::TaskError::Failed(format!("{e}")))
}

pub async fn calibrate_scene_drafts_via_provider(
    inst: &artait_model::ProviderInstance,
    raw: &str,
    project_id: Option<&str>,
    registry: &artait_provider::ProviderRegistry,
    http: std::sync::Arc<dyn artait_provider::HttpClient>,
    ctx: &artait_task::TaskContext,
) -> Result<Vec<Scene>, artait_task::TaskError> {
    use crate::provider_helpers::load_provider_secret;

    ctx.progress(0.1);
    ctx.check_cancelled()
        .map_err(|_| artait_task::TaskError::Cancelled)?;

    let provider = registry.get(&inst.provider_id).ok_or_else(|| {
        artait_task::TaskError::Failed(format!("未找到 provider {}", inst.provider_id))
    })?;
    let analyzer = provider.as_analyzer().ok_or_else(|| {
        artait_task::TaskError::Failed(format!("{} 不支持文本推理", inst.provider_id))
    })?;

    let mut pctx = artait_provider::ProviderContext::with_http(
        inst.id.clone(),
        inst.provider_id.clone(),
        http,
    );
    pctx.endpoint = inst.endpoint.clone();
    pctx.extra = inst.extra.clone();
    pctx.cancellation = ctx.cancel.clone();
    pctx.secret = load_provider_secret(inst, Some(ctx))?;

    ctx.info("调用推理 provider 校准场景设定");
    ctx.progress(0.3);

    let scenes = calibrate_scene_drafts(analyzer, &pctx, raw, project_id)
        .await
        .map_err(|e| artait_task::TaskError::Failed(format!("{e}")))?;
    ctx.progress(0.9);
    Ok(scenes)
}
