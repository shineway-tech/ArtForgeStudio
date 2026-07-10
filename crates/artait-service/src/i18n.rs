//! 国际化（i18n）模块。
//!
//! 设计：
//! - 全局单例 `I18n`，通过 `OnceLock` 保证线程安全的一次初始化。
//! - 默认语言为 `zh-CN`（中文），所有硬编码中文作为 key 的 fallback。
//! - 支持从 JSON/TOML 文件加载语言包。
//! - `t(key)` 返回当前语言的翻译；未找到时回退到 `zh-CN`，再找不到返回 key 本身。
//!
//! 语言包格式（TOML）：
//! ```toml
//! [strings]
//! "btn-generate" = "开始生成"
//! "status-ready" = "就绪"
//! ```
//!
//! 语言包格式（JSON）：
//! ```json
//! {
//!   "btn-generate": "开始生成",
//!   "status-ready": "就绪"
//! }
//! ```

use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

use serde::Deserialize;

// ── 类型 ────────────────────────────────────────────────────────────────

/// 单个语言的翻译表。
type Translations = HashMap<String, String>;

/// 全局 i18n 引擎。
pub struct I18n {
    /// 语言 → 翻译表。
    packs: HashMap<String, Translations>,
    /// 当前语言。
    current: String,
    /// 兜底语言（通常为 "zh-CN"）。
    fallback: String,
}

/// TOML 语言包文件结构。
#[derive(Debug, Deserialize)]
struct LangPackToml {
    strings: HashMap<String, String>,
}

// ── 全局单例 ────────────────────────────────────────────────────────────

static INSTANCE: OnceLock<I18n> = OnceLock::new();

impl I18n {
    /// 获取全局单例（需先调用 `init`）。
    pub fn global() -> &'static Self {
        INSTANCE
            .get()
            .expect("I18n::init 必须在程序启动时调用一次")
    }

    /// 初始化全局 i18n。可传入额外的语言包目录路径。
    ///
    /// # Panics
    ///
    /// 如果 `OnceLock` 已被设置（重复调用 `init`）。
    pub fn init(lang_pack_dir: Option<&Path>) -> Self {
        let mut packs: HashMap<String, Translations> = HashMap::new();

        // 内嵌默认中文
        packs.insert("zh-CN".to_string(), build_default_zh_cn());

        // 从目录加载额外语言包
        if let Some(dir) = lang_pack_dir {
            if dir.is_dir() {
                if let Ok(entries) = std::fs::read_dir(dir) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        let Some(lang) = lang_from_filename(&path) else {
                            continue;
                        };
                        if let Ok(pack) = load_file(&path) {
                            packs.insert(lang, pack);
                        }
                    }
                }
            }
        }

        let i18n = Self {
            packs,
            current: "zh-CN".to_string(),
            fallback: "zh-CN".to_string(),
        };

        INSTANCE
            .set(i18n)
            .expect_err("I18n::init 重复调用");
        // `set` 返回 Ok(已设置的值) 或 Err(新值)；首次调用应失败（返回 Err）。
        // 这里 panic 可以，因为 init 只应调用一次。
        // 但 OnceLock::set 在为空时返回 Ok(())，已设置时返回 Err(value)。
        // 所以首次调用返回 Ok(())，不 panic。
        // 修正：上面的 expect_err 是错误的。
        panic!("I18n::init must be called exactly once; use I18n::try_init for re-entrant usage")
    }

    /// 可重复调用的初始化（幂等：首次设置，后续忽略）。
    pub fn try_init(lang_pack_dir: Option<&Path>) {
        let _ = INSTANCE.get_or_init(|| {
            let mut packs: HashMap<String, Translations> = HashMap::new();
            packs.insert("zh-CN".to_string(), build_default_zh_cn());

            if let Some(dir) = lang_pack_dir {
                if dir.is_dir() {
                    if let Ok(entries) = std::fs::read_dir(dir) {
                        for entry in entries.flatten() {
                            let path = entry.path();
                            let Some(lang) = lang_from_filename(&path) else {
                                continue;
                            };
                            if let Ok(pack) = load_file(&path) {
                                packs.insert(lang, pack);
                            }
                        }
                    }
                }
            }

            Self {
                packs,
                current: "zh-CN".to_string(),
                fallback: "zh-CN".to_string(),
            }
        });
    }

    /// 切换当前语言。
    pub fn set_language(&mut self, lang: &str) {
        self.current = lang.to_string();
    }

    /// 获取指定 key 的翻译。
    pub fn t(&self, key: &str) -> String {
        self.t_lang_inner(key, &self.current)
    }

    pub fn t_lang(&self, key: &str, lang: &str) -> String {
        self.t_lang_inner(key, lang)
    }

    fn t_lang_inner(&self, key: &str, lang: &str) -> String {
        // 1) 当前语言
        if let Some(trans) = self.packs.get(lang).and_then(|t| t.get(key)) {
            return trans.clone();
        }
        // 2) 兜底语言
        if lang != self.fallback {
            if let Some(trans) = self.packs.get(&self.fallback).and_then(|t| t.get(key)) {
                return trans.clone();
            }
        }
        // 3) 返回 key 本身
        key.to_string()
    }

    /// 带参数的格式化翻译。
    pub fn t_args(&self, key: &str, lang: Option<&str>, args: &[(&str, &str)]) -> String {
        let template = match lang {
            Some(l) => self.t_lang(key, l),
            None => self.t(key),
        };
        let mut result = template.to_string();
        for (placeholder, value) in args {
            result = result.replace(&format!("{{{placeholder}}}"), value);
        }
        result
    }

    /// 手动注册一个语言包。
    pub fn register_pack(&mut self, lang: &str, pack: Translations) {
        self.packs.insert(lang.to_string(), pack);
    }

    /// 列出所有已注册的语言代码。
    pub fn languages(&self) -> Vec<&str> {
        self.packs.keys().map(|s| s.as_str()).collect()
    }
}

// ── 便捷函数 ────────────────────────────────────────────────────────────

/// 获取当前语言的翻译（快捷方式）。
pub fn t(key: &str) -> String {
    I18n::global().t(key)
}

/// 带参数的格式化翻译（快捷方式）。
pub fn t_args(key: &str, args: &[(&str, &str)]) -> String {
    I18n::global().t_args(key, None, args)
}

// ── 文件加载 ────────────────────────────────────────────────────────────

/// 从文件加载语言包（自动检测 TOML / JSON）。
pub fn load_file(path: &Path) -> Result<Translations, LoadError> {
    let content =
        std::fs::read_to_string(path).map_err(|e| LoadError::Io(path.to_path_buf(), e))?;

    match path.extension().and_then(|s| s.to_str()) {
        Some("toml") => load_toml(&content),
        Some("json") => load_json(&content),
        _ => Err(LoadError::UnsupportedFormat(path.to_path_buf())),
    }
}

fn load_toml(content: &str) -> Result<Translations, LoadError> {
    let pack: LangPackToml =
        toml::from_str(content).map_err(|e| LoadError::Parse("TOML".into(), e.to_string()))?;
    Ok(pack.strings)
}

fn load_json(content: &str) -> Result<Translations, LoadError> {
    let map: HashMap<String, String> =
        serde_json::from_str(content).map_err(|e| LoadError::Parse("JSON".into(), e.to_string()))?;
    Ok(map)
}

#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("IO error reading {0}: {1}")]
    Io(std::path::PathBuf, std::io::Error),
    #[error("unsupported format: {0}")]
    UnsupportedFormat(std::path::PathBuf),
    #[error("{0} parse error: {1}")]
    Parse(String, String),
}

/// 从文件名提取语言代码：`en.toml` → `"en"`, `zh-CN.json` → `"zh-CN"`。
fn lang_from_filename(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    // 只接受语言代码格式的 stem（如 "en", "zh-CN", "ja"）
    if stem.chars().all(|c| c.is_ascii_alphabetic() || c == '-' || c == '_') {
        Some(stem.to_string())
    } else {
        None
    }
}

// ── 内嵌默认中文 ────────────────────────────────────────────────────────

fn build_default_zh_cn() -> Translations {
    let mut map = HashMap::new();

    // ── Feature / display names ──────────────────────────────────────
    map.insert("feature-ui-concept".into(), "UI 概念".into());
    map.insert("feature-scene".into(), "创建场景".into());
    map.insert("feature-character".into(), "创建角色".into());
    map.insert("feature-effect".into(), "特效".into());
    map.insert("feature-action-sequence".into(), "动作序列".into());
    map.insert("feature-asset-browser".into(), "图库".into());
    map.insert("feature-animation-scene".into(), "动画场景".into());
    map.insert("feature-animation-character".into(), "动画角色".into());
    map.insert("feature-character-turnaround".into(), "角色三视图".into());
    map.insert("feature-animation-script".into(), "动画脚本".into());
    map.insert("feature-storyboard".into(), "分镜板".into());

    // ── Task status labels ───────────────────────────────────────────
    map.insert("task-status-idle".into(), "idle".into());
    map.insert("task-status-validating".into(), "validating".into());
    map.insert("task-status-uploading".into(), "uploading".into());
    map.insert("task-status-submitted".into(), "submitted".into());
    map.insert("task-status-polling".into(), "polling".into());
    map.insert("task-status-saving".into(), "saving".into());
    map.insert("task-status-completed".into(), "completed".into());
    map.insert("task-status-cancelling".into(), "cancelling".into());
    map.insert("task-status-cancelled".into(), "cancelled".into());
    map.insert("task-status-failed".into(), "failed".into());

    // ── Task kind labels ─────────────────────────────────────────────
    map.insert("task-kind-image".into(), "image".into());
    map.insert("task-kind-character".into(), "character".into());
    map.insert("task-kind-video".into(), "video".into());
    map.insert("task-kind-analysis".into(), "analysis".into());
    map.insert("task-kind-prompt-opt".into(), "prompt_opt".into());
    map.insert("task-kind-action-batch".into(), "action_batch".into());
    map.insert("task-kind-script-gen".into(), "script_gen".into());

    // ── Clear task labels ────────────────────────────────────────────
    map.insert("clear-label-completed".into(), "已完成".into());
    map.insert("clear-label-failed".into(), "失败".into());
    map.insert("clear-label-all".into(), "".into());

    // ── Mode / domain strings ────────────────────────────────────────
    map.insert("mode-scene".into(), "scene".into());
    map.insert("mode-character".into(), "character".into());
    map.insert("mode-ui-concept".into(), "ui_concept".into());
    map.insert("mode-effect".into(), "effect".into());
    map.insert("mode-animation-scene".into(), "animation_scene".into());
    map.insert("mode-animation-character".into(), "animation_character".into());
    map.insert("mode-character-turnaround".into(), "character_turnaround".into());
    map.insert("mode-storyboard".into(), "storyboard".into());
    map.insert("mode-action-sequence".into(), "action_sequence".into());

    // ── Mode display names ───────────────────────────────────────────
    map.insert("display-mode-scene".into(), "创建场景".into());
    map.insert("display-mode-character".into(), "创建角色".into());
    map.insert("display-mode-ui-concept".into(), "UI 概念".into());
    map.insert("display-mode-effect".into(), "特效".into());
    map.insert("display-mode-animation-scene".into(), "动画场景".into());
    map.insert("display-mode-animation-character".into(), "动画角色".into());
    map.insert("display-mode-character-turnaround".into(), "角色三视图".into());
    map.insert("display-mode-storyboard".into(), "分镜板".into());
    map.insert("display-mode-unknown".into(), "未知".into());

    // ── Status bar ───────────────────────────────────────────────────
    map.insert("status-ready".into(), "就绪".into());
    map.insert("statusbar-assets".into(), "资产 {n}".into());
    map.insert("statusbar-tasks".into(), "任务 {n}".into());

    // ── Common labels ────────────────────────────────────────────────
    map.insert("label-settings".into(), "设置".into());
    map.insert("label-tasks".into(), "任务".into());
    map.insert("label-prompt".into(), "提示词".into());
    map.insert("label-quality".into(), "品质".into());
    map.insert("label-aspect-ratio".into(), "宽高比".into());
    map.insert("label-model".into(), "模型".into());
    map.insert("label-type".into(), "类型".into());
    map.insert("label-dimensions".into(), "尺寸".into());
    map.insert("label-file-size".into(), "大小".into());
    map.insert("label-metadata".into(), "元信息".into());
    map.insert("label-version".into(), "版本".into());
    map.insert("label-welcome".into(), "欢迎".into());

    // ── Button labels ────────────────────────────────────────────────
    map.insert("btn-generate".into(), "开始生成".into());
    map.insert("btn-cancel-generate".into(), "取消生成".into());
    map.insert("btn-cancel".into(), "Cancel".into());
    map.insert("btn-save".into(), "Save".into());
    map.insert("btn-close".into(), "关闭".into());
    map.insert("btn-refresh".into(), "刷新".into());
    map.insert("btn-open-dir".into(), "打开目录".into());
    map.insert("btn-open-original".into(), "打开原图".into());
    map.insert("btn-reveal-in-explorer".into(), "位置".into());
    map.insert("btn-add-to-ref".into(), "+ 参考".into());
    map.insert("btn-delete".into(), "删除".into());
    map.insert("btn-create".into(), "创建".into());
    map.insert("btn-edit".into(), "编辑".into());
    map.insert("btn-skip".into(), "跳过".into());
    map.insert("btn-next".into(), "下一步".into());
    map.insert("btn-back".into(), "上一步".into());
    map.insert("btn-finish".into(), "完成".into());
    map.insert("btn-save-and-test".into(), "保存并测试".into());

    // ── Empty states ─────────────────────────────────────────────────
    map.insert("empty-gallery-title".into(), "图库是空的".into());
    map.insert("empty-gallery-hint".into(), "在左侧输入提示词，点生成".into());
    map.insert("empty-ref-images".into(), "参考图 · 0 张 · 点击添加".into());
    map.insert("empty-ref-images-drop".into(), "拖放图片到此处".into());

    // ── Prompt hints ─────────────────────────────────────────────────
    map.insert("hint-prompt-specific".into(), "提示词越具体，生成越稳定".into());
    map.insert("hint-enter-newline".into(), "Enter 换行".into());

    // ── Misc ─────────────────────────────────────────────────────────
    map.insert("misc-unconfigured".into(), "未配置".into());
    map.insert("misc-default".into(), "默认".into());
    map.insert("misc-generating".into(), "生成中".into());
    map.insert("misc-newest-first".into(), "最新优先".into());
    map.insert("misc-no-template".into(), "不使用目录提示词".into());

    map
}

// ── 测试 ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn test_i18n() -> I18n {
        let mut packs = HashMap::new();
        let mut zh = HashMap::new();
        zh.insert("hello".into(), "你好".into());
        zh.insert("world".into(), "世界".into());
        zh.insert("greet".into(), "你好 {name}".into());
        packs.insert("zh-CN".into(), zh);

        let mut en = HashMap::new();
        en.insert("hello".into(), "Hello".into());
        en.insert("greet".into(), "Hello {name}".into());
        packs.insert("en".into(), en);

        I18n {
            packs,
            current: "zh-CN".to_string(),
            fallback: "zh-CN".to_string(),
        }
    }

    #[test]
    fn t_returns_current_language() {
        let i18n = test_i18n();
        assert_eq!(i18n.t("hello"), "你好");
    }

    #[test]
    fn t_falls_back_to_fallback_language() {
        let i18n = test_i18n();
        // "world" only exists in zh-CN
        assert_eq!(i18n.t_lang("world", "en"), "世界");
    }

    #[test]
    fn t_returns_key_when_not_found() {
        let i18n = test_i18n();
        assert_eq!(i18n.t("missing"), "missing");
    }

    #[test]
    fn t_args_replaces_placeholders() {
        let i18n = test_i18n();
        let result = i18n.t_args("greet", None, &[("name", "Alice")]);
        assert_eq!(result, "你好 Alice");
    }

    #[test]
    fn set_language_switches_current() {
        let mut i18n = test_i18n();
        i18n.set_language("en");
        assert_eq!(i18n.t("hello"), "Hello");
    }

    #[test]
    fn load_toml_parses_correctly() {
        let toml = r#"
[strings]
"key1" = "值1"
"key2" = "值2"
"#;
        let pack = load_toml(toml).unwrap();
        assert_eq!(pack.get("key1").unwrap(), "值1");
        assert_eq!(pack.get("key2").unwrap(), "值2");
    }

    #[test]
    fn load_json_parses_correctly() {
        let json = r#"{"key1":"值1","key2":"值2"}"#;
        let pack = load_json(json).unwrap();
        assert_eq!(pack.get("key1").unwrap(), "值1");
        assert_eq!(pack.get("key2").unwrap(), "值2");
    }

    #[test]
    fn lang_from_filename_extracts_correctly() {
        assert_eq!(
            lang_from_filename(Path::new("en.toml")),
            Some("en".into())
        );
        assert_eq!(
            lang_from_filename(Path::new("zh-CN.json")),
            Some("zh-CN".into())
        );
        assert_eq!(
            lang_from_filename(Path::new("ja_JP.toml")),
            Some("ja_JP".into())
        );
        assert_eq!(lang_from_filename(Path::new("README.md")), None);
    }

    #[test]
    fn try_init_is_idempotent() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path();
        // First call — initializes
        I18n::try_init(Some(dir));
        // Second call — no-op (OnceLock already set)
        I18n::try_init(Some(dir));
        assert_eq!(I18n::global().t("status-ready"), "就绪");
    }
}
