# ComfyUI/LiteGraph 风格无限画布 Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** 在现有 Rust + Slint 客户端中实现选择/平移、多选与完整分组、增强连线和空白处节点搜索，并保持本地存储及服务端兼容。

**Architecture:** Slint 继续负责画布渲染、即时预览和输入事件；新增 Rust `canvas_ops` 操作内核负责选择事务、批量移动、复制、分组关系和连线校验。持久模型只增加带默认值的分组字段，临时选择状态不序列化，所有结构性修改统一进入已有撤销历史。

**Tech Stack:** Rust 2021、Slint 1.8、serde/serde_json、现有本地 Store、Cargo 测试与 PowerShell 打包脚本。

---

## 实施约束

- 工作树：`C:\Users\deyx1\Documents\ArtForgeStudio-canvas-interactions`
- 分支：`codex/comfy-litegraph-canvas`
- 不修改或接入归档的 `crates/`。
- 不修改服务端 API、生成接口和媒体协议。
- 不引入 WebView、React、LiteGraph 或 ComfyUI 源码依赖。
- 每个任务完成后运行对应测试并独立提交。
- 最终统一执行全量测试、release 构建和 Windows 绿色包输出。
- 不自动推送远程。

### Task 1: 建立基线并增加向后兼容的画布模型字段

**Files:**
- Modify: `native-client/src/runtime/model.rs:40-61`
- Modify: `native-client/ui/types.slint:122-142`
- Modify: `native-client/src/runtime/presentation/sync.rs:246-292`
- Test: `native-client/src/runtime/tests.rs`
- Test: `native-client/src/runtime/presentation/sync.rs`

**Step 1: 运行现有基线**

Run:

```powershell
cargo test -p artforge-studio-native
```

Expected: 当前测试全部通过；记录测试总数，后续不得减少。

**Step 2: 写模型兼容失败测试**

在 `runtime/tests.rs` 增加测试，构造旧格式 JSON，断言缺失字段得到默认值：

```rust
#[test]
fn legacy_canvas_notes_default_to_top_level_and_unselected() {
    let note: CanvasNoteData = serde_json::from_str(
        r#"{"id":"n1","kind":"text","content":"","x":10.0,"y":20.0,"width":320.0,"height":210.0}"#,
    )
    .expect("legacy canvas note");
    assert_eq!(note.parent_group_id, "");
    assert_eq!(note.z_index, 0);
    assert!(!note.selected);
}
```

Run:

```powershell
cargo test -p artforge-studio-native legacy_canvas_notes_default_to_top_level_and_unselected
```

Expected: FAIL，字段尚不存在。

**Step 3: 增加持久字段和瞬时选择字段**

将 `CanvasNoteData` 扩展为：

```rust
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
struct CanvasNoteData {
    id: String,
    #[serde(default = "default_canvas_node_kind")]
    kind: String,
    content: String,
    x: f32,
    y: f32,
    #[serde(default = "default_canvas_node_width")]
    width: f32,
    #[serde(default = "default_canvas_node_height")]
    height: f32,
    #[serde(default)]
    parent_group_id: String,
    #[serde(default)]
    z_index: i32,
    #[serde(skip)]
    selected: bool,
}
```

在 Slint `CanvasNote` 中增加：

```slint
parent-group-id: string,
z-index: int,
selected: bool,
```

在 `push_canvas_notes()` 中完整映射这些字段。

**Step 4: 增加异常父关系清洗**

在载入画布数据后执行纯函数：

```rust
fn normalize_canvas_groups(notes: &mut [CanvasNoteData]) {
    // 清除不存在、指向普通节点、自引用或形成祖先循环的 parent_group_id。
    // 保留节点内容、尺寸和世界坐标。
}
```

为不存在父 ID、自引用和两节点循环各写一个单元测试。

**Step 5: 运行模型与全量测试**

Run:

```powershell
cargo test -p artforge-studio-native canvas
cargo test -p artforge-studio-native
```

Expected: PASS。

**Step 6: 提交**

```powershell
git add native-client/src/runtime/model.rs native-client/ui/types.slint native-client/src/runtime/presentation/sync.rs native-client/src/runtime/tests.rs native-client/src/runtime/storage/local_store.rs
git commit -m "feat: extend canvas model for nested groups"
```

### Task 2: 抽出可测试的画布操作内核与事务历史

**Files:**
- Create: `native-client/src/runtime/canvas_ops.rs`
- Modify: `native-client/src/runtime/mod.rs:1-90`
- Modify: `native-client/src/runtime/callbacks/infinite_canvas.rs`
- Test: `native-client/src/runtime/canvas_ops.rs`

**Step 1: 写操作内核失败测试**

覆盖以下行为：

```rust
#[test]
fn selected_descendants_move_once_with_their_group() { /* ... */ }

#[test]
fn grouping_uses_the_deepest_valid_container() { /* ... */ }

#[test]
fn a_group_cannot_be_parented_to_its_descendant() { /* ... */ }

#[test]
fn one_drag_creates_one_history_entry() { /* ... */ }
```

Run:

```powershell
cargo test -p artforge-studio-native canvas_ops
```

Expected: FAIL，模块尚不存在。

**Step 2: 建立操作内核数据结构**

新增：

```rust
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct CanvasSnapshot {
    pub notes: Vec<CanvasNoteData>,
    pub links: Vec<CanvasLinkData>,
}

#[derive(Clone, Debug, Default)]
pub(super) struct CanvasClipboard {
    notes: Vec<CanvasNoteData>,
    links: Vec<CanvasLinkData>,
}

#[derive(Default)]
pub(super) struct CanvasController {
    undo: Vec<CanvasSnapshot>,
    redo: Vec<CanvasSnapshot>,
    clipboard: CanvasClipboard,
}
```

迁移 `CanvasHistory` 到 `CanvasController`，保留 100 条历史上限。

**Step 3: 实现纯操作函数**

至少提供：

```rust
pub(super) fn selected_ids(notes: &[CanvasNoteData]) -> BTreeSet<String>;
pub(super) fn descendant_ids(notes: &[CanvasNoteData], group_id: &str) -> BTreeSet<String>;
pub(super) fn move_selection(notes: &mut [CanvasNoteData], dx: f32, dy: f32);
pub(super) fn select_in_rect(notes: &mut [CanvasNoteData], rect: CanvasRect, additive: bool);
pub(super) fn assign_deepest_group(notes: &mut [CanvasNoteData], node_ids: &BTreeSet<String>);
pub(super) fn group_selection(notes: &mut Vec<CanvasNoteData>, english: bool) -> Option<String>;
pub(super) fn ungroup_selection(notes: &mut [CanvasNoteData]);
```

移动时对“已选分组的后代”和“同时被选中的后代”去重，确保每个节点只移动一次。

**Step 4: 让回调共享控制器**

`wire_infinite_canvas_callbacks()` 创建单个 `Rc<RefCell<CanvasController>>`，原有新增、更新、删除、连线、撤销和重做回调全部通过它记录事务。

**Step 5: 运行测试**

Run:

```powershell
cargo test -p artforge-studio-native canvas_ops
cargo test -p artforge-studio-native infinite_canvas
```

Expected: PASS。

**Step 6: 提交**

```powershell
git add native-client/src/runtime/canvas_ops.rs native-client/src/runtime/mod.rs native-client/src/runtime/callbacks/infinite_canvas.rs
git commit -m "refactor: add transactional canvas operations"
```

### Task 3: 增加多选、框选和批量命令回调

**Files:**
- Modify: `native-client/ui/app-state.slint:81-104,347-356`
- Modify: `native-client/src/runtime/callbacks/infinite_canvas.rs`
- Modify: `native-client/src/runtime/canvas_ops.rs`
- Modify: `native-client/src/runtime/presentation/sync.rs`
- Test: `native-client/src/runtime/tests.rs`
- Test: `native-client/src/runtime/canvas_ops.rs`

**Step 1: 写回调契约失败测试**

在 `runtime/tests.rs` 断言 AppState 声明并且 Rust 注册：

```slint
in-out property <int> canvas-selected-count: 0;
callback select-canvas-node(string, bool);
callback select-canvas-rect(float, float, float, float, bool);
callback clear-canvas-selection();
callback move-canvas-selection(float, float);
callback copy-canvas-selection();
callback paste-canvas-selection(float, float);
callback duplicate-canvas-selection();
callback remove-canvas-selection();
callback group-canvas-selection(float, float);
callback ungroup-canvas-selection();
```

Run:

```powershell
cargo test -p artforge-studio-native infinite_canvas_exposes_multi_selection_commands
```

Expected: FAIL。

**Step 2: 声明 AppState 属性和回调**

保留 `canvas-selected-id` 作为主选择项和单节点编辑锚点；`CanvasNote.selected` 表示完整选择集。每次选择改变后调用统一同步函数：

```rust
fn sync_canvas_selection(app: &AppWindow, store: &Store) {
    let state = app.global::<AppState>();
    state.set_canvas_selected_count(store.canvas_notes.iter().filter(|n| n.selected).count() as i32);
    push_canvas_notes(app, store);
}
```

**Step 3: 注册选择和批量操作回调**

规则：

- 普通点击：清除旧选择后选中目标。
- `Ctrl` 点击：切换目标选择状态。
- 框选：按矩形与节点包围盒相交判断。
- 批量移动：扩展选中分组的后代并只移动一次。
- 删除分组：解除直接子项，普通节点则清理关联连线。
- 所有结构修改记录一次历史并保存一次。

**Step 4: 实现内部剪贴板**

复制选中节点时包含：

- 选中的普通节点。
- 选中分组的全部后代。
- 两端都位于复制集合内的连线。

粘贴时生成新 UUID，重写父 ID 和内部连线 ID，按请求世界坐标或默认 `(24, 24)` 偏移定位。

**Step 5: 测试选择、复制和删除**

Run:

```powershell
cargo test -p artforge-studio-native canvas_selection
cargo test -p artforge-studio-native canvas_clipboard
cargo test -p artforge-studio-native canvas_delete
```

Expected: PASS。

**Step 6: 提交**

```powershell
git add native-client/ui/app-state.slint native-client/src/runtime/callbacks/infinite_canvas.rs native-client/src/runtime/canvas_ops.rs native-client/src/runtime/presentation/sync.rs native-client/src/runtime/tests.rs
git commit -m "feat: add canvas multi-selection commands"
```

### Task 4: 实现选择/平移模式、框选和桌面快捷键

**Files:**
- Modify: `native-client/ui/pages/infinite-canvas-page.slint:473-1245,1506-1950`
- Modify: `native-client/src/runtime/tests.rs`
- Reuse: `native-client/ui/assets/icons/canvas.svg`
- Reuse: `native-client/ui/assets/icons/move.svg`

**Step 1: 写 Slint 结构失败测试**

断言页面包含：

- `marquee-active`
- `marquee-start-x/y`
- `space-pan-active`
- `temporary-pan-active`
- `select-canvas-rect`
- `move-canvas-selection`
- `Key.Delete`、`Key.Backspace`
- `event.modifiers.control`
- `Key.Escape`
- `Key.Space`

Run:

```powershell
cargo test -p artforge-studio-native infinite_canvas_selection_and_pan_modes
```

Expected: FAIL。

**Step 2: 将空白拖动按模式分流**

选择模式左键拖动：

```slint
root.marquee-active = true;
root.marquee-start-x = self.mouse-x;
root.marquee-start-y = self.mouse-y;
```

释放时把起止屏幕坐标转换为规范化世界坐标，调用 `select-canvas-rect`。

平移模式、中键或空格加左键拖动继续修改 `pan-x/pan-y`。临时平移结束后仅清除临时标记，不改变 `AppState.canvas-tool`。

**Step 3: 绘制框选层**

选择矩形位于节点和连线之上、浮动操作条之下，使用 Sprite Green 主题强调色、半透明背景和 1px 边框。

**Step 4: 改造节点拖动**

`CanvasNodeCard` 使用 `root.note.selected` 绘制选择态。拖动开始时：

- 未选节点：先执行单选。
- 已选节点：保持选择集。

移动期间只更新共享预览位移；释放时调用一次 `move-canvas-selection(dx, dy)`。

**Step 5: 增加快捷键**

页面 `FocusScope` 处理删除、复制粘贴、快速复制、全选、撤销重做、分组/解组、聚焦和 Escape。TextInput 获得焦点时页面 FocusScope 不抢占其按键。

**Step 6: 实现 F 聚焦**

Rust 或 Slint 根据选中节点包围盒计算：

```text
zoom = min(canvas_width / bounds_width, canvas_height / bounds_height) * 0.84
```

限制在 5%–500%，并设置 pan 使包围盒中心对齐画布中心。无选择时使用全部节点。

**Step 7: 编译并测试**

Run:

```powershell
cargo check -p artforge-studio-native
cargo test -p artforge-studio-native infinite_canvas_selection_and_pan_modes
```

Expected: PASS，Slint 编译无属性或事件错误。

**Step 8: 提交**

```powershell
git add native-client/ui/pages/infinite-canvas-page.slint native-client/src/runtime/tests.rs
git commit -m "feat: add canvas marquee and desktop shortcuts"
```

### Task 5: 实现嵌套分组、归组和稳定渲染层级

**Files:**
- Modify: `native-client/src/runtime/canvas_ops.rs`
- Modify: `native-client/src/runtime/callbacks/infinite_canvas.rs`
- Modify: `native-client/ui/pages/infinite-canvas-page.slint`
- Modify: `native-client/src/runtime/tests.rs`
- Test: `native-client/src/runtime/canvas_ops.rs`

**Step 1: 写分组行为失败测试**

覆盖：

```rust
#[test]
fn group_selection_wraps_bounds_with_padding() { /* ... */ }

#[test]
fn dragging_a_group_moves_each_descendant_once() { /* ... */ }

#[test]
fn dropping_uses_the_smallest_deepest_group() { /* ... */ }

#[test]
fn deleting_a_group_promotes_direct_children() { /* ... */ }

#[test]
fn copying_a_group_keeps_only_internal_links() { /* ... */ }
```

Run:

```powershell
cargo test -p artforge-studio-native canvas_group
```

Expected: FAIL。

**Step 2: 完成分组几何函数**

使用固定世界坐标内边距 36：

```rust
const GROUP_PADDING: f32 = 36.0;

fn selection_bounds(notes: &[CanvasNoteData], ids: &BTreeSet<String>) -> Option<CanvasRect>;
fn minimum_group_rect(notes: &[CanvasNoteData], group_id: &str) -> Option<CanvasRect>;
fn deepest_containing_group(notes: &[CanvasNoteData], node: &CanvasNoteData) -> Option<String>;
```

无选择时的分组仍使用现有默认尺寸和视图中心。

**Step 3: 完成拖动结束后的归组**

批量移动提交后，针对顶层被移动项重新计算父分组。忽略自身、已选后代和会形成循环的候选分组。

**Step 4: 调整删除和解组规则**

- 删除分组：直接子项提升到被删分组的父级。
- 解组：选中分组的直接子项提升一级，并删除空壳分组。
- 普通节点删除：保持现有连线清理。

**Step 5: 调整渲染层级**

维持分组背景先渲染、普通节点后渲染。增加层级排序所需的稳定 `z-index`，外层分组不得覆盖内层节点、框选层和浮动工具条。

操作条继续使用已修复的固定方向：

- 普通节点主要操作条在节点上方。
- 媒体编辑面板在节点下方。
- 不因画布边缘翻转，允许超出可视区域。

**Step 6: 运行测试与检查**

Run:

```powershell
cargo test -p artforge-studio-native canvas_group
cargo check -p artforge-studio-native
```

Expected: PASS。

**Step 7: 提交**

```powershell
git add native-client/src/runtime/canvas_ops.rs native-client/src/runtime/callbacks/infinite_canvas.rs native-client/ui/pages/infinite-canvas-page.slint native-client/src/runtime/tests.rs
git commit -m "feat: add nested canvas group containers"
```

### Task 6: 实现端口高亮、重连和连接事务

**Files:**
- Modify: `native-client/ui/app-state.slint`
- Modify: `native-client/ui/pages/infinite-canvas-page.slint:368-465,473-1245,1513-1795`
- Modify: `native-client/src/runtime/canvas_ops.rs`
- Modify: `native-client/src/runtime/callbacks/infinite_canvas.rs`
- Modify: `native-client/src/runtime/tests.rs`
- Test: `native-client/src/runtime/canvas_ops.rs`

**Step 1: 写连线失败测试**

覆盖：

```rust
#[test]
fn connecting_to_an_occupied_input_replaces_the_old_link() { /* ... */ }

#[test]
fn reconnect_cancel_restores_the_original_link() { /* ... */ }

#[test]
fn reconnect_rejects_cycles_without_mutation() { /* ... */ }

#[test]
fn compatible_target_uses_screen_constant_tolerance() { /* ... */ }
```

Run:

```powershell
cargo test -p artforge-studio-native canvas_reconnect
```

Expected: FAIL。

**Step 2: 定义连接结果**

Rust 内部使用：

```rust
enum ConnectResult {
    Connected { link_id: String, target_id: String },
    Empty,
    Rejected(CanvasConnectError),
}
```

Slint 回调返回稳定字符串：`"connected"`、`"empty"`、`"rejected"`。只有 `"empty"` 打开节点搜索。

**Step 3: 将连接建立改为原子替换**

输入端口已有连接时，先在候选快照中移除旧连接，再做重复与循环检查；校验通过后一次性提交并记录一条历史。失败时 Store 完全不变。

**Step 4: 增加输入端口反向重连**

输入端口开始拖动时记录原连接 ID 和来源节点。UI 仅显示临时断开效果；释放到有效输出后替换来源，Escape 或无效释放恢复原显示。

**Step 5: 增加兼容端口高亮**

连接拖动期间，目标节点根据 Rust/Slint 可判定状态显示：

- 绿色强调：可连接。
- 灰色：自身或分组。
- 红色：重复连接或会形成循环。

命中容差使用 `screen_px * 100 / zoom_percent` 换算，保持屏幕视觉大小稳定。

**Step 6: 测试与提交**

Run:

```powershell
cargo test -p artforge-studio-native canvas_reconnect
cargo check -p artforge-studio-native
```

Expected: PASS。

```powershell
git add native-client/ui/app-state.slint native-client/ui/pages/infinite-canvas-page.slint native-client/src/runtime/canvas_ops.rs native-client/src/runtime/callbacks/infinite_canvas.rs native-client/src/runtime/tests.rs
git commit -m "feat: add canvas link reconnection"
```

### Task 7: 实现空白处节点搜索和自动连接

**Files:**
- Modify: `native-client/ui/pages/infinite-canvas-page.slint`
- Modify: `native-client/ui/app-state.slint`
- Modify: `native-client/src/runtime/callbacks/infinite_canvas.rs`
- Modify: `native-client/src/runtime/canvas_ops.rs`
- Modify: `native-client/src/runtime/tests.rs`

**Step 1: 写搜索弹窗失败测试**

断言存在：

- `node-search-open`
- `node-search-query`
- `node-search-world-x/y`
- `node-search-source-id`
- 四类节点候选
- 上下键、Enter 和 Escape 处理
- `add-connected-canvas-node`

Run:

```powershell
cargo test -p artforge-studio-native infinite_canvas_connection_search
```

Expected: FAIL。

**Step 2: 在空白连接结果时打开搜索**

保存释放点世界坐标和来源 ID。弹窗位置由该世界坐标经过当前 pan/zoom 计算，不执行屏幕边缘翻转；父画布继续允许其视觉超出可视区域约束所允许的范围。

**Step 3: 实现四类节点过滤**

候选为：

- 文本 / Text / prompt
- 图片 / Image / picture
- 视频 / Video / movie
- 音频 / Audio / sound

搜索匹配由 Rust 回调或 Slint 简单关键词映射完成，输入为空时显示全部。上下键循环选择，Enter 创建当前候选。

**Step 4: 原子创建并连接**

新增 Rust 回调：

```slint
callback add-connected-canvas-node(string, string, float, float);
```

在一个事务中：

1. 校验来源和节点上限。
2. 创建节点并定位到释放点。
3. 创建来源到新节点的连线。
4. 选择新节点。
5. 保存一次并记录一条撤销历史。

**Step 5: 测试、编译并提交**

Run:

```powershell
cargo test -p artforge-studio-native infinite_canvas_connection_search
cargo test -p artforge-studio-native canvas_auto_connect
cargo check -p artforge-studio-native
```

Expected: PASS。

```powershell
git add native-client/ui/pages/infinite-canvas-page.slint native-client/ui/app-state.slint native-client/src/runtime/callbacks/infinite_canvas.rs native-client/src/runtime/canvas_ops.rs native-client/src/runtime/tests.rs
git commit -m "feat: add canvas connection node search"
```

### Task 8: 性能防回退、兼容性和完整验收

**Files:**
- Modify: `native-client/src/runtime/canvas_ops.rs`
- Modify: `native-client/src/runtime/tests.rs`
- Modify: `native-client/src/runtime/callbacks/infinite_canvas.rs`
- Modify: `native-client/ui/pages/infinite-canvas-page.slint`
- Inspect: `scripts/package-native-client.ps1`
- Inspect: `scripts/package-macos.sh`

**Step 1: 写规模测试**

构造 200 个节点、400 条合法连线，测试：

- 框选不会重复选择。
- 批量移动不会重复移动分组后代。
- 循环检测终止。
- 快照、复制和分组操作保持数据一致。

测试不使用严格毫秒阈值，避免 CI 抖动；只验证算法在目标规模内完成并得到正确结果。

**Step 2: 验证上限提示和无突变失败**

为第 201 个节点、第 401 条连线、非法循环和无效粘贴写测试，断言操作前后 Store 相等，并向 `generation-status` 写入可读的中英文提示。

**Step 3: 运行格式与静态检查**

Run:

```powershell
cargo fmt --all -- --check
cargo check -p artforge-studio-native
cargo clippy -p artforge-studio-native -- -D warnings
```

Expected: 全部成功，无 warning。

**Step 4: 运行完整测试**

Run:

```powershell
cargo test -p artforge-studio-native
```

Expected: 全部测试通过，测试总数不少于 Task 1 记录的基线。

**Step 5: 构建 release**

Run:

```powershell
cargo build --release -p artforge-studio-native --bin ArtForgeStudio
```

Expected: `target\release\ArtForgeStudio.exe` 成功生成。

**Step 6: 检查跨平台打包脚本**

确认：

- `scripts/package-native-client.ps1` 仍识别 `windows`、`macos-x64`、`macos-arm64`。
- `scripts/package-macos.sh` 仍使用 `x86_64-apple-darwin` 与 `aarch64-apple-darwin`。
- 本轮不在 Windows 上伪造 macOS 构建成功。

**Step 7: 生成 Windows 绿色包**

Run:

```powershell
powershell -ExecutionPolicy Bypass -File scripts/package-native-client.ps1 -Target windows -SkipBuild
```

将生成的 Windows x64 zip 复制到：

```text
D:\ArtForgeStudio
```

输出文件大小与 SHA-256：

```powershell
Get-Item D:\ArtForgeStudio\ArtForgeStudio_*_windows_x64_portable.zip
Get-FileHash D:\ArtForgeStudio\ArtForgeStudio_*_windows_x64_portable.zip -Algorithm SHA256
```

**Step 8: 最终提交**

```powershell
git status --short
git add native-client/src/runtime/canvas_ops.rs native-client/src/runtime/model.rs native-client/src/runtime/mod.rs native-client/src/runtime/callbacks/infinite_canvas.rs native-client/src/runtime/presentation/sync.rs native-client/src/runtime/storage/local_store.rs native-client/src/runtime/tests.rs native-client/ui/app-state.slint native-client/ui/types.slint native-client/ui/pages/infinite-canvas-page.slint
git commit -m "test: verify enhanced infinite canvas workflow"
```

如果没有剩余代码改动，则不创建空提交。最终报告分支提交列表、测试结果、release 路径、绿色包路径和 SHA-256，不自动 push。
