// AI-creation prompts are independent of workbench category and composition controls.
pub(super) fn build_canvas_generation_prompt(
    prompt: &str,
    ratio: &str,
    quality: &str,
    english: bool,
) -> String {
    let dimensions = if english {
        format!("AI creation output: aspect ratio {ratio}; resolution {quality}.")
    } else {
        format!("AI创作输出参数：宽高比{ratio}；清晰度{quality}。")
    };
    let uses_workflow_specific_composition = prompt.contains("角色多向图规范：")
        || prompt.contains("Character multi-direction sheet specification:")
        || prompt.contains("场景组合规范：")
        || prompt.contains("Scene composition specification:")
        || prompt.contains("技能图标生成规范：")
        || prompt.contains("Skill icon generation specification:");
    if uses_workflow_specific_composition {
        return format!("{}\n\n{dimensions}", prompt.trim());
    }

    let rules = if english {
        "AI creation — final cutout-safe composition rules (override any conflicting framing or layout instructions above): Show every subject completely inside the canvas. Characters must show the full body from the top of the head to the soles, including all hair ornaments, hair, sleeves, skirts, capes, shoes, weapons and accessories; show complete branches, roots, wings, tails, roofs and structural parts for other subjects. Never crop, clip, hide or truncate a subject or any attached part at an image edge. If the reference is cropped, reconstruct the missing parts consistently instead of copying its crop. Preserve the requested subject count, identities, style and relative proportions. Reserve an invisible separate cell for each subject; the full silhouette including equipment, effects and shadow must occupy no more than 70% of the cell width and 80% of its height. Keep blank solid-color margins of at least 5% of the canvas short edge along all four image edges. Leave uninterrupted solid-background gutters horizontally and vertically, at least one quarter of the largest adjacent subject's width or height, measured from the outermost visible pixels, not body centers. Subjects, hair, clothing, weapons, branches, decorations, shadows and glows must never touch, overlap or connect. No shared platforms, continuous ground strips, connecting lines or scenery. Use one uniform solid-color background with no text, digits, labels, watermarks, visible grid or cell borders. Only workflow-requested localized glows are allowed, fully contained within their own cells without crossing the blank gutters. For six or more subjects use at least two rows, ordered left to right then top to bottom; fewer subjects may also wrap if needed. Zoom out and uniformly scale down subjects until every extremity, gutter and outer margin fits; do not enlarge, stretch or crop them to fill the frame. Keep the selected canvas aspect ratio and output resolution unchanged. Before finalizing, check every edge and gap and reduce subject scale again if anything is clipped or touching. Completeness and separation take priority over large subjects, filling the frame or a fixed single row."
    } else {
        "AI创作最终抠图规范（优先于前文冲突的镜头和排布要求）：所有主体必须在画布内完整展示。角色必须从头顶到脚底全身完整展示，包括发饰、头发、手臂、袖口、裙摆、披风、鞋底、武器和配饰；植物的枝叶根系、怪物的翅膀尾巴、建筑的屋顶和附属结构也必须全部保留。禁止任何主体或附属部件被画布边缘裁切、截断、遮挡或出框。参考图本身不完整时，按原设计和风格补全缺失部位，不得照搬参考图的裁切。保持要求的主体数量、身份、风格和相对比例。先为每个主体分配独立的隐形区域，包含装备、特效和阴影的完整外轮廓最多占该区域宽度的70%、高度的80%。画面四边必须各保留至少画布短边5%的纯色安全边距。横向和纵向相邻主体之间必须存在连续纯色背景空隙，宽度或高度至少为相邻较大主体对应宽度或高度的四分之一；间距从最外围像素计算，不得只按躯干中心计算。主体、头发、服装、武器、枝叶、装饰、阴影和光晕必须互不接触、互不重叠、互不连接。禁止共享底座、连续地面、连接线和场景背景。只使用单一均匀纯色背景，不得出现文字、数字、标签、水印、可见网格或区域边框。仅允许工作流明确要求的局部光晕，且必须限制在自身区域内，不得越过纯色间隔。六个及以上主体至少分成上下两行，按从左到右、从上到下排序；较少主体放不下时也应分行。必须拉远镜头并统一缩小主体，直到全部末端、间隔和四边留白都能容纳，禁止为了铺满画面而放大、拉伸或裁切。保持所选画布比例及正常2K或4K输出尺寸不变。输出前逐个检查头顶、脚底、左右末端及主体之间的空隙；发现裁切或接触时继续缩小主体并重新排布。完整性和间距优先于主体放大、铺满画面或固定单行排布。"
    };
    format!("{}\n\n{dimensions}\n\n{rules}", prompt.trim())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::prompt::{
        compose_character_multi_direction_prompt, compose_scene_composition_prompt,
        compose_skill_icon_prompt,
    };

    #[test]
    fn character_multi_direction_prompt_preserves_transparent_background_rules() {
        let prompt = compose_character_multi_direction_prompt("像素骑士", 8, "standing", false);
        let submitted = build_canvas_generation_prompt(&prompt, "4:3", "4K", false);

        assert!(submitted.contains("完全透明背景"));
        assert!(submitted.contains("2.5D俯斜视角"));
        assert!(submitted.contains("严格使用3列3行九宫格"));
        assert!(submitted.contains("中心格（第2行第2列）必须完全留空并保持透明"));
        assert!(submitted.contains("不要画网格线、边框、分隔线"));
        assert!(submitted.contains("宽高比4:3"));
        assert!(submitted.contains("清晰度4K"));
        assert!(!submitted.contains("必须使用单一纯色背景"));
        assert!(!submitted.contains("uniform solid-color background"));
        assert!(!submitted.contains("局部光晕"));
    }

    #[test]
    fn scene_composition_prompt_preserves_a_full_environment_instead_of_cutout_rules() {
        let prompt = compose_scene_composition_prompt("魔法集市", false);
        let submitted = build_canvas_generation_prompt(&prompt, "16:9", "2K", false);

        assert!(submitted.contains("同一个环境中重新构建"));
        assert!(submitted.contains("禁止输出拼贴图"));
        assert!(submitted.contains("宽高比16:9"));
        assert!(submitted.contains("清晰度2K"));
        assert!(!submitted.contains("AI创作最终抠图规范"));
        assert!(!submitted.contains("必须使用单一纯色背景"));
        assert!(!submitted.contains("禁止共享底座"));
    }

    #[test]
    fn skill_icon_prompt_preserves_its_selected_sheet_background_and_shape() {
        let prompt = compose_skill_icon_prompt("魔法技能", 12, "black", "circle", false);
        let submitted = build_canvas_generation_prompt(&prompt, "4:3", "2K", false);

        assert!(submitted.contains("恰好12个"));
        assert!(submitted.contains("纯黑色（#000000）背景"));
        assert!(submitted.contains("标准圆形图标框"));
        assert!(submitted.contains("宽高比4:3"));
        assert!(submitted.contains("清晰度2K"));
        assert!(!submitted.contains("AI创作最终抠图规范"));
        assert!(!submitted.contains("主体必须在画布内完整展示"));
    }
}
