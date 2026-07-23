# ArtForge Studio Documentation Consolidation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace obsolete and duplicate project documentation with a concise, code-verified set of maintainer documents plus one product-boundary page.

**Architecture:** The root README remains the short entry point, while `docs/README.md` routes readers to five single-purpose authority documents. Current source, packaging scripts, and release workflows are the factual sources; historical plans and archived client generations are deleted instead of copied into an archive.

**Tech Stack:** Markdown, Rust/Cargo, Slint 1.16.1, Bash, PowerShell, GitHub Actions, Node.js for the Markdown link verification command.

## Global Constraints

- The active application is `native-client`, the only Cargo workspace member, and produces the `ArtForgeStudio` binary.
- Documentation is for developers and release maintainers, with one concise product-boundary page.
- Delete obsolete documents directly; do not create `docs/archive/`.
- Git history is the only historical archive.
- Do not modify or delete the untracked `design-qa.md`.
- Do not document mutable model names, membership prices, credit packs, grants, discounts, or per-operation credit costs as client constants.
- Account, membership, credits, orders, agreements, model catalog, pricing, and generation task state are server-authoritative.
- Current facts must be verified against source, `scripts/`, and `.github/workflows/`.

---

### Task 1: Rewrite the repository entry point and maintainer rules

**Files:**
- Modify: `README.md`
- Modify: `AGENTS.md`

**Interfaces:**
- Consumes: `Cargo.toml`, `native-client/Cargo.toml`, `native-client/src/runtime/`, `native-client/ui/`, `scripts/`, `.github/workflows/`
- Produces: the root entry point and the mandatory repository-maintenance rules used by every later document

- [ ] **Step 1: Record the current stale assertions**

Run:

```bash
rg -n 'docs/rewrite|Slint 1\\.8|Provider Registry|artait-app|ui/pages|themes/|schema|PySide6' README.md AGENTS.md
```

Expected: matches show the obsolete rewrite index, Slint version, archived Provider architecture, root UI tree, themes, or schema guidance.

- [ ] **Step 2: Rewrite `README.md` as the concise entry point**

Use this exact section order:

```markdown
# ArtForge Studio

ArtForge Studio 是使用 Rust 与 Slint 构建的跨平台桌面 AI 美术生产客户端。根 Cargo workspace 只构建 `native-client`，并生成唯一应用二进制 `ArtForgeStudio`。

## Supported platforms
## Quick start
## Build and test
## Package
## Repository layout
## Documentation
```

Required content:

- State that `native-client` is the only active Cargo workspace member and `ArtForgeStudio` is the only application binary.
- State that Windows x64, macOS Intel, and macOS Apple Silicon are release targets.
- Show `cargo run -p artforge-studio-native --bin ArtForgeStudio`.
- Show `cargo check -p artforge-studio-native` and `cargo test -p artforge-studio-native`.
- Show `cargo build --release -p artforge-studio-native --bin ArtForgeStudio`.
- Show `./scripts/package-macos.sh x64`, `./scripts/package-macos.sh aarch64`, and `./scripts/package-native-client.ps1 -Target windows`.
- Identify `crates/`, root `ui/`, `schemas/`, and `themes/` as historical source excluded from the current build.
- Link only to `docs/README.md` for detailed documentation.

- [ ] **Step 3: Rewrite `AGENTS.md` around the active client**

Use this exact section order:

```markdown
# ArtForge Studio — Agent Guide

## Repository boundary
## Active architecture
## Build and verification
## Slint and Rust integration
## Server-authoritative data
## Local persistence and recovery
## Platform behavior
## Security and logging
## Change guidelines
```

Required facts:

- Rust edition 2021, package version from `native-client/Cargo.toml`, Slint 1.16.1.
- `src/runtime/app.rs` owns startup and top-level wiring.
- `src/runtime/callbacks/`, `api/`, `generation/`, `storage/`, `features/`, and `presentation/` are the active Rust boundaries.
- `native-client/ui/app.slint`, `app-state.slint`, `types.slint`, `components/`, `pages/`, and `dialogs/` are the active UI boundaries.
- Long work stays off the Slint UI event loop; UI updates return through the event loop.
- API tokens, API keys, prompts, checkout URLs, signed URLs, and agreement contents must not enter logs.
- Old Provider Endpoint/API Key configuration and local credit mutation must not be reintroduced.
- Preserve user changes and do not modify archived roots unless a task explicitly scopes them.

- [ ] **Step 4: Verify root documentation no longer presents archived systems as active**

Run:

```bash
rg -n 'docs/rewrite|Slint 1\\.8|Provider Registry|artait-app/src|ui/pages|themes/|schemas/' README.md AGENTS.md
```

Expected: no matches.

Run:

```bash
rg -n 'Slint 1\\.16\\.1|server-authoritative|服务端|native-client|ArtForgeStudio' README.md AGENTS.md
```

Expected: both files contain the active client boundary, and `AGENTS.md` contains the current Slint and service-authority rules.

- [ ] **Step 5: Commit the entry-point rewrite**

```bash
git add README.md AGENTS.md
git commit -m "docs: refresh repository entry points"
```

---

### Task 2: Add the documentation index and product boundary

**Files:**
- Create: `docs/README.md`
- Create: `docs/PRODUCT.md`

**Interfaces:**
- Consumes: Task 1 root terminology and `native-client/ui/app.slint`
- Produces: the only detailed-document index and the stable product/data-authority boundary

- [ ] **Step 1: Confirm the active page composition before documenting it**

Run:

```bash
rg -n 'Page \\{|AppState\\.page ==' native-client/ui/app.slint native-client/ui/pages
```

Expected: active routes and page components for welcome, inspiration, studio, assets, models, credits, notifications, and settings are visible in the source.

- [ ] **Step 2: Create `docs/README.md`**

Use this exact table:

```markdown
| Document | Purpose |
|---|---|
| [PRODUCT.md](PRODUCT.md) | Product scope and client/server authority |
| [ARCHITECTURE.md](ARCHITECTURE.md) | Active native-client architecture and data flow |
| [DEVELOPMENT.md](DEVELOPMENT.md) | Local development, tests, and troubleshooting |
| [RELEASE.md](RELEASE.md) | Packaging, CI, signing, and release verification |
| [MIGRATION.md](MIGRATION.md) | Migration from provider-direct client versions |
```

Add:

- A statement that current source and workflows override documents when they differ.
- Reading orders for a new maintainer, a release operator, and migration support.
- A rule that plans, dated status snapshots, backend database design, and mutable commercial values do not belong in this index.

- [ ] **Step 3: Create `docs/PRODUCT.md`**

Use this exact section order:

```markdown
# Product Boundary

## Purpose
## Current client areas
## Server-authoritative data
## Local data
## Deliberate exclusions
```

Document:

- Welcome, inspiration, studio generation, generation history, assets, model catalog, credits/membership, notifications, and settings.
- Server authority for authentication, device sessions, agreements, membership, credits, orders, payment status, model catalog, pricing, and generation task state.
- Local authority for downloaded works, local metadata needed to display them, UI preferences, prompt drafts/custom prompts, and recovery records.
- Dynamic commercial values are displayed from API responses and are not hard-coded into documentation.
- Direct Provider/API Key configuration, locally granted credits, invitation rebate mock data, and backend database internals are outside the current client.

- [ ] **Step 4: Verify the index and product boundary**

Run:

```bash
test -f docs/README.md
test -f docs/PRODUCT.md
rg -n 'ARCHITECTURE.md|DEVELOPMENT.md|RELEASE.md|MIGRATION.md' docs/README.md
rg -n '服务端|server|本地|local|动态|dynamic' docs/PRODUCT.md
```

Expected: both files exist; all five authority documents are indexed; server/local/dynamic boundaries are explicit.

- [ ] **Step 5: Commit the index and product boundary**

```bash
git add docs/README.md docs/PRODUCT.md
git commit -m "docs: add current product documentation index"
```

---

### Task 3: Consolidate architecture and migration truth

**Files:**
- Create: `docs/ARCHITECTURE.md`
- Create: `docs/MIGRATION.md`
- Delete: `native-client/ARCHITECTURE.md`
- Delete: `native-client/MIGRATION.md`
- Delete: `docs/CURRENT_CLIENT_MERGE.md`

**Interfaces:**
- Consumes: `native-client/src/runtime/`, `native-client/ui/`, existing native-client architecture and migration documents
- Produces: one architecture authority and one migration authority

- [ ] **Step 1: Capture active architecture modules and migration text**

Run:

```bash
rg --files native-client/src/runtime native-client/ui | sort
sed -n '1,220p' native-client/ARCHITECTURE.md
sed -n '1,220p' native-client/MIGRATION.md
sed -n '1,160p' docs/CURRENT_CLIENT_MERGE.md
```

Expected: the active source tree and all still-useful source text are available before deletion.

- [ ] **Step 2: Create `docs/ARCHITECTURE.md`**

Use this exact section order:

```markdown
# Architecture

## Repository and binary boundary
## Runtime layers
## Slint layers
## Startup and account bootstrap
## Image and prompt task flow
## Payment flow
## Result delivery and local recovery
## Platform differences
## Security boundary
## Historical source boundary
```

Required runtime paths:

- `src/main.rs`, `src/lib.rs`, `src/runtime/app.rs`
- `src/runtime/callbacks/`
- `src/runtime/api/`
- `src/runtime/generation/`
- `src/runtime/storage/`
- `src/runtime/features/`
- `src/runtime/presentation/`
- `src/runtime/services/image_processing.rs`

Required flow rules:

- The platform API owns remote business state.
- Requests and recovery operations use stable identifiers so retries do not create duplicate business operations.
- Generation results are downloaded, verified, saved atomically, recorded locally, and then acknowledged.
- Windows defaults to the GPU-backed Slint renderer, while `SLINT_BACKEND` remains an explicit override; non-Windows defaults to the software renderer.
- Windows and macOS embed trusted HTTPS payment/agreement content with platform WebViews where implemented; navigation is restricted to allowlisted hosts.

- [ ] **Step 3: Create `docs/MIGRATION.md`**

Preserve the existing migration behavior while replacing file-local wording with repository-wide links. Include:

- retained works, metadata, prompt drafts, preferences, secure refresh session, and recovery records;
- discarded local login, local credits, Provider Endpoint/API Key, provider model selection, and connection-test state;
- first-start login, agreement acceptance, account snapshot, asset check, and pending-operation recovery;
- a warning not to delete the current works directory.

- [ ] **Step 4: Delete the superseded architecture and migration files**

Delete only:

```text
native-client/ARCHITECTURE.md
native-client/MIGRATION.md
docs/CURRENT_CLIENT_MERGE.md
```

- [ ] **Step 5: Verify the consolidated documents before committing**

Run:

```bash
test -f docs/ARCHITECTURE.md
test -f docs/MIGRATION.md
test ! -e native-client/ARCHITECTURE.md
test ! -e native-client/MIGRATION.md
test ! -e docs/CURRENT_CLIENT_MERGE.md
rg -n 'runtime/api|runtime/generation|runtime/storage|SLINT_BACKEND|WebView|历史' docs/ARCHITECTURE.md
rg -n '作品|API Key|积分|首次启动|不要删除' docs/MIGRATION.md
```

Expected: new authority files exist, three superseded files are absent, and required architecture/migration concepts are present.

- [ ] **Step 6: Commit architecture and migration consolidation**

```bash
git add docs/ARCHITECTURE.md docs/MIGRATION.md native-client/ARCHITECTURE.md native-client/MIGRATION.md docs/CURRENT_CLIENT_MERGE.md
git commit -m "docs: consolidate architecture and migration guides"
```

---

### Task 4: Consolidate development and release operations

**Files:**
- Create: `docs/DEVELOPMENT.md`
- Create: `docs/RELEASE.md`
- Delete: `docs/GITHUB_ACTIONS_RELEASE_SETUP.md`

**Interfaces:**
- Consumes: `native-client/src/runtime/api/client.rs`, `native-client/src/runtime/api/cross_stack_tests.rs`, `native-client/src/runtime/app.rs`, `scripts/`, `.github/workflows/`
- Produces: one development authority and one release authority

- [ ] **Step 1: Verify source-backed commands and environment variables**

Run:

```bash
rg -n 'ARTFORGE_API_BASE_URL|ARTFORGE_CROSS_STACK_BASE_URL|ARTFORGE_MOCK_EMAIL_CODE|SLINT_BACKEND' native-client/src
rg -n 'Target|x64|aarch64|windows|APP_VERSION|APPLE_SIGNING_IDENTITY' scripts/package-macos.sh scripts/package-native-client.ps1 scripts/build-release.ps1
rg -n 'tags:|runs-on:|APPLE_|ALIYUN_|ArtForgeStudio_' .github/workflows/release-desktop.yml
```

Expected: every environment variable, target, secret name, and artifact naming rule to be documented has a current code or workflow source.

- [ ] **Step 2: Create `docs/DEVELOPMENT.md`**

Use this exact section order:

```markdown
# Development

## Prerequisites
## Run locally
## Check and test
## API environment overrides
## Cross-stack Mock API tests
## Renderer selection
## Platform-specific verification
## Troubleshooting
```

Required commands:

```bash
cargo run -p artforge-studio-native --bin ArtForgeStudio
cargo check -p artforge-studio-native
cargo test -p artforge-studio-native
cargo build --release -p artforge-studio-native --bin ArtForgeStudio
```

Required environment variables:

- `ARTFORGE_API_BASE_URL`
- `ARTFORGE_CROSS_STACK_BASE_URL`
- `ARTFORGE_MOCK_EMAIL_CODE`
- `SLINT_BACKEND`

Cross-stack command:

```bash
ARTFORGE_CROSS_STACK_BASE_URL=http://127.0.0.1:39091 \
ARTFORGE_MOCK_EMAIL_CODE=654321 \
cargo test -p artforge-studio-native --locked \
  cross_stack_ -- --ignored --nocapture --test-threads=1
```

State that the backend Mock API must already be running and that backend startup belongs to the backend repository.

- [ ] **Step 3: Create `docs/RELEASE.md`**

Use this exact section order:

```markdown
# Release

## Version and tag
## Local packages
## Release workflow
## Artifacts
## Required secrets
## macOS signing and notarization
## Windows packaging
## Release checklist
## Windows payment and WebView2 acceptance
```

Required artifact names:

- `ArtForgeStudio_<version>_windows_x64_setup.exe`
- `ArtForgeStudio_<version>_windows_x64_portable.zip`
- `ArtForgeStudio_<version>_macos_x64.dmg`
- `ArtForgeStudio_<version>_macos_aarch64.dmg`

Required facts:

- Version comes from `native-client/Cargo.toml`; tag `vX.Y.Z` must match.
- `.github/workflows/release-desktop.yml` currently triggers on `v*` tag pushes.
- `./scripts/package-macos.sh x64` and `aarch64` create DMGs on macOS.
- `./scripts/package-native-client.ps1 -Target windows` creates the portable Windows package; CI also creates the Inno Setup installer.
- Without `APPLE_SIGNING_IDENTITY`, the local DMG is explicitly unsigned.
- List the exact `APPLE_*`, `KEYCHAIN_PASSWORD`, and `ALIYUN_OSS_*` secret names already used by the workflow, with no values.
- Preserve the current WebView2/payment acceptance requirements, but remove dated validation logs and sample version `1.0.0`.

- [ ] **Step 4: Delete the superseded release document**

Delete:

```text
docs/GITHUB_ACTIONS_RELEASE_SETUP.md
```

- [ ] **Step 5: Verify operational documentation**

Run:

```bash
test -f docs/DEVELOPMENT.md
test -f docs/RELEASE.md
test ! -e docs/GITHUB_ACTIONS_RELEASE_SETUP.md
rg -n 'ARTFORGE_API_BASE_URL|ARTFORGE_CROSS_STACK_BASE_URL|SLINT_BACKEND' docs/DEVELOPMENT.md
rg -n 'windows_x64_setup|windows_x64_portable|macos_x64|macos_aarch64|APPLE_SIGNING_IDENTITY|ALIYUN_OSS_' docs/RELEASE.md
```

Expected: development overrides and all four release artifacts are documented from current sources, and the old release document is absent.

- [ ] **Step 6: Commit development and release consolidation**

```bash
git add docs/DEVELOPMENT.md docs/RELEASE.md docs/GITHUB_ACTIONS_RELEASE_SETUP.md
git commit -m "docs: consolidate development and release operations"
```

---

### Task 5: Delete obsolete generations and verify the final documentation set

**Files:**
- Delete: `PROJECT_STRUCTURE.md`
- Delete: `assets/README.md`
- Delete: `schemas/README.md`
- Delete: `docs/ArtForgeStudio-exe-function-breakdown.md`
- Delete: `docs/ArtStudio-main-source-function-interaction-map.md`
- Delete: `docs/FRONTEND_BACKEND_INTEGRATION_EXECUTION_PLAN.md`
- Delete: `docs/MEMBERSHIP_DATABASE_DESIGN.md`
- Delete: `docs/MEMBERSHIP_INTEGRATION_PLAN.md`
- Delete: `docs/MIGRATION_PLAN.md`
- Delete: `docs/STATUS.md`
- Delete: `docs/TODOS.md`
- Delete: `docs/plans/2026-06-12-asset-metadata-index.md`
- Delete: `docs/rewrite/00-index.md`
- Delete: `docs/rewrite/01-product-overview.md`
- Delete: `docs/rewrite/02-ui-map.md`
- Delete: `docs/rewrite/03-ui-feature-spec.md`
- Delete: `docs/rewrite/04-user-workflows.md`
- Delete: `docs/rewrite/05-data-model.md`
- Delete: `docs/rewrite/06-provider-contract.md`
- Delete: `docs/rewrite/07-rust-architecture.md`
- Delete: `docs/rewrite/08-migration-plan.md`
- Delete: `docs/rewrite/09-ui-theming.md`
- Delete: `docs/rewrite/10-onboarding.md`
- Delete: `docs/rewrite/11-ui-framework-guidelines.md`
- Delete: `docs/rewrite/12-v1.5-game-asset-control.md`

**Interfaces:**
- Consumes: Tasks 1–4, which must already contain every current fact retained from the old documents
- Produces: the final minimal documentation tree

- [ ] **Step 1: Confirm all delete targets are tracked and `design-qa.md` is untracked**

Run:

```bash
git status --short
git ls-files PROJECT_STRUCTURE.md assets/README.md schemas/README.md docs native-client/ARCHITECTURE.md native-client/MIGRATION.md
```

Expected: delete targets are tracked; `design-qa.md` is shown only as untracked and is not part of any delete list.

- [ ] **Step 2: Delete the obsolete files listed in this task**

Use `apply_patch` delete operations for every listed file. Do not delete:

```text
design-qa.md
docs/README.md
docs/PRODUCT.md
docs/ARCHITECTURE.md
docs/DEVELOPMENT.md
docs/RELEASE.md
docs/MIGRATION.md
docs/superpowers/specs/2026-07-23-documentation-consolidation-design.md
docs/superpowers/plans/2026-07-23-documentation-consolidation.md
```

- [ ] **Step 3: Verify the final Markdown inventory**

Run:

```bash
rg --files -g '*.md' | sort
```

Expected tracked project documentation:

```text
AGENTS.md
README.md
docs/ARCHITECTURE.md
docs/DEVELOPMENT.md
docs/MIGRATION.md
docs/PRODUCT.md
docs/README.md
docs/RELEASE.md
docs/superpowers/plans/2026-07-23-documentation-consolidation.md
docs/superpowers/specs/2026-07-23-documentation-consolidation-design.md
```

`design-qa.md` may also appear because it remains intentionally untracked.

- [ ] **Step 4: Scan for obsolete current-state claims**

Run:

```bash
rg -n 'PySide6|ArtAITRust\\.exe|Slint 1\\.8|固定验证码|本地积分|Provider Registry|docs/rewrite|阶段 [0-9]+|当前版本.*0\\.1\\.0' README.md AGENTS.md docs
```

Expected: no obsolete current-state claims. Historical phrases are allowed only when `docs/MIGRATION.md` explicitly says the behavior is discarded.

- [ ] **Step 5: Check every local Markdown link**

Run:

```bash
node -e 'const fs=require("fs"),path=require("path"),cp=require("child_process");const files=cp.execFileSync("rg",["--files","-g","*.md"],{encoding:"utf8"}).trim().split("\\n").filter(Boolean);let bad=0;for(const file of files){const text=fs.readFileSync(file,"utf8").replace(/```[\\s\\S]*?```/g,"");for(const match of text.matchAll(/\\]\\(([^)]+)\\)/g)){const raw=match[1].split("#")[0];if(!raw||/^(https?:|mailto:)/.test(raw))continue;const target=path.resolve(path.dirname(file),decodeURI(raw));if(!fs.existsSync(target)){console.error(`${file}: missing ${match[1]}`);bad++;}}}process.exitCode=bad?1:0;'
```

Expected: exit code 0 and no missing-link output.

- [ ] **Step 6: Verify formatting and project health**

Run:

```bash
git diff --check
cargo check -p artforge-studio-native
git status --short
```

Expected:

- `git diff --check` exits 0.
- Cargo check finishes successfully.
- `design-qa.md` remains `?? design-qa.md`.
- No build output is staged or tracked.

- [ ] **Step 7: Commit obsolete-document removal**

```bash
git add -u
git commit -m "docs: remove obsolete project documentation"
```

- [ ] **Step 8: Verify commit boundary and final status**

Run:

```bash
git status --short
git log -6 --oneline --decorate
```

Expected: only `?? design-qa.md` remains; the documentation commits are visible and no unrelated file was committed.
