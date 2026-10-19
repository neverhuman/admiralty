# Python-to-Rust Migration Plan (`JEPSON_RULING_20261018T1940Z`)

Per Jepson's 2026-10-18 stack ruling (`JEPSON_RULING_20261018T1940Z_stack_rust_jankurai.md`), Admiralty's target language stack is **Rust** (backend, CLI, tools, compute, smoke/release harnesses) and **Vite + TypeScript + React** (UI). All existing Python files are inventoried below with line counts, purpose, disposition, and assigned porting tasks.

## 1. Complete Inventory of Existing Python Files (`40` files, `11,351` LOC)

| Path | LOC | Purpose | Disposition | Tracker Task |
| :--- | ---: | :--- | :--- | :--- |
| `dashboard/collector.py` | 1,223 | Legacy `18890` status.json collector | Port to Rust (`admiralty-server` / `admiralty-observe`) & delete after cutover | `ADM-PY01` |
| `dashboard/extras.py` | 582 | Legacy hierarchy, resources & token efficiency collector | Port to Rust (`admiralty-server` / `admiralty-core::economics`) & delete | `ADM-PY01` |
| `dashboard/globalres.py` | 113 | Host CPU/RAM/GPU/disk snapshot helper | Port to Rust (`admiralty-runtime::reservations`) & delete | `ADM-PY01` |
| `dashboard/make_snapshot.py` | 10 | Static HTML snapshot generator | Replace with Rust CLI / React build & delete | `ADM-PY01` |
| `dashboard/rollout.py` | 425 | Legacy rollout manifest loader & live worker joiner | Replace with Rust `admiralty-server` projection & delete | `ADM-PY01` |
| `dashboard/serve.py` | 22 | Legacy `18890` static HTTP server | Replace with `admiralty-server` (`admiraltyd`) & delete | `ADM-PY01` |
| `dashboard/token_yield.py` | 214 | Legacy `tokens.db` yield aggregator | Replace with `admiralty-core::economics` & delete | `ADM-PY01` |
| `scripts/check/canary-smoke.py` | 1,105 | Isolated HTTP/CLI canary smoke test runner | Port to Rust binary/test in `admiralty-testkit` & delete | `ADM-PY02` |
| `scripts/check/excursions-smoke.py` | 143 | Isolated excursions CLI/HTTP smoke test runner | Port to Rust test in `admiralty-testkit` & delete | `ADM-PY02` |
| `scripts/demos/encode.py` | 177 | Deterministic demo fixture encoder | Port to Rust subcommand in `admiralty-cli` & delete | `ADM-PY02` |
| `scripts/demos/verify.py` | 71 | Deterministic demo fixture verifier | Port to Rust subcommand in `admiralty-cli` & delete | `ADM-PY02` |
| `scripts/release/package.py` | 609 | Deterministic release tarball & manifest builder | Port to Rust `admiralty-cli release package` & delete | `ADM-PY02` |
| `scripts/release/qualify_native.py` | 188 | Native release qualification runner | Port to Rust `admiralty-cli release qualify` & delete | `ADM-PY02` |
| `tools/sc_audit.py` | 1,776 | Fleet-wide Super-Captain watchdog & stall/PR/GPU auditor | Port to Rust `admiralty-cli sc-audit` binary with parity tests & delete | `ADM-PY03` |
| `tools/rollout_observe.py` | 521 | Bounded screen/worktree metadata observer | Port to Rust `admiralty-observe` / `admiralty-runtime::workspace::registry` & delete | `ADM-PY04` |
| `tools/rollout_packets.py` | 559 | Rollout packet manifest & capsule exporter | Port to Rust `admiralty-cli workplans` & delete | `ADM-PY04` |
| `tools/bin/classify_screen.py` | 357 | Screen process tree classifier | Port to Rust `admiralty-runtime::providers::observation` & delete | `ADM-PY04` |
| `tools/export_history.py` | 316 | Historical status JSONL exporter | Port to Rust `admiralty-store` export command & delete | `ADM-PY04` |
| `tools/probe_opencode.py` | 175 | OpenCode SSH capability probe | Port to Rust `admiralty-runtime::providers::opencode_ssh` & delete | `ADM-PY04` |
| `tests/fixtures/legacy/verify.py` | 135 | Legacy fixture verifier | Port to Rust integration test in `admiralty-testkit` & delete | `ADM-PY05` |
| `tests/legacy/test_rollout.py` | 363 | Unit tests for `dashboard/rollout.py` | Retire with `dashboard/rollout.py` after Rust parity (`ADM-PY01`) | `ADM-PY05` |
| `tests/legacy/test_rollout_live_workers.py` | 213 | Unit tests for `dashboard/rollout.py` live workers | Retire with `dashboard/rollout.py` after Rust parity (`ADM-PY01`) | `ADM-PY05` |
| `tests/legacy/test_rollout_observe.py` | 301 | Unit tests for `tools/rollout_observe.py` | Port to Rust tests with `ADM-PY04` & delete | `ADM-PY05` |
| `tests/legacy/test_rollout_packets.py` | 239 | Unit tests for `tools/rollout_packets.py` | Port to Rust tests with `ADM-PY04` & delete | `ADM-PY05` |
| `tests/legacy/test_stage_evidence_regressions.py` | 239 | Unit tests for `dashboard/collector.py` stage evidence | Port to Rust tests with `ADM-PY01` & delete | `ADM-PY05` |
| `tests/qualification/release_evidence.py` | 155 | Release qualification evidence verifier | Port to Rust integration test in `admiralty-testkit` & delete | `ADM-PY05` |
| `tests/qualification/test_release_evidence.py` | 92 | Unit tests for `tests/qualification/release_evidence.py` | Port to Rust integration test in `admiralty-testkit` & delete | `ADM-PY05` |
| `tests/test_sc_audit_ci_queue.py` | 20 | Unit tests for `tools/sc_audit.py` CI queue | Port to Rust tests with `ADM-PY03` & delete | `ADM-PY06` |
| `tests/test_sc_audit_dope.py` | 246 | Unit tests for `tools/sc_audit.py` DOPE adapter | Port to Rust tests with `ADM-PY03` & delete | `ADM-PY06` |
| `tests/test_sc_audit_gpu.py` | 37 | Unit tests for `tools/sc_audit.py` GPU checks | Port to Rust tests with `ADM-PY03` & delete | `ADM-PY06` |
| `tests/test_sc_audit_integrity.py` | 194 | Unit tests for `tools/sc_audit.py` integrity | Port to Rust tests with `ADM-PY03` & delete | `ADM-PY06` |
| `tests/test_sc_audit_jepson.py` | 32 | Unit tests for `tools/sc_audit.py` Jepson rules | Port to Rust tests with `ADM-PY03` & delete | `ADM-PY06` |
| `tests/test_sc_audit_joil_flip.py` | 99 | Unit tests for `tools/sc_audit.py` JOIL flip | Port to Rust tests with `ADM-PY03` & delete | `ADM-PY06` |
| `tests/test_sc_audit_jopedime_mr.py` | 108 | Unit tests for `tools/sc_audit.py` JopeDime MR | Port to Rust tests with `ADM-PY03` & delete | `ADM-PY06` |
| `tests/test_sc_audit_stale_pr.py` | 34 | Unit tests for `tools/sc_audit.py` stale PRs | Port to Rust tests with `ADM-PY03` & delete | `ADM-PY06` |
| `tests/test_sc_audit_stall.py` | 41 | Unit tests for `tools/sc_audit.py` stall clock | Port to Rust tests with `ADM-PY03` & delete | `ADM-PY06` |
| `tests/test_sc_audit_stall_rollback.py` | 43 | Unit tests for `tools/sc_audit.py` stall rollback | Port to Rust tests with `ADM-PY03` & delete | `ADM-PY06` |
| `tests/test_sc_audit_stdin.py` | 24 | Unit tests for `tools/sc_audit.py` stdin hygiene | Port to Rust tests with `ADM-PY03` & delete | `ADM-PY06` |
| `tests/test_sc_audit_survey.py` | 17 | Unit tests for `tools/sc_audit.py` survey | Port to Rust tests with `ADM-PY03` & delete | `ADM-PY06` |
| `tests/test_sc_audit_watchdog.py` | 133 | Unit tests for `tools/sc_audit.py` watchdog | Port to Rust tests with `ADM-PY03` & delete | `ADM-PY06` |

## 2. Tracker Tasks for Rust Porting (Post-Cutover on Spare Workers)

| Task ID | Scope | Assigned Owner | Target |
| :--- | :--- | :--- | :--- |
| **`ADM-PY01`** | Retire `dashboard/*.py` (2,589 LOC) once native `admiralty-server` + `admiralty-observe` + React UI cutover is finalized | `2062267.agy_captain_admiralty` | Delete `dashboard/*.py` after legacy collector retirement |
| **`ADM-PY02`** | Port `scripts/check/*.py`, `scripts/demos/*.py`, and `scripts/release/*.py` (2,293 LOC) to Rust in `admiralty-cli` / `admiralty-testkit` | `2062267.agy_captain_admiralty` | Parity-tested Rust subcommands + delete `scripts/**/*.py` |
| **`ADM-PY03`** | Port `tools/sc_audit.py` (1,776 LOC) to Rust `admiralty-cli sc-audit` | `2062267.agy_captain_admiralty` | Parity-tested Rust binary + delete `tools/sc_audit.py` |
| **`ADM-PY04`** | Port `tools/{rollout_observe,rollout_packets,export_history,probe_opencode}.py` + `tools/bin/classify_screen.py` (1,928 LOC) to Rust | `2062267.agy_captain_admiralty` | Parity-tested Rust modules + delete `tools/**/*.py` |
| **`ADM-PY05`** | Retire `tests/legacy/*.py`, `tests/fixtures/legacy/verify.py`, and `tests/qualification/*.py` (1,737 LOC) into Rust integration tests | `2062267.agy_captain_admiralty` | Delete `tests/legacy/*.py` and `tests/qualification/*.py` with `ADM-PY01`/`ADM-PY04` |
| **`ADM-PY06`** | Port `tests/test_sc_audit_*.py` (1,028 LOC) to Rust integration tests alongside `ADM-PY03` | `2062267.agy_captain_admiralty` | Delete `tests/test_sc_audit_*.py` with `ADM-PY03` |
