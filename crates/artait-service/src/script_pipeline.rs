//! 剧本流水线服务。
//!
//! 整合剧本规范化、角色提取、AI 校准调用，
//! 形成「导入剧本 → 解析 → 角色提取 → AI 校准 → 角色库」的完整流程。

use anyhow::{Context, Result};
use artait_model::script::Episode;
use artait_model::{Character, CharacterStats, CharacterStatus};
use artait_provider::{Analyzer, ProviderContext};
use tracing::info;

use crate::character_calibrator::{self, CalibrationInput, CalibrationStrictness};
use crate::character_store::CharacterStore;
use crate::script_parser;

// ============================================================================
// 剧本规范化
// ============================================================================

/// 规范化导入的剧本文本。
///
/// 处理常见的不规范输入：
/// - 统一换行为 LF
/// - 去除 BOM 头
/// - 合并多余空行
/// - 尝试为无标题剧本补充标题行
pub fn normalize_script(raw: &str) -> String {
    let mut text = raw.to_string();

    // 去除 BOM
    if text.starts_with('\u{FEFF}') {
        text = text[3..].to_string();
    }

    // 统一换行
    text = text.replace("\r\n", "\n").replace('\r', "\n");

    // 合并多余空行（最多保留一个空行）
    let mut result = String::with_capacity(text.len());
    let mut prev_empty = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if !prev_empty {
                result.push('\n');
                prev_empty = true;
            }
        } else {
            result.push_str(line);
            result.push('\n');
            prev_empty = false;
        }
    }

    result.trim().to_string()
}

// ============================================================================
// 角色提取
// ============================================================================

/// 从解析后的剧本中提取所有角色名及出场统计。
///
/// 扫描所有集的所有场景的对白和出场人物列表。
pub fn extract_characters_from_episodes(episodes: &[Episode]) -> Vec<CharacterStats> {
    use std::collections::HashMap;

    let mut stats_map: HashMap<String, CharacterStats> = HashMap::new();

    for ep in episodes {
        let ep_idx = ep.episode_index;
        for scene in &ep.scenes {
            // 收集该场景所有角色
            let all_chars = scene.all_characters();
            for name in &all_chars {
                let entry = stats_map
                    .entry(name.to_string())
                    .or_insert_with(|| CharacterStats {
                        name: name.to_string(),
                        scene_count: 0,
                        dialogue_count: 0,
                        episodes: vec![],
                        first_episode: ep_idx,
                        last_episode: ep_idx,
                        dialogue_samples: vec![],
                        scene_samples: vec![],
                    });

                entry.scene_count += 1;
                entry.last_episode = ep_idx;

                if !entry.episodes.contains(&ep_idx) {
                    entry.episodes.push(ep_idx);
                }

                // 采样场景头
                if entry.scene_samples.len() < 3 {
                    entry.scene_samples.push(scene.scene_header.clone());
                }
            }

            // 对白计数
            for dialogue in &scene.dialogues {
                if let Some(entry) = stats_map.get_mut(&dialogue.character) {
                    entry.dialogue_count += 1;
                    if entry.dialogue_samples.len() < 3 {
                        entry.dialogue_samples.push(dialogue.line.clone());
                    }
                }
            }
        }
    }

    let mut stats: Vec<CharacterStats> = stats_map.into_values().collect();
    stats.sort_by(|a, b| b.scene_count.cmp(&a.scene_count));
    stats
}

// ============================================================================
// 完整流水线
// ============================================================================

/// 运行完整流水线：导入 → 解析 → 提取角色 → AI 校准 → 存入角色库。
pub async fn import_and_calibrate(
    raw_text: &str,
    store: &mut CharacterStore,
    analyzer: &dyn Analyzer,
    pctx: &ProviderContext,
    project_id: Option<&str>,
) -> Result<Vec<String>> {
    // 1. 规范化
    let normalized = normalize_script(raw_text);

    // 2. 解析
    let episodes = script_parser::parse_script(&normalized).context("剧本解析失败")?;
    info!(episodes = episodes.len(), "剧本解析完成");

    // 3. 提取角色统计
    let stats = extract_characters_from_episodes(&episodes);
    info!(count = stats.len(), "提取角色");

    if stats.is_empty() {
        return Ok(vec![]);
    }

    // 4. AI 校准
    let input = CalibrationInput {
        stats: stats.clone(),
        strictness: CalibrationStrictness::Normal,
        batch_size: 30,
        script_context: Some(normalized.chars().take(2000).collect()),
    };

    let result = character_calibrator::calibrate_characters(analyzer, pctx, input)
        .await
        .context("角色校准失败")?;

    // 5. 存入角色库
    let mut created_ids = Vec::new();
    let pid = project_id.map(|s| s.to_string());

    for calibrated in &result.characters {
        let id = uuid::Uuid::new_v4().to_string();
        let mut character = Character::new(id, calibrated.name.clone());
        character.gender = calibrated.gender.clone();
        character.age = calibrated.age.clone();
        character.role = calibrated.role.clone();
        character.relationships = calibrated.relationships.clone();
        character.visual_prompt_en = calibrated.visual_prompt_en.clone();
        character.visual_prompt_zh = calibrated.visual_prompt_zh.clone();
        character.identity_anchors = calibrated.identity_anchors.clone();
        character.negative_prompt = calibrated.negative_prompt.clone();
        character.project_id = pid.clone();
        character.status = CharacterStatus::Linked;
        character.description = Some(format!(
            "重要度: {} · 出场: {} 次",
            calibrated.importance.display_name(),
            calibrated.appearance_count
        ));

        match store.create_character(character) {
            Ok(id) => {
                created_ids.push(id);
            }
            Err(e) => {
                tracing::warn!(name = %calibrated.name, error = %e, "存储角色失败");
            }
        }
    }

    info!(created = created_ids.len(), "角色入库完成");
    Ok(created_ids)
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_strips_bom() {
        let input = "\u{FEFF}# 剧本标题\n\n张三：你好。\n";
        let result = normalize_script(input);
        assert!(!result.starts_with('\u{FEFF}'));
        assert!(result.starts_with("# 剧本标题"));
    }

    #[test]
    fn normalize_collapses_blank_lines() {
        let input = "第一行\n\n\n\n第二行\n\n\n第三行";
        let result = normalize_script(input);
        let blank_count = result.lines().filter(|l| l.trim().is_empty()).count();
        assert_eq!(blank_count, 2); // 最多一个空行分隔
    }

    #[test]
    fn normalize_unifies_line_endings() {
        let input = "line1\r\nline2\rline3\nline4";
        let result = normalize_script(input);
        assert!(result.lines().count() == 4);
        assert!(!result.contains('\r'));
    }

    #[test]
    fn extract_characters_from_dialogues() {
        let ep = Episode {
            episode_index: 0,
            title: "测试".into(),
            scenes: vec![artait_model::script::SceneContent {
                scene_header: "1-1 日 内".into(),
                dialogues: vec![
                    artait_model::script::DialogueLine {
                        character: "张三".into(),
                        parenthetical: None,
                        line: "你好".into(),
                    },
                    artait_model::script::DialogueLine {
                        character: "李四".into(),
                        parenthetical: None,
                        line: "来了".into(),
                    },
                ],
                ..Default::default()
            }],
            ..Default::default()
        };

        let stats = extract_characters_from_episodes(&[ep]);
        assert_eq!(stats.len(), 2);
        assert!(
            stats
                .iter()
                .find(|s| s.name == "张三")
                .unwrap()
                .dialogue_count
                == 1
        );
    }
}
