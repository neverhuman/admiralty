# Admiralty Jankurai Conformance Cleanup Backlog (`SC-DEC-186J`)

- **Decision ID:** `SC-DEC-186J` (`SC_ORDER_ADMIRALTY_SC186J_PR158_7291122E_85_FINDINGS_56_BLOCKERS_CLEANUP_DECISION_AND_MAIN_FREEZE_20261019T2018Z.md`)
- **Recorded At:** `2026-10-19T20:46:00Z`
- **Assigned Owner:** `2062267.agy_captain_admiralty:0` (Admiralty Commodore, coordinated with Root Codex `01a1548d-1f24-77b1-8886-1081828fe3b4`)
- **Audited Source Head (`PR #158`):** `7291122e7c33581fbfbc35468a9c1c7c5594a164` (base `origin/main` `d858a9a292a9e458d5851531e1e548f817908520`)
- **Auditor Binary:** Official Jankurai `v1.7.5` (`installer sha256: 828e513628080823a9afaa19eeaeee4ce2e54174483a11cdde00073dbafb895f`, `binary sha256: d2414884a3dbe1cc2d02b4240588869f6afac26ff0f167829e907b1617501121`)
- **Hosted CI Audit Run:** `run 39315599993` / `job 118276153040` (`https://github.com/neverhuman/admiralty/actions/runs/39315599993/job/118276153040`), uploaded artifact `12217379455` (`sha256: d6ba1660ead8b0965089f61e8180aa79c105840e0f53d4ca1765d97bb2b365a6`)
- **Local 90s Audit Receipt:** `/tmp/admiralty-pr158-local-qualification-7291122/receipt-90s.json` (`repo-score.json` `sha256: b93b310bb1b19905e020978faec29af4a24a550ec6f8378f7c4174f1cdbe06ec`, `repo-score.md` `sha256: d1cf411b90ddfad3b62ced00690d97df23e3042676c899ff405fc9c9bc112293`, `repair-queue.jsonl` `sha256: 8e798e97f2ba09ae9a0b349997845322e0f813c605817456888ee4bde0375d98`, `step-summary.md` `sha256: 38c7e1cd59fb8f7fdad197efaa7e63dc8eb24087a350004bce82c74473deb395`)
- **Baseline Score & Counts:** `score = 39`, `raw_score = 39`, `caps_applied = 22`, `findings = 85` (`56` hard conformance blockers + `29` soft findings; `1` critical, `55` high, `29` medium)
- **Main Merge Freeze:** `neverhuman/admiralty` `main` (`d858a9a292a9e458d5851531e1e548f817908520`) remains under strict merge freeze for all non-cutover items until `d858a9a` hosted `Admiralty checks` is green and `PR #158` passes its 20-stage local gate and Root's exact-head review.

## 1. Category Summary & Phased Remediation Schedule (`85` Findings / `56` Hard Blockers)

| Category | Total | Hard Blockers | Soft Findings | Tracker Phase | Assigned Owner | Target ETA (UTC) |
| :--- | ---: | ---: | ---: | :--- | :--- | :--- |
| `vibe` | 22 | 22 | 0 | `ADM-JK04 (Vibe / Overgrown Module Decomposition & Proof Lanes)` | `2062267.agy_captain_admiralty:0` | `2026-10-20T08:00:00Z` |
| `boundary` | 13 | 12 | 1 | `ADM-JK03 (Contracts, Decoders & Generated Zone Guards)` | `2062267.agy_captain_admiralty:0` | `2026-10-20T06:00:00Z` |
| `security` | 10 | 9 | 1 | `ADM-JK02 (Security, Shell & Dynamic Sink Hardening)` | `2062267.agy_captain_admiralty:0` | `2026-10-20T04:00:00Z` |
| `python` | 5 | 4 | 1 | `ADM-JK01 (Python Retirement & SQLite Parameterization)` | `2062267.agy_captain_admiralty:0` | `2026-10-20T02:00:00Z` |
| `agent` | 2 | 2 | 0 | `ADM-JK05 (CI Ops Extraction, Proof, Release & Context Metadata)` | `2062267.agy_captain_admiralty:0` | `2026-10-20T10:00:00Z` |
| `proof` | 4 | 1 | 3 | `ADM-JK05 (CI Ops Extraction, Proof, Release & Context Metadata)` | `2062267.agy_captain_admiralty:0` | `2026-10-20T10:00:00Z` |
| `release` | 2 | 1 | 1 | `ADM-JK05 (CI Ops Extraction, Proof, Release & Context Metadata)` | `2062267.agy_captain_admiralty:0` | `2026-10-20T10:00:00Z` |
| `exceptions` | 1 | 1 | 0 | `ADM-JK05 (CI Ops Extraction, Proof, Release & Context Metadata)` | `2062267.agy_captain_admiralty:0` | `2026-10-20T10:00:00Z` |
| `generated` | 1 | 1 | 0 | `ADM-JK03 (Contracts, Decoders & Generated Zone Guards)` | `2062267.agy_captain_admiralty:0` | `2026-10-20T06:00:00Z` |
| `stack` | 1 | 1 | 0 | `ADM-JK01 (Python Retirement & SQLite Parameterization)` | `2062267.agy_captain_admiralty:0` | `2026-10-20T02:00:00Z` |
| `test` | 1 | 1 | 0 | `ADM-JK05 (CI Ops Extraction, Proof, Release & Context Metadata)` | `2062267.agy_captain_admiralty:0` | `2026-10-20T10:00:00Z` |
| `ux-qa` | 1 | 1 | 0 | `ADM-JK05 (CI Ops Extraction, Proof, Release & Context Metadata)` | `2062267.agy_captain_admiralty:0` | `2026-10-20T10:00:00Z` |
| `ci` | 8 | 0 | 8 | `ADM-JK05 (CI Ops Extraction, Proof, Release & Context Metadata)` | `2062267.agy_captain_admiralty:0` | `2026-10-20T10:00:00Z` |
| `context` | 8 | 0 | 8 | `ADM-JK05 (CI Ops Extraction, Proof, Release & Context Metadata)` | `2062267.agy_captain_admiralty:0` | `2026-10-20T10:00:00Z` |
| `copy-code` | 3 | 0 | 3 | `ADM-JK04 (Vibe / Overgrown Module Decomposition & Proof Lanes)` | `2062267.agy_captain_admiralty:0` | `2026-10-20T08:00:00Z` |
| `docs` | 1 | 0 | 1 | `ADM-JK05 (CI Ops Extraction, Proof, Release & Context Metadata)` | `2062267.agy_captain_admiralty:0` | `2026-10-20T10:00:00Z` |
| `observability` | 1 | 0 | 1 | `ADM-JK05 (CI Ops Extraction, Proof, Release & Context Metadata)` | `2062267.agy_captain_admiralty:0` | `2026-10-20T10:00:00Z` |
| `shape` | 1 | 0 | 1 | `ADM-JK04 (Vibe / Overgrown Module Decomposition & Proof Lanes)` | `2062267.agy_captain_admiralty:0` | `2026-10-20T08:00:00Z` |
| **TOTAL** | **85** | **56** | **29** | `ADM-JK01`..`ADM-JK05` | `2062267.agy_captain_admiralty:0` | `2026-10-20T10:00:00Z` |

## 2. Complete Inventory of All 56 Hard Conformance Blockers (`hardness = hard`)

| # | Category | Severity | Rule / Check ID | Path | Problem | Phase & ETA |
| ---: | :--- | :--- | :--- | :--- | :--- | :--- |
| 1 | `proof` | `high` | `HLT-000-SCORE-DIMENSION:proof` | `.` | no one-command setup or validation lane was detected | `ADM-JK05` (`2026-10-20T10:00:00Z`) |
| 2 | `security` | `high` | `HLT-009-GENERATED-SECURITY` | `.github/workflows` | high-risk repo has no explicit security lane | `ADM-JK02` (`2026-10-20T04:00:00Z`) |
| 3 | `security` | `high` | `HLT-016-SUPPLY-CHAIN-DRIFT` | `.github/workflows` | no secret or dependency scanning was found in CI | `ADM-JK02` (`2026-10-20T04:00:00Z`) |
| 4 | `security` | `high` | `HLT-034-CI-BAD-BEHAVIOR` | `.github/workflows/ci.yml` | untrusted CI job runs with privileged Docker access | `ADM-JK02` (`2026-10-20T04:00:00Z`) |
| 5 | `security` | `high` | `HLT-034-CI-BAD-BEHAVIOR` | `.github/workflows/dashboard-e2e.yml` | untrusted CI job runs with privileged Docker access | `ADM-JK02` (`2026-10-20T04:00:00Z`) |
| 6 | `security` | `high` | `HLT-039-WEB-SECURITY-BAD-BEHAVIOR` | `apps/dashboard/e2e/dashboard-e2e.e2e.ts` | sensitive token or session material is stored in browser-accessible storage | `ADM-JK02` (`2026-10-20T04:00:00Z`) |
| 7 | `security` | `high` | `HLT-039-WEB-SECURITY-BAD-BEHAVIOR` | `apps/dashboard/e2e/dashboard-e2e.e2e.ts` | sensitive token or session material is stored in browser-accessible storage | `ADM-JK02` (`2026-10-20T04:00:00Z`) |
| 8 | `security` | `high` | `HLT-039-WEB-SECURITY-BAD-BEHAVIOR` | `apps/dashboard/e2e/intake.e2e.ts` | sensitive token or session material is stored in browser-accessible storage | `ADM-JK02` (`2026-10-20T04:00:00Z`) |
| 9 | `boundary` | `high` | `HLT-031-TYPESCRIPT-BAD-BEHAVIOR` | `apps/dashboard/src/command-journeys.test.tsx` | unchecked boundary cast or parse result crosses a trust boundary | `ADM-JK03` (`2026-10-20T06:00:00Z`) |
| 10 | `boundary` | `high` | `HLT-031-TYPESCRIPT-BAD-BEHAVIOR` | `apps/dashboard/src/command-journeys.test.tsx` | unchecked boundary cast or parse result crosses a trust boundary | `ADM-JK03` (`2026-10-20T06:00:00Z`) |
| 11 | `boundary` | `high` | `HLT-031-TYPESCRIPT-BAD-BEHAVIOR` | `apps/dashboard/src/command-journeys.test.tsx` | unchecked boundary cast or parse result crosses a trust boundary | `ADM-JK03` (`2026-10-20T06:00:00Z`) |
| 12 | `boundary` | `high` | `HLT-031-TYPESCRIPT-BAD-BEHAVIOR` | `apps/dashboard/src/features/economics/report.ts` | unchecked boundary cast or parse result crosses a trust boundary | `ADM-JK03` (`2026-10-20T06:00:00Z`) |
| 13 | `boundary` | `high` | `HLT-031-TYPESCRIPT-BAD-BEHAVIOR` | `apps/dashboard/src/hierarchy.test.tsx` | dynamic code or raw HTML sink appears in product TypeScript | `ADM-JK03` (`2026-10-20T06:00:00Z`) |
| 14 | `security` | `high` | `HLT-023-INPUT-BOUNDARY-GAP` | `apps/dashboard/src/hierarchy.test.tsx` | unsafe or unvalidated input boundary marker appears in product code | `ADM-JK02` (`2026-10-20T04:00:00Z`) |
| 15 | `security` | `critical` | `HLT-010-SECRET-SPRAWL` | `apps/dashboard/src/intakeDuplicates.test.ts` | secret-like value or credential material appears in repository text | `ADM-JK02` (`2026-10-20T04:00:00Z`) |
| 16 | `boundary` | `high` | `HLT-031-TYPESCRIPT-BAD-BEHAVIOR` | `apps/dashboard/src/legacy.test.tsx` | dynamic code or raw HTML sink appears in product TypeScript | `ADM-JK03` (`2026-10-20T06:00:00Z`) |
| 17 | `boundary` | `high` | `HLT-031-TYPESCRIPT-BAD-BEHAVIOR` | `apps/dashboard/src/ui/CommandJourneys.tsx` | unchecked boundary cast or parse result crosses a trust boundary | `ADM-JK03` (`2026-10-20T06:00:00Z`) |
| 18 | `boundary` | `high` | `HLT-031-TYPESCRIPT-BAD-BEHAVIOR` | `apps/dashboard/src/ui/CommandJourneys.tsx` | unchecked boundary cast or parse result crosses a trust boundary | `ADM-JK03` (`2026-10-20T06:00:00Z`) |
| 19 | `boundary` | `high` | `HLT-031-TYPESCRIPT-BAD-BEHAVIOR` | `apps/dashboard/src/ui/CommandJourneys.tsx` | unchecked boundary cast or parse result crosses a trust boundary | `ADM-JK03` (`2026-10-20T06:00:00Z`) |
| 20 | `boundary` | `high` | `HLT-031-TYPESCRIPT-BAD-BEHAVIOR` | `apps/dashboard/src/ui/CommandJourneys.tsx` | unchecked boundary cast or parse result crosses a trust boundary | `ADM-JK03` (`2026-10-20T06:00:00Z`) |
| 21 | `boundary` | `high` | `HLT-031-TYPESCRIPT-BAD-BEHAVIOR` | `apps/dashboard/src/ui/CommandJourneys.tsx` | unchecked boundary cast or parse result crosses a trust boundary | `ADM-JK03` (`2026-10-20T06:00:00Z`) |
| 22 | `boundary` | `high` | `HLT-031-TYPESCRIPT-BAD-BEHAVIOR` | `apps/dashboard/src/ui/intake.test.tsx` | unchecked boundary cast or parse result crosses a trust boundary | `ADM-JK03` (`2026-10-20T06:00:00Z`) |
| 23 | `ux-qa` | `high` | `HLT-013-RENDERED-UX-GAP` | `apps/web` | web surface lacks layered rendered UX QA evidence | `ADM-JK05` (`2026-10-20T10:00:00Z`) |
| 24 | `test` | `high` | `HLT-008-FALSE-GREEN-RISK` | `crates/` | Rust surface lacks required property and/or integration tests | `ADM-JK05` (`2026-10-20T10:00:00Z`) |
| 25 | `vibe` | `high` | `HLT-001-DEAD-MARKER` | `crates/admiralty-cli/src/main.rs` | future-hostile/dead-language term `temporary` appears in product/runtime code | `ADM-JK04` (`2026-10-20T08:00:00Z`) |
| 26 | `vibe` | `high` | `HLT-001-DEAD-MARKER` | `crates/admiralty-cli/src/native_install.rs` | future-hostile/dead-language term `temporary` appears in product/runtime code | `ADM-JK04` (`2026-10-20T08:00:00Z`) |
| 27 | `vibe` | `high` | `HLT-001-DEAD-MARKER` | `crates/admiralty-core/src/import/todoq.rs` | future-hostile/dead-language term `todo` appears in product/runtime code | `ADM-JK04` (`2026-10-20T08:00:00Z`) |
| 28 | `vibe` | `high` | `HLT-001-DEAD-MARKER` | `crates/admiralty-core/src/import/todoq.rs` | future-hostile/dead-language term `todo` appears in product/runtime code | `ADM-JK04` (`2026-10-20T08:00:00Z`) |
| 29 | `vibe` | `high` | `HLT-001-DEAD-MARKER` | `crates/admiralty-core/src/import/todoq.rs` | future-hostile/dead-language term `todo` appears in product/runtime code | `ADM-JK04` (`2026-10-20T08:00:00Z`) |
| 30 | `vibe` | `high` | `HLT-001-DEAD-MARKER` | `crates/admiralty-core/src/import/todoq.rs` | future-hostile/dead-language term `todo` appears in product/runtime code | `ADM-JK04` (`2026-10-20T08:00:00Z`) |
| 31 | `vibe` | `high` | `HLT-001-DEAD-MARKER` | `crates/admiralty-observe/src/fleet.rs` | fallback soup detected in product code | `ADM-JK04` (`2026-10-20T08:00:00Z`) |
| 32 | `vibe` | `high` | `HLT-001-DEAD-MARKER` | `crates/admiralty-observe/src/projects.rs` | future-hostile/dead-language term `todo` appears in product/runtime code | `ADM-JK04` (`2026-10-20T08:00:00Z`) |
| 33 | `vibe` | `high` | `HLT-001-DEAD-MARKER` | `crates/admiralty-observe/src/projects.rs` | future-hostile/dead-language term `todo` appears in product/runtime code | `ADM-JK04` (`2026-10-20T08:00:00Z`) |
| 34 | `vibe` | `high` | `HLT-001-DEAD-MARKER` | `crates/admiralty-observe/src/usage.rs` | future-hostile/dead-language term `todo` appears in product/runtime code | `ADM-JK04` (`2026-10-20T08:00:00Z`) |
| 35 | `vibe` | `high` | `HLT-001-DEAD-MARKER` | `crates/admiralty-observe/src/usage.rs` | future-hostile/dead-language term `todo` appears in product/runtime code | `ADM-JK04` (`2026-10-20T08:00:00Z`) |
| 36 | `vibe` | `high` | `HLT-001-DEAD-MARKER` | `crates/admiralty-observe/src/usage.rs` | future-hostile/dead-language term `todo` appears in product/runtime code | `ADM-JK04` (`2026-10-20T08:00:00Z`) |
| 37 | `vibe` | `high` | `HLT-001-DEAD-MARKER` | `crates/admiralty-observe/src/usage.rs` | future-hostile/dead-language term `todo` appears in product/runtime code | `ADM-JK04` (`2026-10-20T08:00:00Z`) |
| 38 | `vibe` | `high` | `HLT-001-DEAD-MARKER` | `crates/admiralty-runtime/src/providers/codex.rs` | future-hostile/dead-language term `todo` appears in product/runtime code | `ADM-JK04` (`2026-10-20T08:00:00Z`) |
| 39 | `vibe` | `high` | `HLT-001-DEAD-MARKER` | `crates/admiralty-runtime/src/providers/codex.rs` | future-hostile/dead-language term `todo` appears in product/runtime code | `ADM-JK04` (`2026-10-20T08:00:00Z`) |
| 40 | `exceptions` | `high` | `HLT-017-OPAQUE-OBSERVABILITY` | `crates/domain` | no agent-friendly exception/error pattern was detected | `ADM-JK05` (`2026-10-20T10:00:00Z`) |
| 41 | `stack` | `high` | `HLT-000-SCORE-DIMENSION:stack` | `dashboard/collector.py` | runtime code uses a language outside the chosen optimal stack | `ADM-JK01` (`2026-10-20T02:00:00Z`) |
| 42 | `vibe` | `high` | `HLT-001-DEAD-MARKER` | `dashboard/collector.py` | future-hostile/dead-language term `todo` appears in product/runtime code | `ADM-JK04` (`2026-10-20T08:00:00Z`) |
| 43 | `vibe` | `high` | `HLT-001-DEAD-MARKER` | `dashboard/collector.py` | future-hostile/dead-language term `todo` appears in product/runtime code | `ADM-JK04` (`2026-10-20T08:00:00Z`) |
| 44 | `vibe` | `high` | `HLT-001-DEAD-MARKER` | `dashboard/collector.py` | future-hostile/dead-language term `todo` appears in product/runtime code | `ADM-JK04` (`2026-10-20T08:00:00Z`) |
| 45 | `vibe` | `high` | `HLT-001-DEAD-MARKER` | `dashboard/collector.py` | future-hostile/dead-language term `todo` appears in product/runtime code | `ADM-JK04` (`2026-10-20T08:00:00Z`) |
| 46 | `vibe` | `high` | `HLT-001-DEAD-MARKER` | `dashboard/collector.py` | future-hostile/dead-language term `todo` appears in product/runtime code | `ADM-JK04` (`2026-10-20T08:00:00Z`) |
| 47 | `vibe` | `high` | `HLT-001-DEAD-MARKER` | `dashboard/collector.py` | future-hostile/dead-language term `todo` appears in product/runtime code | `ADM-JK04` (`2026-10-20T08:00:00Z`) |
| 48 | `vibe` | `high` | `HLT-001-DEAD-MARKER` | `dashboard/collector.py` | future-hostile/dead-language term `todo` appears in product/runtime code | `ADM-JK04` (`2026-10-20T08:00:00Z`) |
| 49 | `python` | `high` | `HLT-033-PYTHON-BAD-BEHAVIOR` | `dashboard/token_yield.py` | string-built SQL reaches a database sink without parameter binding | `ADM-JK01` (`2026-10-20T02:00:00Z`) |
| 50 | `python` | `high` | `HLT-033-PYTHON-BAD-BEHAVIOR` | `dashboard/token_yield.py` | string-built SQL reaches a database sink without parameter binding | `ADM-JK01` (`2026-10-20T02:00:00Z`) |
| 51 | `python` | `high` | `HLT-033-PYTHON-BAD-BEHAVIOR` | `dashboard/token_yield.py` | string-built SQL reaches a database sink without parameter binding | `ADM-JK01` (`2026-10-20T02:00:00Z`) |
| 52 | `release` | `high` | `HLT-025-RELEASE-READINESS-GAP` | `docs/release.md` | release management structure is incomplete | `ADM-JK05` (`2026-10-20T10:00:00Z`) |
| 53 | `generated` | `high` | `HLT-002-GENERATED-MUTATION` | `packages/contracts/generated/config-layer-receipt.ts` | generated zone is not protected strongly enough against hand edits | `ADM-JK03` (`2026-10-20T06:00:00Z`) |
| 54 | `python` | `high` | `HLT-005-PYTHON-PRODUCT-TRUTH` | `python/` | Python appears without a rare advanced-ML/data exception or owns product truth | `ADM-JK01` (`2026-10-20T02:00:00Z`) |
| 55 | `agent` | `high` | `HLT-035-GIT-BAD-BEHAVIOR` | `tools/history_export_cron.sh` | script performs a destructive git clean | `ADM-JK05` (`2026-10-20T10:00:00Z`) |
| 56 | `agent` | `high` | `HLT-035-GIT-BAD-BEHAVIOR` | `tools/history_export_cron.sh` | script stages the entire tree or skips verification | `ADM-JK05` (`2026-10-20T10:00:00Z`) |

## 3. Complete Inventory of All 29 Soft Findings (`hardness = soft`)

| # | Category | Severity | Rule / Check ID | Path | Problem | Phase & ETA |
| ---: | :--- | :--- | :--- | :--- | :--- | :--- |
| 1 | `shape` | `medium` | `HLT-001-DEAD-MARKER` | `.` | `Code shape and semantic surface` scored 0 below the standard floor of 85 | `ADM-JK04` (`2026-10-20T08:00:00Z`) |
| 2 | `context` | `medium` | `HLT-038-REFERENCE-PROFILE-STRUCTURE-GAP` | `.github` | reference-profile cell `ops` is detected at a noncanonical path | `ADM-JK05` (`2026-10-20T10:00:00Z`) |
| 3 | `context` | `medium` | `HLT-048-CANONICAL-CI-GAP` | `.github/workflows/arm64-qualification.yml` | no workflow delegates to a versioned `ops/ci/*.sh` script, so CI cannot be reproduced locally before push | `ADM-JK05` (`2026-10-20T10:00:00Z`) |
| 4 | `ci` | `medium` | `HLT-042-CI-LOCAL-PARITY` | `.github/workflows/arm64-qualification.yml` | ops/ci/lib.sh is missing | `ADM-JK05` (`2026-10-20T10:00:00Z`) |
| 5 | `ci` | `medium` | `HLT-042-CI-LOCAL-PARITY` | `.github/workflows/arm64-qualification.yml` | ops/git-hooks/pre-push is missing | `ADM-JK05` (`2026-10-20T10:00:00Z`) |
| 6 | `ci` | `medium` | `HLT-042-CI-LOCAL-PARITY` | `.github/workflows/arm64-qualification.yml` | scripts/ci-doctor.sh is missing | `ADM-JK05` (`2026-10-20T10:00:00Z`) |
| 7 | `ci` | `medium` | `HLT-042-CI-LOCAL-PARITY` | `.github/workflows/arm64-qualification.yml` | scripts/ci-local.sh is missing | `ADM-JK05` (`2026-10-20T10:00:00Z`) |
| 8 | `ci` | `medium` | `HLT-042-CI-LOCAL-PARITY` | `.github/workflows/arm64-qualification.yml` | workflow inlines CI commands instead of delegating to ops/ci/*.sh | `ADM-JK05` (`2026-10-20T10:00:00Z`) |
| 9 | `ci` | `medium` | `HLT-042-CI-LOCAL-PARITY` | `.github/workflows/ci.yml` | workflow inlines CI commands instead of delegating to ops/ci/*.sh | `ADM-JK05` (`2026-10-20T10:00:00Z`) |
| 10 | `ci` | `medium` | `HLT-042-CI-LOCAL-PARITY` | `.github/workflows/dashboard-e2e.yml` | workflow inlines CI commands instead of delegating to ops/ci/*.sh | `ADM-JK05` (`2026-10-20T10:00:00Z`) |
| 11 | `security` | `medium` | `HLT-016-SUPPLY-CHAIN-DRIFT` | `.github/workflows/jankurai.yml` | `Security and supply-chain posture` scored 50 below the standard floor of 85 | `ADM-JK02` (`2026-10-20T04:00:00Z`) |
| 12 | `ci` | `medium` | `HLT-042-CI-LOCAL-PARITY` | `.github/workflows/jankurai.yml` | workflow inlines CI commands instead of delegating to ops/ci/*.sh | `ADM-JK05` (`2026-10-20T10:00:00Z`) |
| 13 | `context` | `medium` | `HLT-015-CONTEXT-SETUP-GAP` | `AGENTS.md` | `Context economy and agent instructions` scored 2 below the standard floor of 85 | `ADM-JK05` (`2026-10-20T10:00:00Z`) |
| 14 | `context` | `medium` | `HLT-000-SCORE-DIMENSION:context` | `AGENTS.md` | no root agent/developer instruction file routes contributors at the repository root | `ADM-JK05` (`2026-10-20T10:00:00Z`) |
| 15 | `proof` | `medium` | `HLT-018-PERF-CONCURRENCY-DRIFT` | `Justfile` | `Build speed signals` scored 80 below the standard floor of 85 | `ADM-JK05` (`2026-10-20T10:00:00Z`) |
| 16 | `context` | `medium` | `HLT-047-CANONICAL-README` | `README.md` | README has no status or score badge, so build/audit health is not visible at a glance | `ADM-JK05` (`2026-10-20T10:00:00Z`) |
| 17 | `boundary` | `medium` | `HLT-007-HANDWRITTEN-CONTRACT` | `agent/boundaries.toml` | `Contract and boundary integrity` scored 73 below the standard floor of 85 | `ADM-JK03` (`2026-10-20T06:00:00Z`) |
| 18 | `context` | `medium` | `HLT-003-OWNERLESS-PATH` | `agent/owner-map.json` | `Ownership and navigation surface` scored 24 below the standard floor of 85 | `ADM-JK05` (`2026-10-20T10:00:00Z`) |
| 19 | `proof` | `medium` | `HLT-004-UNMAPPED-PROOF` | `agent/test-map.json` | `Proof lanes and test routing` scored 57 below the standard floor of 85 | `ADM-JK05` (`2026-10-20T10:00:00Z`) |
| 20 | `copy-code` | `medium` | `HLT-046-UNNECESSARY-VARIETY` | `crates/admiralty-cli/src/diagnostics.rs` | enum `Knowledge` has 2 divergent definitions across modules where one consistent definition is expected | `ADM-JK04` (`2026-10-20T08:00:00Z`) |
| 21 | `copy-code` | `medium` | `HLT-046-UNNECESSARY-VARIETY` | `crates/admiralty-core/src/delivery.rs` | enum `ResponsibleRole` has 2 divergent definitions across modules where one consistent definition is expected | `ADM-JK04` (`2026-10-20T08:00:00Z`) |
| 22 | `proof` | `medium` | `HLT-027-HUMAN-REVIEW-EVIDENCE-GAP` | `crates/admiralty-core/tests/scheduling_profile.rs` | human review or proof claim lacks reproducible evidence | `ADM-JK05` (`2026-10-20T10:00:00Z`) |
| 23 | `copy-code` | `medium` | `HLT-046-UNNECESSARY-VARIETY` | `crates/admiralty-runtime/src/providers/usage.rs` | enum `UsageError` has 2 divergent definitions across modules where one consistent definition is expected | `ADM-JK04` (`2026-10-20T08:00:00Z`) |
| 24 | `context` | `medium` | `HLT-040-REPO-ROT-BAD-BEHAVIOR` | `crates/admiralty-store/src/backup.rs` | active source path looks like an old, backup, copied, or parked implementation | `ADM-JK05` (`2026-10-20T10:00:00Z`) |
| 25 | `docs` | `medium` | `HLT-000-SCORE-DIMENSION:docs` | `docs/` | agent-readable documentation is incomplete | `ADM-JK05` (`2026-10-20T10:00:00Z`) |
| 26 | `observability` | `medium` | `HLT-017-OPAQUE-OBSERVABILITY` | `docs/testing.md` | `Observability and repair evidence` scored 48 below the standard floor of 85 | `ADM-JK05` (`2026-10-20T10:00:00Z`) |
| 27 | `release` | `medium` | `HLT-026-COST-BUDGET-GAP` | `docs/testing.md` | cost or spend-risk surface lacks budget proof | `ADM-JK05` (`2026-10-20T10:00:00Z`) |
| 28 | `context` | `medium` | `HLT-038-REFERENCE-PROFILE-STRUCTURE-GAP` | `ops/` | reference-profile cell `ops` lacks local AGENTS.md guidance | `ADM-JK05` (`2026-10-20T10:00:00Z`) |
| 29 | `python` | `medium` | `HLT-005-PYTHON-PRODUCT-TRUTH` | `python/ai-service` | `Python containment and polyglot hygiene` scored 0 below the standard floor of 85 | `ADM-JK01` (`2026-10-20T02:00:00Z`) |

## Source review and current onboarding boundary

This is the Commodore's recorded schedule and baseline, not a claim of completed
remediation. All 85 source fingerprints, locations and dispositions are retained
in [the machine-readable triage catalog](receipts/integration/jankurai-triage-20261019.json).
Seven matches are source-reviewed negative-test or parameterized-SQL false
positives; those guards remain intact. No finding is marked auditor-resolved.

SC-DEC-186J explicitly changes the onboarding job to advisory baseline reporting.
The original standard-mode exit1, score39 and 56 blockers remain in the archived
reports; advisory success does not establish strict conformance or release readiness.
The Rust `python_policy` integration target, included in `pnpm check` and hosted
`cargo test --workspace --locked`, rejects net-new Python, undocumented exceptions,
stale/duplicate rows and incorrect migration line counts. The 40 existing paths
in `tests/fixtures/python-baseline.txt` form the reviewed ceiling. A new row in
the exception document cannot approve its own addition. Changing that ceiling
requires the maintainer's explicit exception and the normal exact-head review.
(Superseded on 2026-10-21: the ceiling is now zero; see section 9.)

## 8. SC-DEC-186O Remediation Slice 1 Dispositions (`b958736f36bfc656961c30af5df5499c6c1dd239` Base)

- **Original & Intermediate Staged Commits Preserved:**
  - `c2a5398bb151dc826074bfd43f07a24d6e0a4338` (archived at `/home/ubuntu/admiralty-v2-build/bundles/commodore-c2a5398bb151-sc186o-slice1-20261019.bundle`, `sha256: 89f46857a0896075d5245e1dbe1d2c2130d8e2efc697c50d3ba4fb217255a616`).
  - `f320be58e5a20da4c235400ba35e44eaccf4038f` (archived at `/home/ubuntu/admiralty-v2-build/bundles/commodore-f320be58e5a2-sc186o-slice1-20261019.bundle`, `sha256: 5b79b1df9b35c6fa3d62f284d5b46cf5ccc35896c4ad23921004dba8c01a1248`).
  - Root's negative-control failure receipt on `c2a5398bb151` `tools/history_export_cron.sh` (`script_sha256: 29fc13e88d05c0665dce4a876bc059fc9937742d8c43cfdba8dec2098fec2e73`): preserved at `/tmp/adm-history-negative-h138499r/negative-control-receipt.json` (`sha256: 907929d861396aa58b42643984988873af00c12c3dee1dda557601ddbe0c800d`, `safety_property_passed: false`).
- **Truthful Stack Profile & No Policy Suppression:**
  - `agent/audit-policy.toml` sets `blocking = true` and `excluded_paths = []`.
  - `agent/boundaries.toml` declares truthful `id = "rust-ts-vite-react-sqlite-bounded-python"` matching `crates/admiralty-store` SQLite persistence.
- **Remediated Findings (`tools/history_export_cron.sh`):**
  - `ADM-AUDIT-084`, `ADM-AUDIT-085` (`HLT-035-GIT-BAD-BEHAVIOR` on `tools/history_export_cron.sh:20,21`): Enforces pre-export index and worktree custody across the entire repository (refuses pre-existing staged index entries, dirty tracked files, or untracked files without running `git reset` or `git clean`), stages only paths matching the exact `is_allowed_export_path` allowlist, re-verifies `git diff --cached --name-only` against `is_allowed_export_path`, and on export failure restores tracked `data/history` from `HEAD` and deletes `.tmp` scratch files. Verified across 5 cases against a local bare remote in `test_history_export_cron_refuses_dirty_tree_and_preserves_untracked_files_on_failure` (`tests/legacy/test_rollout.py`).
- **Calibrated / Partial-Scope Findings Retained OPEN (`audit_resolved: false`):**
  - `ADM-AUDIT-004` (`HLT-009-GENERATED-SECURITY`), `ADM-AUDIT-005` & `ADM-AUDIT-016` (`HLT-016-SUPPLY-CHAIN-DRIFT`): **RETAINED OPEN.** `.github/workflows/security.yml`, `tools/security-lane.sh`, `scripts/check/security-supply-chain.sh`, and `agent/security-policy.toml` enforce `actionlint -config-file .github/actionlint.yaml`, `gitleaks git --no-banner --redact --log-opts="-1" .`, `cargo metadata --locked`, workflow permission/credential-persistence checks, and SHA-256 supply-chain digest emission in a fresh `0700` non-symlink directory, failing closed when any required tool is missing (`test_security_supply_chain_fails_closed_when_required_scanner_missing_or_tmpdir_is_symlink`). Because `cargo audit`, `pnpm audit`, `pip-audit`, and `grype` are not yet provisioned across all LAN runners (`xbabe1`/`xbabe3`), these findings remain open (`unprovisioned_tools` in `agent/security-policy.toml`).
  - `ADM-AUDIT-073`, `ADM-AUDIT-074`, `ADM-AUDIT-075` (`HLT-033-PYTHON-BAD-BEHAVIOR` on `dashboard/token_yield.py:70,89,102`): **RETAINED AS `source_reviewed_detector_false_positive` (`audit_resolved: false`).** Original `off + cut + 1` and `'claude-' + str(sid)` parameter tuple expressions are preserved without syntax camouflage; `test_token_yield_parameterized_sql_rejects_injection_metacharacters` proves `?` parameter binding against SQL-injection metacharacters in an isolated SQLite database.
  - `ADM-AUDIT-022`, `ADM-AUDIT-024` (`agent/boundaries.toml`, `agent/proof-lanes.toml`): **RETAINED OPEN.** `agent/proof-lanes.toml` documents that `contract-drift` checks TypeScript/Rust schema drift and does not itself prove all boundary manifests.

## 9. 2026-10-21 Python retirement (`claude/admiralty-rust-only-embedded-console`)

Every Python file and logic-bearing shell script was ported to Rust or
TypeScript and deleted; `PY_TO_RUST_PLAN.md` records where each went.
`PYTHON_EXCEPTIONS.md` and `tests/fixtures/python-baseline.txt` are retired and
`python_policy` now rejects any Python file or new shell script.
`agent/boundaries.toml` declares `rust-ts-vite-react-sqlite` with no Python
truth paths.

Hard blockers whose source no longer exists: rows 41 (`HLT-000` stack,
`dashboard/collector.py`), 42-48 (`HLT-001` markers in `dashboard/collector.py`),
49-51 (`HLT-033`, `dashboard/token_yield.py`), 54 (`HLT-005`, `python/`) and
55-56 (`HLT-035`, `tools/history_export_cron.sh`, now `admiralty history
publish`, which keeps the custody and allowlist checks and their tests), plus
soft row 29 (`Python containment and polyglot hygiene`).

A local read-only run of the pinned auditor (`jankurai 1.7.5`, `audit . --read-only
--no-badge --no-score-history --mode advisory`) on the working tree reports
`Python containment and polyglot hygiene` 0 → 100 and no Python caps. These rows
stay open in this record until the hosted Jankurai job on the merged head
confirms them; nothing here is marked auditor-resolved.
