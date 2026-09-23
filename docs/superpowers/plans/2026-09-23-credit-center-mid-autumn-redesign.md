# Credit Center Mid-Autumn Promotion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task.

**Goal:** 在现有 ArtForgeStudio 积分中心内，将积分明细提升为独立顶层 Tab，并在充值 Tab 内加入服务端控制的中秋充值加赠活动。活动截止到北京时间 2026 年 9 月 30 日 23:59:59；结束后新订单自动恢复现有积分和价格，无需再次发布客户端、重启服务或手工改配置。会员充值折扣继续生效，活动赠送不参与折扣计算。

**Architecture:** 服务端新增版本化的活动与档位规则表。充值目录接口在活动时间窗内返回加赠投影，订单创建在事务内重新计算会员折扣和加赠并保存不可变快照，履约只读取快照并沿用当前单一充值 lot 与幂等链路。积分流水和订单视图提供安全的活动摘要，客户端只展示服务端字段，不自行判断日期。客户端保留当前积分页外壳和组件风格，顶层 Tab 调整为“充值 / 积分明细 / 兑换码 / 订阅”，充值卡整卡可点击但不显示“选择”按钮，充值页只保留一个“立即充值”主操作。

**Tech Stack:** Node.js 24、Koa、Sequelize、MySQL、现有订单/支付/积分履约逻辑、OpenAPI YAML；Rust 2021、Slint 1.17.1、Serde、现有 ArtForgeStudio native-client projection/state tests。

**Spec:** docs/superpowers/specs/2026-09-23-credit-center-mid-autumn-redesign.md

## Global Constraints

- 活动时间必须使用半开区间 [starts_at, ends_at)；结束边界固定为 2026-10-01T00:00:00+08:00（UTC 2026-09-30T16:00:00.000Z）。客户端不得根据本地日期判断活动是否有效。
- credits 始终表示基础充值积分；payable_price_cents 始终表示会员折后实付金额。活动只增加到账积分，不改变原价、会员折扣或支付金额。
- 客户端提交的 bonus、total、promotion 或价格字段全部不可信；服务端在创建订单事务内按当前会员状态、充值包版本和活动规则重算并写入快照。
- 活动订单即使在活动结束后才支付，也必须按订单快照发放；支付回调不得重新读取当前活动。
- 履约保持一个充值 lot，source_id 和幂等键保持现有语义；metadata 记录基础/赠送/总积分和活动信息，保证团队账务校验与重试行为不变。
- 邀请返利按基础积分计算，并把 referral_basis_credits 写入订单快照；旧订单没有该字段时才回退到旧的 credit_amount。
- 旧客户端、旧订单和无活动响应必须继续可用。新增 OpenAPI 字段应为可选，除非它们是新响应的稳定必需字段。
- 不引入活动结束 cron、客户端发版后手工回滚、人工修改余额或无审计的充值冲正。
- 不改动现有未跟踪文档 docs/superpowers/plans/2026-09-11-current-account-directory-migration.md 和 docs/superpowers/specs/2026-09-11-current-account-directory-migration.md。

## Review Focus

- 9 月 30 日 23:59:59.999（北京时间）创建的订单有赠送，10 月 1 日 00:00:00（北京时间）创建的新订单无赠送。
- 会员 8.5 折的 10,000 档必须显示基础 10,000、赠送 2,000、到账 12,000、实付 ¥85。
- 活动结束后无需客户端更新，充值目录自然回到基础四档；活动结束前创建、结束后支付的订单仍到账总积分。
- 重复支付通知、履约重试、团队 owner 账组和邀请返利均保持幂等与原有边界。
- 充值卡没有每卡“选择”按钮或“已选择”文字，整卡点击和边框/check 状态仍清晰；页面只保留一个“立即充值”按钮。
- 积分明细在独立 Tab 运行原有分页/空态/错误处理，充值 Tab 不再在底部重复渲染明细。
- 老服务端 JSON 缺活动字段时客户端回到普通充值样式；活动订单在活动结束后仍能正确识别发票和原始充值积分。

## Task 1: Add versioned promotion schema, migration, and seed

**Files:**
- Create server/artforge-api/database/schemas/credit_promotions.js
- Modify server/artforge-api/database/runtime_schema.js
- Create server/artforge-api/migrations/20260923000200-create-credit-promotions.js
- Create server/artforge-api/seeders/20260923000300-seed-mid-autumn-credit-promotion.js
- Add assertions in server/artforge-api/test/credit_promotions.test.js

**Interfaces and data shape:**
- credit_promotions: public code, integer version, title, subtitle, copy snapshot, status, starts_at, ends_at, config_sha256, timestamps.
- credit_promotion_rules: promotion_id, pack_code, pack_version, bonus_type (fixed or bps), bonus_value, label, display_order, timestamps.
- Seed code/version is mid_autumn_2026/v1, with four fixed bonuses: 100, 750, 2,000, 7,500 for the 1,000, 5,000, 10,000, 30,000 packs. End is 2026-09-30T16:00:00.000Z; start is the approved release start instant recorded by the seed.
- Add unique constraints for promotion code/version and promotion/pack/version, indexes for active status/time range and pack lookup, and a deterministic hash over copy/rules.
- Keep timestamps UTC and use the existing project schema conventions, foreign keys, and migration transaction style.

**Steps:**
- [ ] Inspect the existing pack schema/version source and runtime schema registration; rules must refer to the same immutable pack version used by the catalog.
- [ ] Implement both schemas with strict types, public UUIDs, decimal strings/BigInt-compatible values, indexes, and associations following neighboring credit/membership schemas.
- [ ] Implement reversible migration creation and down behavior without deleting historical order snapshots.
- [ ] Seed one active version with server-side copy and four rules. Make the seed idempotent and hash-stable.
- [ ] Add tests for uniqueness, pack-version matching, UTC conversion, and end boundary.

**Test command:**
  cd /Users/fanxiao/workstation/ai/ArtForgeStudio/server/artforge-api
  node --test --test-concurrency=1 test/credit_promotions.test.js

**Commit:** feat: add versioned credit promotion schema

## Task 2: Evaluate promotion rules in the credit catalog and publish the contract

**Files:**
- Create server/artforge-api/src/logics/credit_promotions.js
- Modify server/artforge-api/src/logics/credits.js
- Modify server/artforge-api/docs/openapi.yaml
- Modify/add server/artforge-api/test/credit_promotions.test.js, test/selected_catalog_openapi.test.js, and catalog tests

**Interfaces and data shape:**
- Implement getActivePromotion(now, transaction), selecting the highest active version where starts_at <= now < ends_at.
- Implement evaluatePack(pack, promotion, now), returning string-safe bonus_credits, total_credits, promotion_id, promotion_version, promotion_label, promotion_ends_at, and optional promotion_deadline_label.
- Match by pack code plus immutable pack version/config hash. Invalid or stale rules must return the base pack with zero bonus.
- Extend GET /v1/credits/packs while preserving existing fields and membership price calculation. During inactivity, return zero/empty promotion fields and total_credits equal to credits.
- Update SelectedCreditPack OpenAPI fields and examples. New fields remain optional for old-client compatibility; preserve strict envelopes and decimal-string/date-time patterns.
- Keep the API response independent of client locale; any deadline label is a server-provided stable Chinese display string.

**Steps:**
- [ ] Add a clock-injectable evaluator and tests for before-start, exact-start, just-before-end, exact-end, inactive status, newer-version selection, and pack-version mismatch.
- [ ] Use integer arithmetic for fixed bonuses and deterministic rounding for any retained bps rule; never use floating point.
- [ ] Refactor credits.listPacks to attach promotion projection after membership discount calculation without changing base field meaning.
- [ ] Update OpenAPI and strict response tests for active and inactive catalog payloads.
- [ ] Add a post-end catalog test proving a fresh request returns original prices and base credits without a follow-up config change.

**Test command:**
  cd /Users/fanxiao/workstation/ai/ArtForgeStudio/server/artforge-api
  node --test --test-concurrency=1 test/credit_promotions.test.js test/selected_catalog_openapi.test.js test/core.test.js

**Commit:** feat: expose server-authoritative credit promotion catalog

## Task 3: Snapshot promotion, membership discount, fulfillment, referrals, and history

**Files:**
- Modify server/artforge-api/src/logics/order_creation.js
- Modify server/artforge-api/src/logics/order_fulfillment.js
- Modify server/artforge-api/src/logics/referral_rewards.js or the existing referral module
- Modify server/artforge-api/src/logics/order_view.js
- Modify credit ledger projection logic used by src/logics/credits.js
- Modify server/artforge-api/docs/openapi.yaml
- Add/update order_fulfillment_snapshot.test.js, payment_checkout.test.js, referrals.test.js, team_billing_orders.test.js, and a focused ledger/history test

**Interfaces and data shape:**
- Extend the order item snapshot with pack_code, pack_version, base_credits, bonus_credits, total_credits, referral_basis_credits, promotion_id, promotion_version, promotion_starts_at, promotion_ends_at, promotion_config_sha256, membership plan/tier/discount, original/discount/payable amounts, and pricing snapshot version.
- Add optional credit_recharge to TeamOrder with base_credits, bonus_credits, total_credits, pack_code, promotion_id, promotion_version, promotion_ends_at.
- Add optional ledger projection fields for recharge entries: base_credits, bonus_credits, total_credits, price_cents, promotion_id. Derive them from lot metadata/order snapshot, never from the current catalog.
- Continue setting OrderItem.credit_amount to total_credits so existing balance/reconciliation code remains single-lot and integer-safe.

**Steps:**
- [ ] In the existing order transaction, lock/read selected pack version, membership entitlement, and active promotion; calculate base/bonus/total and discounted payable amount together.
- [ ] Persist the complete JSON snapshot on the order item. Retries for the same idempotency key must return the original snapshot.
- [ ] Keep payment checkout amount unchanged by bonus credits. Payment callbacks must call fulfillment with the saved order item only.
- [ ] Update fulfillment metadata and description to include “中秋充值加赠” when bonus_credits > 0; preserve one recharge lot, source id, account-group ownership, balance conservation, and idempotency.
- [ ] Change referral reward calculation to prefer referral_basis_credits, then base_credits, then legacy credit_amount; test that bonus credits do not increase inviter rewards.
- [ ] Extend order presentation and ledger list responses with safe optional summary fields, retaining old shapes for non-recharge entries.
- [ ] Add OpenAPI schemas and strict envelope tests for order and ledger summaries.

**Test command:**
  cd /Users/fanxiao/workstation/ai/ArtForgeStudio/server/artforge-api
  node --test --test-concurrency=1 test/order_fulfillment_snapshot.test.js test/payment_checkout.test.js test/referrals.test.js test/team_billing_orders.test.js test/core.test.js

**Commit:** feat: snapshot and fulfill credit promotion orders

## Task 4: Project promotion data into the native client API and state

**Files:**
- Modify ArtForgeStudio/native-client/src/runtime/api/payment.rs
- Modify ArtForgeStudio/native-client/src/runtime/api/account.rs
- Modify ArtForgeStudio/native-client/src/runtime/callbacks/auth.rs
- Modify ArtForgeStudio/native-client/src/runtime/callbacks/credits.rs
- Modify ArtForgeStudio/native-client/src/runtime/callbacks/payment.rs
- Modify ArtForgeStudio/native-client/ui/types.slint
- Modify ArtForgeStudio/native-client/ui/app-state.slint

**Interfaces and data shape:**
- Add serde-default optional fields to CreditPack and CreditPackView: bonus_credits, total_credits, promotion_id, promotion_label, promotion_ends_at, and optional copy/deadline fields. Preserve credits as base credits and fall back to total_credits equal to credits when absent.
- Add optional recharge summary to CreditLedgerItem: base_credits, bonus_credits, total_credits, price_cents, promotion_id. Serde defaults must allow old ledger JSON.
- Add AppState properties for selected bonus/total and promotion active/title/description/label/ends-at. Clear them whenever account snapshot, pack refresh, logout, or reset has no active promotion.
- Derive Hero state from the first non-empty server promotion metadata, never from pack index or local date.
- Make PaymentPresentation::credit accept server order total or recharge summary so success feedback shows actual total received.

**Steps:**
- [ ] Update both existing CreditPack projection sites in auth.rs, including defaults for old JSON and clear/reset behavior.
- [ ] Update ledger decoding and history mapping; prefer server base/price metadata in invoice_order, with legacy current-pack matching only as fallback.
- [ ] Update payment success presentation to display total credits from the order snapshot, with a safe fallback for old responses.
- [ ] Add projection tests for active, inactive, missing-field, and old-server payloads; assert no panic when credit-packs is empty.
- [ ] Run Rust formatting before committing.

**Test command:**
  cd /Users/fanxiao/workstation/ai/ArtForgeStudio/ArtForgeStudio
  cargo fmt --all -- --check
  cargo test -p artforge-studio-native --lib runtime::callbacks::credits runtime::callbacks::payment runtime::tests

**Commit:** feat: project credit promotion data into native client

## Task 5: Rebuild the credits page hierarchy and Mid-Autumn campaign presentation

**Files:**
- Modify ArtForgeStudio/native-client/ui/pages/credits-page.slint
- Modify ArtForgeStudio/native-client/ui/components/credit-plan.slint
- Modify ArtForgeStudio/native-client/ui/app-state.slint if layout properties need adjustment
- Create ArtForgeStudio/native-client/ui/components/credit-balance-summary.slint only if the existing balance card cannot support two-value summary
- Add/update page structure assertions in ArtForgeStudio/native-client/src/runtime/tests.rs

**Interfaces and behavior:**
- Top tabs are exactly recharge, ledger, redeem, membership, displayed as “充值 / 积分明细 / 兑换码 / 订阅”.
- Recharge order: balance summary, conditional Mid-Autumn Hero, four pack cards, agreement text, one bottom “立即充值” CTA, catalog note.
- Ledger owns the existing CreditLedgerSection, with available/frozen summary above it; recharge must not render a second ledger section.
- Hero is conditional on server-provided promotion active state and uses existing Slint primitives: deep indigo background, gold moon, sparse stars, lantern accents, low-density rabbit line art. It contains title/copy/deadline and a rules link; it does not duplicate the purchase action.
- CreditPlan remains a full-card TouchArea. Remove per-card “选择”/“已选择” text and button; show selected state only through border, light fill, and compact check marker. The page-level CTA is the sole purchase action.
- Each card shows base credits, bonus credits when positive, total credits, member discount/final payable amount, and existing names/notes. When bonus is zero, hide bonus copy and keep ordinary recharge styling.
- Fix content-height and scrolling calculations per active tab so recharge does not reserve ledger rows and ledger keeps pagination/empty/error states.

**Steps:**
- [ ] Replace tab conditions while preserving current tab button styling and localization conventions.
- [ ] Move CreditLedgerSection from recharge into the ledger branch and add available/frozen summary.
- [ ] Add Hero using existing fonts/colors/components and server-driven strings; do not hardcode activity dates or perform locale date math.
- [ ] Refactor pack cards to render server values and preserve full-card click selection; remove all “选择” and “已选择” text from component and page.
- [ ] Keep redeem/subscription contents unchanged apart from their relationship to the new tab order.
- [ ] Add structure tests for tab order, ledger ownership, Hero conditional rendering, absent-activity fallback, no selection button/text, and single CTA.

**Test command:**
  cd /Users/fanxiao/workstation/ai/ArtForgeStudio/ArtForgeStudio
  cargo test -p artforge-studio-native --lib runtime::tests::credits

**Commit:** feat: redesign credits page and mid-autumn hero

## Task 6: Verify invoice/history compatibility, UI interaction, and cross-stack contracts

**Files:**
- Update focused client tests in ArtForgeStudio/native-client/src/runtime/tests.rs, callbacks/credits.rs, and callbacks/payment.rs
- Update server tests in server/artforge-api/test/selected_catalog_openapi.test.js, test/credit_promotions.test.js, and order/ledger tests
- Update docs/openapi.yaml only when tests expose a contract mismatch

**Steps:**
- [ ] Test a recharge ledger row whose total delta is 12,000 but base snapshot is 10,000; invoice matching must use the base snapshot and original price.
- [ ] Test an old ledger row without promotion metadata; invoice matching must preserve the legacy path.
- [ ] Test active promotion plus 8.5折 membership projection in UI state and payment success message.
- [ ] Test inactive promotion response with zero/empty fields; Hero, bonus labels, and promotion copy disappear while ordinary cards remain usable.
- [ ] Test full-card pack click/check marker and absence of per-card selection controls through page source assertions and, where available, Slint testing backend interaction.
- [ ] Run API OpenAPI/selected-catalog tests and native client tests from clean working trees, excluding the two pre-existing account-directory docs.

**Test commands:**
  cd /Users/fanxiao/workstation/ai/ArtForgeStudio/server/artforge-api
  npm test -- --test-name-pattern='credit|catalog|order|referral|ledger|team'
  npm run lint

  cd /Users/fanxiao/workstation/ai/ArtForgeStudio/ArtForgeStudio
  cargo fmt --all -- --check
  cargo check -p artforge-studio-native
  cargo test -p artforge-studio-native --lib

**Commit:** test: verify credit center promotion compatibility

## Final verification and release checklist

- [ ] git diff --check passes in both repositories.
- [ ] Server targeted tests, full test suite, and lint pass; MySQL-backed tests run when configured.
- [ ] Native client formatting, check, and library tests pass.
- [ ] OpenAPI selected catalog, team order, and ledger schemas match actual JSON and maintain strict envelopes.
- [ ] Migration/seed applies once in a clean database and is idempotent on a second seed attempt.
- [ ] Fresh post-end catalog request has no bonus and unchanged prices without another deployment.
- [ ] An order created before the end and paid after the end fulfills the snapshotted total once.
- [ ] Team owner restrictions, balance conservation, and referral basis remain unchanged.
- [ ] The page has one recharge CTA, no card-level selection button/text, independent ledger Tab, and no Hero when activity fields are absent.
- [ ] Record migration/seed version and promotion config hash in release notes; rollback must not alter historical snapshots.
