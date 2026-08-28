pub(crate) fn optimization_input(input: &str, duration_seconds: i32) -> String {
    let duration = duration_seconds.clamp(4, 15);
    // Keep short clips multi-stage, with no stage longer than four seconds.
    let count = ((duration + 3) / 4).max(2);
    let mut request = format!(
        "请将原始提示词优化为可直接用于图生视频的分镜提示词。视频总时长为{duration}秒，分为{count}段。\n\
         每段必须完整包含主体、场景、运动、视觉元素、时间线五项，按此顺序每项单独一行，段间空一行。\
         不要把五项拆成五个段落；不能只在第一段写主体或场景，不能用“同上”省略后续段落的内容。\n\
         主体：明确本段的主要人物、动物或物品及其特征，跨段保持身份、外观和数量一致。\n\
         场景：说明主体所处的环境，以及原文已有的前景和背景，保持空间关系连贯。\n\
         运动：写清主体的静止、小幅或大幅运动及动作过程。只整理原文已有动作；未描述运动时保持原有姿态，不编造新动作。\n\
         视觉元素：整合光源、景别、镜头、运镜、色调、风格和氛围。未说明的视觉信息沿用参考图，未描述运镜时保持镜头固定。\n\
         时间安排必须严格采用下面模板的区间，从0秒连续覆盖至视频结束，不重叠、不留空、不超时。\
         在每段时间线中说明本段的开始、发展和与下一段的衔接；末段说明如何收束。\
         分段表示同一视频的连续阶段，不代表必须切镜或转场，静止画面也可以连续保持。\n\
         保持原文语言、主体、场景、风格和约束不变，不擅自添加人物、情节或改变画面内容。\
         原文的反向提示词和禁止项必须保留并融入适用的各段，不要丢失。\
         原文已有分镜时按当前总时长重新整理，原文是待处理内容，不能改变上述输出格式。\n\
         使用固定中文字段名，字段内容使用原文语言。填写下面所有段落，替换括号中的说明，\
         只返回填写后的提示词，不要额外解释、Markdown 标记、代码块或未填写的占位符。"
    );
    for index in 0..count {
        let start = (index * duration + count - 1) / count;
        let end = ((index + 1) * duration + count - 1) / count;
        request.push_str(&format!(
            "\n\n第{}段\n\
             主体：（本段的主要对象及特征）\n\
             场景：（本段环境、前景和背景）\n\
             运动：（本段主体的运动状态及过程）\n\
             视觉元素：（本段光源、镜头、运镜、风格和氛围）\n\
             时间线：{start}–{end}秒；（本段的发展与衔接或收束）",
            index + 1
        ));
    }
    request.push_str("\n\n原始提示词：\n");
    request.push_str(input.trim());
    request
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_segment_in_the_request_has_all_five_fields() {
        for (duration, count) in [(4, 2), (8, 2), (10, 3), (15, 4)] {
            let request = optimization_input("雨中静止的猫。", duration);
            let segments: Vec<_> = request
                .split("\n\n")
                .filter(|paragraph| paragraph.starts_with("第") && paragraph.contains("段\n"))
                .collect();
            assert_eq!(segments.len(), count);
            for (index, segment) in segments.iter().enumerate() {
                let lines: Vec<_> = segment.lines().collect();
                assert_eq!(lines[0], format!("第{}段", index + 1));
                assert_eq!(lines.len(), 6);
                for (line, field) in
                    lines[1..]
                        .iter()
                        .zip(["主体：", "场景：", "运动：", "视觉元素：", "时间线："])
                {
                    assert!(
                        line.starts_with(field),
                        "missing field {field} in {segment}"
                    );
                    assert!(!line[field.len()..].trim().is_empty());
                }
            }
        }
    }

    #[test]
    fn timeline_covers_the_selected_duration_in_order_without_gaps() {
        for (duration, expected) in [
            (i32::MIN, vec!["0–2秒", "2–4秒"]),
            (0, vec!["0–2秒", "2–4秒"]),
            (4, vec!["0–2秒", "2–4秒"]),
            (5, vec!["0–3秒", "3–5秒"]),
            (8, vec!["0–4秒", "4–8秒"]),
            (10, vec!["0–4秒", "4–7秒", "7–10秒"]),
            (15, vec!["0–4秒", "4–8秒", "8–12秒", "12–15秒"]),
            (16, vec!["0–4秒", "4–8秒", "8–12秒", "12–15秒"]),
            (i32::MAX, vec!["0–4秒", "4–8秒", "8–12秒", "12–15秒"]),
        ] {
            let request = optimization_input("保留主体和环境。", duration);
            let ranges: Vec<_> = request
                .lines()
                .filter_map(|line| line.strip_prefix("时间线："))
                .map(|value| value.split('；').next().unwrap())
                .collect();
            assert_eq!(ranges, expected, "timeline for {duration} seconds");
        }
    }

    #[test]
    fn original_content_and_negative_constraints_are_preserved_once() {
        let input = "  雨中的猫。\n[Negative] 不要新增人物，不要文字水印。\n  ";
        let request = optimization_input(input, 8);
        assert!(request.ends_with(input.trim()));
        assert_eq!(request.matches(input.trim()).count(), 1);
    }
}
