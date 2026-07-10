//! 任务面板过滤与标签辅助函数。

use artait_model;

/// 返回清空操作的中文标签。
pub fn clear_task_label(filter: &str) -> &'static str {
    match filter {
        "completed" => "已完成",
        "failed" => "失败",
        "all" => "",
        _ => "",
    }
}

/// 判断一条任务是否匹配清空过滤条件。
pub fn task_matches_clear_filter(task_status: &str, filter: &str) -> bool {
    match filter {
        "completed" => task_status == "completed",
        "failed" => task_status == "failed" || task_status == "cancelled",
        "all" => !artait_model::is_active_task_status(task_status),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clear_label_returns_correct_chinese() {
        assert_eq!(clear_task_label("completed"), "已完成");
        assert_eq!(clear_task_label("failed"), "失败");
        assert_eq!(clear_task_label("all"), "");
        assert_eq!(clear_task_label("unknown"), "");
    }

    #[test]
    fn task_matches_completed_filter() {
        assert!(task_matches_clear_filter("completed", "completed"));
        assert!(!task_matches_clear_filter("running", "completed"));
    }

    #[test]
    fn task_matches_failed_filter() {
        assert!(task_matches_clear_filter("failed", "failed"));
        assert!(task_matches_clear_filter("cancelled", "failed"));
        assert!(!task_matches_clear_filter("completed", "failed"));
    }

    #[test]
    fn task_matches_all_excludes_active() {
        assert!(!task_matches_clear_filter("running", "all"));
        assert!(task_matches_clear_filter("completed", "all"));
        assert!(task_matches_clear_filter("failed", "all"));
    }
}
