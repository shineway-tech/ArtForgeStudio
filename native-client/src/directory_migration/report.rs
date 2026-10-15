use std::path::PathBuf;

/// Counts copied files separately from a successfully committed relocation.
#[derive(Clone, Debug, Default)]
pub(crate) struct MigrationReport {
    pub total_files: usize,
    pub copied_files: usize,
    pub copied_bytes: u64,
    pub skipped_files: Vec<PathBuf>,
    pub failed_files: Vec<(PathBuf, String)>,
    pub operation_errors: Vec<String>,
    pub move_source: bool,
    pub removed_source_files: usize,
    pub retained_source_files: Vec<(PathBuf, String)>,
}

impl MigrationReport {
    pub fn render(&self, committed: bool) -> String {
        let pending = self.total_files.saturating_sub(self.copied_files + self.failed_files.len());
        let heading = if committed { "迁移完成，新目录已保存。" } else { "迁移未完成，仍使用原目录。" };
        let copied = if committed { "成功" } else { "已复制" };
        let mode = if self.move_source { "剪切" } else { "拷贝" };
        let mut text = format!(
            "{heading}\n迁移方式：{mode}\n{copied}：{} 个文件\n跳过：{} 个文件\n失败：{} 个文件\n未处理：{pending} 个文件\n已复制大小：{} 字节",
            self.copied_files, self.skipped_files.len(), self.failed_files.len(), self.copied_bytes,
        );
        if self.move_source && committed {
            let pending_cleanup = self.copied_files.saturating_sub(self.removed_source_files + self.retained_source_files.len());
            text.push_str(&format!("\n已清理原文件：{} 个\n原文件清理失败：{} 个\n尚未清理原文件：{pending_cleanup} 个\n原目录可能保留为空文件夹。",
                self.removed_source_files, self.retained_source_files.len()));
        } else {
            text.push_str("\n原文件已保留。");
        }
        if !self.operation_errors.is_empty() {
            text.push_str(&format!("\n\n流程错误：{} 项", self.operation_errors.len()));
            for reason in &self.operation_errors { text.push_str(&format!("\n• {reason}")); }
        }
        if !self.failed_files.is_empty() {
            text.push_str("\n\n失败明细：");
            for (path, reason) in &self.failed_files {
                text.push_str(&format!("\n• {}：{reason}", path.display()));
            }
        }
        if !self.skipped_files.is_empty() {
            text.push_str("\n\n跳过明细：");
            for path in &self.skipped_files {
                text.push_str(&format!("\n• {}：迁移开始前已被删除或移走，或已变为文件夹，本次跳过旧文件记录", path.display()));
            }
        }
        if !self.retained_source_files.is_empty() {
            text.push_str("\n\n以下原文件仍保留，新目录中的副本已保存：");
            for (path, reason) in &self.retained_source_files {
                text.push_str(&format!("\n• {}：{reason}", path.display()));
            }
        }
        if !committed {
            text.push_str("\n\n目标目录可能留有已复制文件或临时文件，可处理原因后重新迁移。");
        }
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completed_report_distinguishes_missing_files_and_lists_reasons() {
        let report = MigrationReport {
            total_files: 2, copied_files: 2, copied_bytes: 4096,
            skipped_files: vec![PathBuf::from("old/deleted.png")],
            ..Default::default()
        };
        let text = report.render(true);
        assert!(text.contains("成功：2"));
        assert!(text.contains("跳过：1"));
        assert!(text.contains("失败：0"));
        assert!(text.contains("未处理：0"));
        assert!(text.contains("old/deleted.png"));
        assert!(text.contains("删除或移走"));
    }

    #[test]
    fn failed_report_does_not_claim_location_changed_or_count_unprocessed_as_failures() {
        let report = MigrationReport {
            total_files: 5, copied_files: 2,
            failed_files: vec![(PathBuf::from("old/locked.png"), "没有读取权限".into())],
            operation_errors: vec!["保存新目录失败".into()],
            ..Default::default()
        };
        let text = report.render(false);
        assert!(text.contains("仍使用原目录"));
        assert!(text.contains("已复制：2"));
        assert!(text.contains("失败：1"));
        assert!(text.contains("未处理：2"));
        assert!(text.contains("流程错误：1"));
        assert!(text.contains("old/locked.png：没有读取权限"));
        assert!(text.contains("保存新目录失败"));
    }

    #[test]
    fn move_report_lists_originals_that_could_not_be_removed() {
        let report = MigrationReport {
            total_files: 2, copied_files: 2, move_source: true, removed_source_files: 1,
            retained_source_files: vec![(PathBuf::from("old/edited.png"), "复制后原文件发生变化".into())],
            ..Default::default()
        };
        let text = report.render(true);
        assert!(text.contains("剪切"));
        assert!(text.contains("已清理原文件：1"));
        assert!(text.contains("原文件清理失败：1"));
        assert!(text.contains("old/edited.png：复制后原文件发生变化"));
        assert!(!text.contains("原文件已保留。"));
    }
}
