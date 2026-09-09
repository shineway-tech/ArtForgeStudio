# Account center redesign — native QA

Status: passed for this native account/team redesign, with the non-blocking limitation below.

## Visual target and environment

- Approved reference: `account-center-selected.png` in this directory (first corrected design, entire bottom close/footer strip removed).
- Product: existing Rust + Slint native desktop client, not a web implementation.
- Comparison target: 1364 × 928 logical full native window; test-component harness uses 1364 × 900 content pixels (28-pixel OS titlebar excluded).
- Native component harness imports the actual ProfileDialog and TopBar; shell background and presentation data are test-only. A harness render does not prove authenticated runtime callbacks or backend integration.
- State for reference comparison: owner of 林间设计团队, joined 星河工作室 as member, owner balance 2,000, two member rows 小雨 (500/80/20/400) and 阿青 (300/0/0/300, suspended).
- Reference and rendered screenshot must be inspected together in one comparison input after implementation.

## Existing failure evidence

- `/private/tmp/artforge-team-function-audit.0VJiNe/preview-current.png`: pre-change production component render, captured before implementation. Bottom close/logout controls visibly overlay the second member row. Layout is dense and vertical, unlike selected design.
- First ElementHandle run was invalid because Slint debug metadata was absent. This is a harness setup issue, not product regression evidence. Rebuild uses `with_debug_info(true)`.
- Corrected pre-change harness then produced a genuine geometry RED (exit 101): at 1364 × 900, TopBar right edge = 1364, last PillButton right edge = 1390 (26 pixels outside). Test asserts all actions remain within toolbar. Source comes from pre-edit snapshot, with copied original assets.
- Pre-change rendered member interaction also produced genuine RED (exit 101): clicking first row “设置额度” leaves the shared amount at stale `999` instead of row amount `500`, and immediately emits one update-team-member callback (expected zero when opening a cancelable edit form). Repro binary: `/private/tmp/artforge-team-function-audit.0VJiNe/preview-interaction-red`; source archived as `preview-red.rs` and `preview-red.slint` in the same directory.

## Functional evidence (separate from visual QA)

- Local mock contract script `/private/tmp/artforge-team-function-audit.0VJiNe/verify-team-contract.cjs`, final run exit 0: rename, invite/revoke/decline/accept, set-limit, suspend/resume/remove, reauth, own-plus-one joined-team restriction, zero-is-zero, member finance privacy, second-team rejection without eviction, leave retaining owned group.
- Mock client version 1.0.22 was needed; initial runs with helper default 1.0.18 were rejected by required upgrade. No server source/configuration changed.
- Three disposable local mock accounts were used; their sessions logged out afterward. No production endpoint or existing user-account mutation.

## Completion checks

- [x] Layout/composition: first design hierarchy, compact sidebar, horizontal account choices, owner/member separation, entire footer absent.
- [x] Spacing/geometry: matching modal and header proportions, scrollable long content, no action overlay; toolbar edge checks pass at 1364 and 1180 (full login/payment matrix is separately covered by runtime tests).
- [x] Typography: readable Chinese, consistent labels/statuses and numeric alignment, no truncation of critical controls.
- [x] Color/surfaces: existing theme tokens and assets, selected violet accents, restrained borders, other themes remain supported.
- [x] Interaction/content: real callbacks, member privacy, invite limits/statuses, scoped cancelable forms, pending/loading/empty/error/success states, reauth return flow and retained account destinations.
- [x] Rendered member, sent/received invitations, usage, reauth, account overview/login/devices/referral states inspected.
- [x] Focused behavioral tests and affected full suite/build have fresh logs.
- [x] Task review and final integration review findings resolved or explicitly reported.
- [x] Actual native binary launch against local mock verified, or precise launch/GUI limitation disclosed.

## Comparison history

- Intermediate shell-only render `/private/tmp/artforge-team-function-audit.0VJiNe/preview-shell-intermediate.png` uses new shell/topbar but old team panel (compiler captured before panel replacement). Exit 0: toolbar edge assertion passes at 1364 width. Footer strip removed and sidebar logout visible. This is not final reference comparison; account/team body still old in this intermediate capture.
- First complete-panel comparison: approved image and `/private/tmp/artforge-team-function-audit.0VJiNe/iteration1-owner.png` opened together in the same comparison input. [P1] Table header was centered vertically and overlapped the first member row; row actions also sat above numeric row values. Assigned fix: explicit y:0 for header, center row actions, cover geometry with rendered-element assertions. Result remains blocked pending recapture.
- Member quota, sent/received invitations, reauth, quota form, overview, login methods, devices-empty and referral form were rendered and inspected. No additional major clipping found at 1364 × 900. Chinese-first localization pass remains in progress.
- Narrow-window first capture rejected as invalid: harness declared fixed width1364 so setting renderer width1180 cropped content without resizing root. Changed to preferred-width/preferred-height; final narrow verification must rerun. Earlier1180-only edge assertion is not accepted evidence.
- First usage/expiry capture also used raw DTO-like strings directly in presentation fixture; production Rust mapping already formats them. Corrected fixture to presentation text rather than classifying raw fixture labels as a product defect.
- Second comparison: member header now starts above the first row, actions align with numeric values; prior P1 resolved. Corrected 1180 × 900 root sizing asserted (actual toolbar right edge equals requested width), model controls move to second row and account dialog remains wholly inside window. Member account overview shows team and quota400 with no plan/recharge or team balance. [P2] English sidebar/action labels clipped and table/action labels crowded; assigned short labels and language-aware widths/gaps.
- Third render: `/private/tmp/artforge-team-function-audit.0VJiNe/preview-english.png` shows Teams/Devices, Credits / Orders, Redeem, Held/Left and separated Suspend/Resume/Remove; prior English P2 resolved. Owner/member/overview/form/dark/narrow renders were regenerated and edge assertions exit0. Final selected-source comparison and post-conflict-fix form check remain before handoff.
- Theme fixture corrected to exact existing runtime palette values: light plus ocean dark theme, rather than arbitrary dark colors. Native font PingFang SC; no new art or substitute logo assets.

## Fidelity assessment at latest render

- Typography: headings, sidebar, table names and numeric columns are readable; English core actions no longer elide. User-provided team names and some runtime-generated labels are separate from static localization. Existing shared English model/theme trigger abbreviations remain outside this team's new component content.
- Spacing/layout: selected hierarchy retained (choices → payer/amount → tabs/actions → table); wider1120 native modal preserves all content. Entire footer removed, X/sidebar logout retained. Native shared pill button shape and slightly roomier table rows are intentional reuse of existing components.
- Colors/tokens: real AppTheme light/ocean palettes drive surfaces, accents and semantic colors. No hardcoded light backgrounds in new team panels.
- Assets: existing logo/account icons used; simple native radio selection controls are actual UI, not substitute artwork. No new raster illustration required.
- Copy/content: own-plus-one teams, zero-as-zero, occupied invitation slot, member-only quota and consequences are explicit. No third wallet, create-team, unlimited-credit or automatic eviction affordance introduced.
- Functional completion is separate: task review found two conflict-recovery issues, assigned to implementer. Full fresh suite/binary still pending; screenshots do not certify those behaviors.

## Final assessment

Fourth comparison: approved reference and latest owner render opened together, followed by compact1180 × 760, long-list scrolled-to-bottom, quota conflict, and inline rename conflict. All 23 fixture modes completed with exit0. Entire footer remains absent; no blocking visual mismatch remains in inspected states. Long content is intentionally scrollable, including when inline rename/error increases height; the actual pointer-scroll event brings row20 and pagination fully into view. Conflict forms preserve visible typed values, display the confirmed version and explicit review action, and visibly disable submission before review.

Visual assessment passes for inspected states. The native first-click FocusScope interception was fixed and verified with both the production-component click harness and the actual app test. Final focused32/0 and full1332/0/55 ignored/0 filtered passed. Default GUI build completed; actual native launch and navigation were verified.

## Final verification and native handoff

- Focused: `/private/tmp/account-center-final-focused.log`,32 passed0 failed.
- Final unfiltered full library: `/private/tmp/account-center-final-canonical-full.log`,1332 passed0 failed55 ignored0 filtered,108.74s,exit0. Temporary test app profile overrides(opt0/debug0) avoid expensive optimization; production build uses normal dev profile.
- Initial full1299/33/55 failures were investigated, not ignored:31 filesystem tests failed with the environment's symlinked `/var` temporary path; same executable passes with `TMPDIR=/private/tmp`, without weakening directory authority or changing HOME. Two obsolete layout-source assertions were replaced by actual rendered geometry; each exact test and the final unfiltered suite passed.
- Default `SLINT_EMIT_DEBUG_INFO=1 cargo build -p artforge-studio-native --bin ElunviCanvas`: exit0,17m52s,291 existing warnings. Log `/private/tmp/account-center-final-build.log`; no new warning categories found by implementer comparison.
- Binary: `target/debug/ElunviCanvas`,SHA256 `94b68b0294929361e93edcf74abb7a9ad2fa35c71cacf6d695e438636a3f7780`.
- Actual new process22654/window4892 at1440 × 928 launched with local API override127.0.0.1:39091. The native startup shows both model selectors and complete account actions. OS navigation opened login methods, then account/team view, then invite form, and canceled it without sending an invitation. Left the client open on account/team view. Native screenshots: `account-center-native.png` and `account-center-native-invite.png`.
- Initial per-PID synthetic clicks did not navigate; corrected foreground/HID event delivery succeeded and was screenshot-verified. Rejected no-op captures are not navigation evidence.
- Final overall review and late tests-only re-review: no Critical/Important; technical verification gate accepted. Scoped diff whitespace check passed.
- Known non-blocking limitation: closing/reopening the account panel during same-scope conflict refresh can re-display the old page error. It does not restore targets, replay operations or affect billing authority. Deferred rather than extending this redesign.
- Scope of evidence: macOS native launch/navigation, actual-component state matrix and Rust/backend contract tests. Not a claim of Windows QA, production rollout, or every mutation being manually exercised through the live UI. No production data, server source, unrelated user changes, commits or pushes were made.

final result: passed
