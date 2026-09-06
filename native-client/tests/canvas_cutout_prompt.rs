#[path = "../src/runtime/generation/canvas_prompt.rs"]
mod canvas_prompt;

#[test]
fn every_canvas_request_ends_with_complete_isolated_subject_rules() {
    for request in [
        "角色换装，8套大裙摆",
        "植物生长",
        "怪物",
        "升级进化",
        "角色年龄变化",
        "角色体型变化",
        "建筑衍生器",
        "自由画布中的一把剑",
    ] {
        let prompt = canvas_prompt::build_canvas_generation_prompt(request, "16:9", "4K", false);
        assert!(prompt.starts_with(request));
        assert!(
            prompt.contains("全身完整展示"),
            "missing full subject constraint for {request}"
        );
        assert!(prompt.contains("画面四边"));
        assert!(prompt.contains("5%"));
        assert!(prompt.contains("70%"));
        assert!(prompt.contains("互不接触、互不重叠、互不连接"));
        assert!(prompt.contains("16:9"));
        assert!(prompt.contains("4K"));
        assert!(prompt.ends_with("完整性和间距优先于主体放大、铺满画面或固定单行排布。"));
    }
}

#[test]
fn english_canvas_requests_keep_dimensions_and_do_not_allow_cropping() {
    let prompt = canvas_prompt::build_canvas_generation_prompt(
        "Eight outfits, wide capes",
        "16:9",
        "2K",
        true,
    );
    assert!(prompt.contains("full body from the top of the head to the soles"));
    assert!(prompt.contains("Never crop"));
    assert!(prompt.contains("70%"));
    assert!(prompt.contains("at least 5%"));
    assert!(prompt.contains("16:9"));
    assert!(prompt.contains("2K"));
    assert!(!prompt.contains("workspace category"));
    assert!(!prompt.contains("UI component atlas"));
}
