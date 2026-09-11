# Account Center Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the selected native account/team design, without its entire bottom close/footer strip, and complete its existing real functionality and role-sensitive states.

**Architecture:** Retain Rust + Slint and the current server-authoritative team API. Reorganize account-center layout and split team UI into focused Slint components, with narrowly extended presentation/form state and existing callback/worker authority checks. Reuse existing icons/theme, not a web prototype.

**Tech Stack:** Rust 2021, existing Slint 1.17.1, reqwest, existing backend APIs; no new dependency or provider.

**Spec:** /private/tmp/artforge-team-function-audit.0VJiNe/function-inventory.md plus user-approved revised image /Users/fanxiao/.codex/generated_images/01a05b95-a3f1-7001-ac0e-e453d54106fc/exec-525971b0-6c0e-426c-9a2d-b16d2e6f6d20.png

## Global Constraints

- Native desktop production UI; do not scaffold a website, alter backend billing, use Figma, deploy, commit, or push.
- Preserve all pre-existing dirty changes. The entire linked feature/team-accounts worktree is user work, not a clean base.
- Match selected light/violet style through existing AppTheme tokens; continue to support other themes. Remove entire bottom footer and duplicate close button. Keep top-right X and sidebar logout/identity verification.
- Own team is automatically provisioned and unique; at most one joined team. No create-team, third personal wallet, admin role, dissolve, owner transfer or shared files.
- Team membership free and unlimited in member count; credit limits strictly numeric nonnegative integers; zero means zero, not unlimited. Member only sees own quota, never group finance or others' usage. Per-account concurrency unchanged.
- Received invitations are personal/account-level, sent invitations belong to selected owned team. Preserve original account-center destinations and referral-code function.
- Existing auth/session/billing generation captures, permissions, request idempotency, namespace isolation and stale-response rejection must remain intact.
- Use apply_patch. No new std-widgets; use native Flickable or existing custom scrolling patterns for new team components.
- Work on current mock environment only for GUI validation; do not stop API/server or modify production data. Parent controls native GUI restart/QA. Worker does not control the live GUI or spawn agents.
- Security-sensitive/irreversible new scope must stop; ordinary UI input validation and permission rendering are in scope.

### Task 1: Native account-center redesign and complete team interactions

**Files:**
- Modify native-client/ui/dialogs/profile-dialog.slint (shell, dimensions, sidebar, footer removal; preserve login/devices/referral behaviors. Capability-gate overview: member own quota and quota link only, no purchase/recharge/team balance; preserve owner overview behavior. This scope correction is recorded in the ledger.)
- Modify native-client/ui/dialogs/accounts-teams-panel.slint (layout orchestration).
- Create focused native-client/ui/dialogs/team-*.slint components as needed for shared controls, members, invitations, overview and reauth; keep responsibilities clear, no monolith.
- Modify native-client/ui/components/top-bar.slint (actual responsive action widths, no overflow).
- Modify native-client/ui/app-state.slint and native-client/ui/types.slint (minimal presentation/form fields).
- Modify native-client/src/runtime/callbacks/team_accounts.rs; optional child Rust module for presentation/form helpers and tests rather than ballooning this existing file.
- Add behavioral tests in existing team callback test module or focused test module. If a standalone Slint test harness is needed, keep it in tests/dev and reuse production components.
- Adapt only the two obsolete layout source-string tests in native-client/src/runtime/tests.rs to real rendered geometry; preserve all unrelated assertions. This late test-only scope adjustment is recorded in the ledger.

**Interfaces:**
- Consume existing callbacks refresh-account-groups, switch-account-group, rename-team, load-team-members, load-team-invitations, load-pending-team-invitations, load-team-usage, create/resend/revoke/accept/decline-team-invitation, update-team-member, leave-team, request-team-reauth-code, confirm-team-reauth.
- Consume existing AppState capability booleans and account-groups/member/invitation/usage DTOs. Do not infer capabilities from a display label.
- Produce real UI actions with stable target IDs/versions, cancelable scoped form/confirmation state, and complete privacy-correct member quota projection. New fields must be initialized in both prepare_activation_team_projection and render_team_context and cleared on retirement.

- [x] **Step 1: Establish baseline and add failing behavioral coverage before implementation.** Read existing team fixture/setup and run focused baseline tests. Prove meaningful failures for redesigned behavior (e.g. member quota projection does not leak owner balance and uses snapshot quota; selected row actions capture target/version and prefill its amount, cancellation sends no mutation; state cleared after switching). Reuse real UI callback fixture rather than source substring tests. Concrete expectations: numeric limit input `"0"` is accepted as zero, `"-1"`, `"不限"`, empty are rejected; selected member limit500/reserved20/settled80/remaining400 remains strings without float conversion. At least one test must exercise rendered component interaction or its actual production callback/state boundary, not a mock-only test.

```rust
// Extend the existing setup/transport test helpers: execute the real wired callbacks.
// After projecting a member snapshot whose quota is 500/80/20/400:
assert!(!state.get_team_can_read_finance());
assert!(!state.get_team_can_manage_members());
assert!(state.get_account_amount_label().contains("400"));
// Set-limit form must use the row's own amount and member identity, not a shared
// last invitation input; leaving/canceling must never issue the mutation.
```

- [x] **Step 2: Implement layout and page flows.** Shell target about1050–1160 logical pixels wide at1364x928, sidebar~220, header~72, no footer. Use content-driven scroll height and top alignment so forms do not stretch. Preserve five settings destinations. Account/team choices show own/joined group roles, statuses, selection and allowed switch; when only own exists, show explanatory no-joined-team empty state rather than fake account. Keep summary always clear: current group name, role, payer explanation, owner balance with recharge/orders+redeem links OR member numeric quota details and leave action.

```slint
// Member actions must retain server authority and stable identity:
// on a row: open form with member.member-id and member.version;
// confirmation: AppState.update-team-member(saved-id, saved-version, action);
// scope changes/profile closure: clear the saved form, never replay it.
// All scroll areas have explicit viewport/content relationship; no shared footer overlay.
```

- [x] **Step 3: Complete each real flow and state.** Members: masked email/status, numeric monthly/settled/reserved/remaining, per-row quota edit with prefill, suspend/resume/remove with consequence confirmation, actionable status gating and pagination. Invitations sent: email + own numeric limit input, send, delivery/status/expiry, resend with prefilled invitation limit, revoke and pagination; no available mutations on consumed/revoked/superseded entries. Received: count, team/owner/email/limit/expiry, accept/decline and occupied-slot explanation/disabled accept, no automatic team eviction. Usage: real summaries with mapped readable operation/phase/outcome labels and pagination, no creative contents. Reauth: both supported methods, code countdown, enter/confirm, error/success and return; no default reauth. Rename: own-team name with inline edit/save/cancel. Retain purchase/redeem routing and original account-center pages. Provide loading, empty, errors, refresh/retry and successful-action feedback without stale success on another account. Close local forms on successful completion only; failures keep input. UI may keep a scoped form open during mutation, disabling duplicate clicks. Form/pending target cleared on account/billing/profile lifecycle.

- [x] **Step 4: Fix measured toolbar overflow.** Current five logged-in actions total48+94+94+150+128+4*8=546 logical px (read exact widths; do not repeat earlier arithmetic error). Current function502 is wrong. Payment adds112+8. Compute widths from shared component constants/width properties and responsive reductions; preserve nickname and billing entry fully inside window. Compact model pickers or use a second toolbar row on narrow window if necessary; no silently lost required model selector. Test several supported window widths plus active payment and signed-out states. No team switch in toolbar.

- [x] **Step 5: Verify GREEN and build.** Run focused Rust tests, Slint compile/cargo check, then affected behavioral regressions and full library tests once. Build actual GUI executable after source stabilizes. Compiler warnings may pre-exist: report exact new versus pre-existing warnings; do not make an unsupported pristine-output claim. Send parent readiness so it can perform reference-versus-native visual QA and local mock interaction checks.

```sh
cargo test -p artforge-studio-native --lib core_team_ui -- --test-threads=1
cargo check -p artforge-studio-native
cargo test -p artforge-studio-native --lib -- --test-threads=1
cargo build -p artforge-studio-native --bin ElunviCanvas
```

- [x] **Step 6: Self-review and report, no commit.** Report files, TDD RED/GREEN evidence, commands and raw log paths, known limits, final GUI binary path. Parent arranges read-only task review and visual QA, then returns scoped findings if needed. Preserve user changes and reference asset.

## Parent verification and handoff

- [x] Capture pre-edit source snapshot for scoped no-commit review, verify existing isolation.
- [x] Inspect selected revised image; reuse existing logo/icons with no new raster assets needed. Preserve selected reference in workspace docs/design.
- [x] Run read-only task review (spec and quality) using before/after diff, not entire unrelated dirty tree.
- [x] Launch rebuilt native client against existing local mock only; check owner/member/invitation states with disposable mock accounts or test-only harness, not the user's real/production data.
- [x] Compare reference and new native screenshot at matching viewport/state; inspect narrow window and footer removal. Record passed/blocked evidence honestly; correct meaningful overlap/privacy/function issues.
- [x] Final report includes actual completion, test results and any blocked verification. No commit, push, deploy, or unsupported all-platform claim.
