# Credit Ledger Pagination Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 为积分明细增加后端游标分页，并把技术流水转换为用户可理解的中文标题、金额、时间和说明。

**Architecture:** `AccountApi` 返回带 `next_cursor` 的分页对象，运行时 Store 记录当前页与每页起始游标，独立 `credits` 回调模块负责翻页和流水展示模型转换。Slint 页面只渲染状态和触发上一页、下一页回调。

**Tech Stack:** Rust 2021、Slint 1.8、reqwest、chrono、现有 Koa `/v1/credits/ledger` 游标接口。

## Global Constraints

- 每页固定显示 8 条流水。
- 不修改后端分页协议，不引入新的依赖。
- 不显示 `generation_task`、RFC 3339 原始时间或“结算 0 积分”。
- 翻页失败时保留当前页；加载期间禁用分页按钮。
- 保留用户当前工作区改动，不提交 Git。

---

### Task 1: 流水语义转换

**Files:**
- Create: `native-client/src/runtime/callbacks/credits.rs`
- Modify: `native-client/src/runtime/mod.rs`
- Modify: `native-client/ui/types.slint`
- Modify: `native-client/ui/components/credit-record-row.slint`

**Interfaces:**
- Consumes: `CreditLedgerItem`、chrono `Local`、Slint `CreditRecord`。
- Produces: `fn credit_record(item: &CreditLedgerItem) -> CreditRecord`，以及 `CreditRecord.tone: string`。

- [ ] **Step 1: Write failing formatter tests**

在 `credits.rs` 先写冻结、结算、退回和未知类型测试，直接断言用户可见字段：

```rust
#[test]
fn commit_uses_reserved_delta_instead_of_zero_available_delta() {
    let record = credit_record(&ledger_item("commit", "0", "-50", "generation_task"));
    assert_eq!(record.title.as_str(), "AI 创作积分已扣除");
    assert_eq!(record.amount.as_str(), "扣除 50");
    assert!(!record.note.as_str().contains("generation_task"));
}
```

- [ ] **Step 2: Run the formatter test and verify RED**

Run: `cargo test -p artforge-studio-native credit_callbacks::tests --offline`

Expected: FAIL because `credit_record` and the credit callback module implementation do not exist yet.

- [ ] **Step 3: Implement the minimal formatter**

按 `entry_type` 计算标题、金额和 `tone`，按 `business_type` 返回中文来源，并把时间转换成本地时间：

```rust
fn format_ledger_time(value: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|time| time.with_timezone(&Local).format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|_| value.to_string())
}
```

`commit` 使用 `reserved_delta` 绝对值；`reserve` 使用 `available_delta` 绝对值；`release` 使用正向 `available_delta`。说明末尾增加“可用积分余额 X”。

- [ ] **Step 4: Replace boolean amount color with semantic tone**

将 `CreditRecord.positive` 改成 `CreditRecord.tone`，行组件按 `positive`、`negative`、`neutral` 显示绿色、红色、中性色。

- [ ] **Step 5: Run formatter tests and verify GREEN**

Run: `cargo test -p artforge-studio-native credit_callbacks::tests --offline`

Expected: all formatter tests PASS.

---

### Task 2: API 游标分页

**Files:**
- Modify: `native-client/src/runtime/api/account.rs`

**Interfaces:**
- Consumes: `ApiResponse.meta.next_cursor` 和服务端 `cursor`、`limit` 查询参数。
- Produces: `CreditLedgerPage { items: Vec<CreditLedgerItem>, next_cursor: Option<String> }` 和 `AccountApi::ledger_page(cursor: Option<&str>, limit: usize)`。

- [ ] **Step 1: Write a failing response conversion test**

增加纯函数测试，给定流水数据和 `ApiMeta { next_cursor: Some("42") }` 时，分页结果必须保留游标；没有 meta 时游标为 `None`。

- [ ] **Step 2: Run API tests and verify RED**

Run: `cargo test -p artforge-studio-native ledger_page --offline`

Expected: FAIL because the page type/conversion does not exist.

- [ ] **Step 3: Implement ledger page request**

```rust
pub(crate) const CREDIT_LEDGER_PAGE_SIZE: usize = 8;

pub(crate) fn ledger_page(
    &self,
    cursor: Option<&str>,
    limit: usize,
) -> Result<CreditLedgerPage, ApiError>
```

第一页请求 `/v1/credits/ledger?limit=8`，后续页追加 `&cursor=<id>`。`snapshot()` 使用该方法并把第一页 `next_cursor` 写入 `BackendSnapshot.ledger_next_cursor`。

- [ ] **Step 4: Run API tests and verify GREEN**

Run: `cargo test -p artforge-studio-native ledger_page --offline`

Expected: pagination response tests PASS.

---

### Task 3: 分页运行时状态与回调

**Files:**
- Modify: `native-client/src/runtime/model.rs`
- Modify: `native-client/src/runtime/callbacks/credits.rs`
- Modify: `native-client/src/runtime/callbacks/auth.rs`
- Modify: `native-client/src/runtime/app.rs`
- Modify: `native-client/ui/app-state.slint`

**Interfaces:**
- Consumes: `AccountApi::ledger_page`、`BackendSnapshot.ledger_next_cursor`。
- Produces: `reset_credit_ledger`、`wire_credit_callbacks`、AppState 的分页属性和回调。

- [ ] **Step 1: Add a failing cursor history test**

测试第一页游标为 `None`、进入下一页时保存起始游标、返回上一页时取回原游标，并断言第一页不可后退。

- [ ] **Step 2: Run cursor state tests and verify RED**

Run: `cargo test -p artforge-studio-native credit_callbacks::tests --offline`

Expected: FAIL because cursor state transitions are missing.

- [ ] **Step 3: Implement pagination state**

Store 新增当前页索引、页面起始游标列表和当前页的下一游标。AppState 新增：

```slint
in-out property <int> credit-ledger-page: 1;
in-out property <bool> credit-ledger-has-previous: false;
in-out property <bool> credit-ledger-has-next: false;
in-out property <bool> credit-ledger-loading: false;
in-out property <string> credit-ledger-message: "";
callback credit-ledger-previous-page();
callback credit-ledger-next-page();
```

- [ ] **Step 4: Implement asynchronous page callbacks**

下一页使用当前 `next_cursor`，上一页使用 Store 中目标页的起始游标。后台线程请求数据，Slint 定时器轮询结果；成功后一次更新列表、页码和按钮状态，失败只更新错误消息。

- [ ] **Step 5: Reset pagination when applying account snapshot**

`apply_backend_snapshot` 调用 `reset_credit_ledger`，使登录、充值完成和重新进入积分页时统一回到最新的第一页。

- [ ] **Step 6: Run cursor tests and verify GREEN**

Run: `cargo test -p artforge-studio-native credit_callbacks::tests --offline`

Expected: formatter and cursor state tests all PASS.

---

### Task 4: 分页页面

**Files:**
- Modify: `native-client/ui/pages/credits-page.slint`

**Interfaces:**
- Consumes: AppState 分页属性和上一页、下一页回调。
- Produces: 页面底部可操作的分页栏和加载/错误反馈。

- [ ] **Step 1: Add pagination controls**

在积分记录列表之后增加 48px 高分页栏，左右使用 `PillButton`，中间显示“第 N 页”；加载时显示“加载中…”。按钮禁用条件同时包含没有对应页面和正在加载。

- [ ] **Step 2: Add non-destructive error feedback**

`credit-ledger-message` 非空时在分页栏上方显示红色提示，不覆盖或清空当前列表。

- [ ] **Step 3: Adjust scroll content height**

为分页栏和提示信息预留高度，保证第 8 条记录及分页按钮可滚动到完整可见位置。

- [ ] **Step 4: Compile-check Slint bindings**

Run: `cargo check -p artforge-studio-native --offline`

Expected: exit 0 with no Slint property/callback/type errors.

---

### Task 5: 完整验证与本地启动

**Files:**
- Verify only; no additional production files unless verification finds a defect.

**Interfaces:**
- Consumes: completed feature.
- Produces: fresh test, compile and launch evidence.

- [ ] **Step 1: Run the native client test suite**

Run: `cargo test -p artforge-studio-native --offline`

Expected: exit 0, all tests PASS.

- [ ] **Step 2: Run compiler verification**

Run: `cargo check -p artforge-studio-native --offline`

Expected: exit 0.

- [ ] **Step 3: Review the scoped diff**

Run: `git diff --check && git diff -- native-client/src/runtime/api/account.rs native-client/src/runtime/model.rs native-client/src/runtime/mod.rs native-client/src/runtime/app.rs native-client/src/runtime/callbacks/auth.rs native-client/src/runtime/callbacks/credits.rs native-client/ui/app-state.slint native-client/ui/types.slint native-client/ui/components/credit-record-row.slint native-client/ui/pages/credits-page.slint`

Expected: no whitespace errors; diff contains only pagination and readable-ledger changes.

- [ ] **Step 4: Restart the local client**

Stop the existing client process, then run the existing development launch command so it continues to call the local backend. Confirm the process remains running and the credits page can request data without startup errors.
