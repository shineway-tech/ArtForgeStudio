# 待完成事项

> 2025-06-15 更新 — P0/P1 全部收尾

## ✅ P0 — 接线（已完成）

### 1. provider callback 接入 service ✅
- `callbacks/provider.rs` 已调用 `artait_service::provider::create_provider()` (L582) / `edit_provider()` (L558)
- `validate_endpoint()` 由 `create_provider`/`edit_provider` 内部调用
- 连接测试走 `artait_service::provider_helpers::run_connection_test()` (L601)
- 模型解析/合并/Scope 规范化全部走 service helper

### 2. SettingsSaveOutcome 实际使用 ✅
- `callbacks/settings.rs` L337：`if outcome.output_dir_changed { ... }` 条件重启 watcher
- 不再无条件重启 asset watcher

## ✅ P1 — 清理（已完成）

### 3. 工作区文件清理 ✅
- `LICENSE`、`PROJECT_STRUCTURE.md`、`config.example.json`、`scripts/build-release.ps1` 均存在，未误删
- `.codegraph/`、`.reasonix/` 已加入 `.gitignore`
- ⚠️ `.nezha/` 目录（Agent 框架缓存）建议也加入 `.gitignore`

## 当前状态

```
artait-service/ (13 modules)
├── assets.rs           ✅ read_asset_metadata
├── generation.rs       ✅ run_image_generation + 8 tests
├── onboarding.rs       ✅
├── page_routing.rs     ✅ is_workspace_page / initial_page_from_config + 6 tests
├── prompt_template.rs  ✅
├── provider.rs         ✅ create_provider/edit_provider/validate_endpoint
├── provider_helpers.rs ✅ run_analysis/run_connection_test + 16 tests
├── script.rs           ✅ generate_script_via_provider
├── settings.rs         ✅ apply_basic_settings / SettingsSaveOutcome + 5 tests
├── task_filter.rs      ✅ clear_task_label / task_matches_clear_filter + 4 tests
├── task_history.rs     ✅
└── utils.rs            ✅ short/short_safe/mime_for_path/is_image_path + 4 tests

artait-model/task.rs: ✅ is_active_task_status + 2 tests (统一 bridge.rs/main.rs 重复)

callbacks/ (8 files)   ✅ 全部接入 service
main.rs: 4542 → 768 行 (-83%)
生成链路: mode: &str → CreationMode 枚举（删除 4 个字符串匹配函数）
tests:  5 → 80 (48 unit + 11 integration + artait-model)
```
