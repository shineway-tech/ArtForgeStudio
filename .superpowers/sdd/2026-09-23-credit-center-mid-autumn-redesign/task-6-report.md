# Task 6 compatibility verification

## Scope

Added focused cross-stack compatibility coverage for the Mid-Autumn credit promotion:

- Native invoice matching uses `base_credits` and the original `price_cents` when a recharge delta includes a bonus (12,000 total / 10,000 base).
- Legacy recharge ledger rows without promotion metadata continue matching by the historical delta amount.
- Active promotion projection preserves membership discounted pricing, bonus/total credits, and promotion metadata; empty promotion fields retain ordinary pack pricing and totals.
- Credit pack source checks require one full-card `TouchArea`, no per-card button or selection copy, and one recharge CTA.
- Server OpenAPI order projection validates the optional credit recharge base/bonus/total snapshot and rejects unknown internal fields.

## Verification

- Server selected catalog, promotion, and order/ledger focused tests: 17 passed, 1 MySQL integration skipped.
- Server ESLint for touched focused tests: passed.
- Server team OpenAPI test could not start because the isolated worktree's copied `configs/dev.local.yaml` fails existing schema validation (`deepseek_text` missing and `getapi_video` invalid); this is the documented baseline environment failure.
- Both worktrees pass `git diff --check` before commit.
- Native focused test compilation was already running in the shared workspace and remained in progress due the large native crate; no completed result was available during this task.
