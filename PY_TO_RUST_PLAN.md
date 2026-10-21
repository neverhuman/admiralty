# Python-to-Rust Migration Plan (`JEPSON_RULING_20261018T1940Z`)

Per Jepson's 2026-10-18 stack ruling (`JEPSON_RULING_20261018T1940Z_stack_rust_jankurai.md`), Admiralty's target language stack is **Rust** (backend, CLI, tools, compute, smoke/release harnesses) and **Vite + TypeScript + React** (UI). All existing Python files are inventoried below with line counts, purpose, disposition, and assigned porting tasks. The owned production Python collector, server and observer have already been retired in favor of Rust; retained compatibility source is not authorization to restart them. Remaining ports preserve actual behavior and tests before source deletion.

## 1. Complete Inventory of Existing Python Files (`38` files, `11,439` LOC)

| Path | LOC | Purpose | Disposition | Tracker Task |
| :--- | ---: | :--- | :--- | :--- |
| `dashboard/collector.py` | 1,223 | Legacy `18890` status.json collector | Port to Rust (`admiralty-server` / `admiralty-observe`) & delete after cutover | `ADM-PY01` |
| `dashboard/extras.py` | 582 | Legacy hierarchy, resources & token efficiency collector | Port to Rust (`admiralty-server` / `admiralty-core::economics`) & delete | `ADM-PY01` |
| `dashboard/globalres.py` | 113 | Host CPU/RAM/GPU/disk snapshot helper | Rust parity landed in `admiralty-observe` (disk growth, hours-to-full, top RSS, D-state, `--resources` table); delete after captain standing orders switch to `admiralty-observe --config CONFIG --resources` | `ADM-PY01` |
| `dashboard/rollout.py` | 425 | Legacy rollout manifest loader & live worker joiner | Replace with Rust `admiralty-server` projection & delete | `ADM-PY01` |
| `dashboard/token_yield.py` | 214 | Legacy `tokens.db` yield aggregator | Replace with `admiralty-core::economics` & delete | `ADM-PY01` |
| `scripts/check/canary-smoke.py` | 1,124 | Isolated HTTP/CLI canary smoke test runner | Port to Rust binary/test in `admiralty-testkit` & delete | `ADM-PY02` |
| `scripts/demos/encode.py` | 177 | Actual production PNG-capture WebP/GIF encoder | Port to Rust subcommand in `admiralty-cli` & delete | `ADM-PY02` |
| `scripts/demos/verify.py` | 71 | Actual capture/encoded-media integrity verifier | Port to Rust subcommand in `admiralty-cli` & delete | `ADM-PY02` |
| `scripts/release/package.py` | 609 | Deterministic release tarball & manifest builder | Port to Rust `admiralty-cli release package` & delete | `ADM-PY02` |
| `tools/sc_audit.py` | 1,828 | Fleet-wide Super-Captain watchdog & stall/PR/GPU auditor | Port to Rust `admiralty-cli sc-audit` binary with parity tests & delete | `ADM-PY03` |
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
| `tests/test_sc_audit_watchdog.py` | 201 | Unit tests for `tools/sc_audit.py` watchdog | Port to Rust tests with `ADM-PY03` & delete | `ADM-PY06` |
| `tools/native_queue_health.py` | 77 | Bounded read-only native queue metadata / endpoint preconditions for retained audit | Fleet Admiral owner; port to Rust `admiralty-cli sc-audit` and delete; no transport enforcement | `ADM-PY03` |
| `tests/test_native_queue_health.py` | 235 | Synthetic/mocked draft, queue and endpoint regression controls | Fleet Admiral owner; port with Rust audit tests and delete | `ADM-PY06` |

Approved narrowly by the [21 October 2026 Admiral decision](https://github.com/neverhuman/admiralty/pull/166#issuecomment-6307405031), receipt SHA256 `cf32cfd9ec8fa87af6326f78bc7f9c11e33150bc81f4a730f64158f8413e0daf`. Owner for these two additions is the existing sole Fleet Admiral; PR166’s source writer owns this repair integration. Retirement requires Rust parity for unknown/multiline drafts, read-only bounded queue metadata, missing/corrupt/WAL data, and endpoint preconditions before deleting both compatibility files. The helper remains unwired and does not establish queue consumption or delivery authority.

### Retired on 21 October 2026

| Retired | Former path | LOC | Replacement |
| :--- | :--- | ---: | :--- |
| 2026-10-21 | `tests/qualification/release_evidence.py` | 155 | Rust `admiralty release evidence` (`crates/admiralty-cli/src/release_evidence.rs`), same gates, bounds, duplicate-key rejection, no-follow hashing and 0/1/2 exit codes |
| 2026-10-21 | `tests/qualification/test_release_evidence.py` | 92 | Ported regression cases in the same Rust module, plus missing/symlinked evidence and CLI exit-status cases |
| 2026-10-21 | `dashboard/serve.py` | 22 | `admiraltyd` (already the production server); the retired `dashboard/run.sh` launcher was removed with it |
| 2026-10-21 | `dashboard/make_snapshot.py` | 10 | Unreferenced static snapshot helper; the React build and `admiralty brief` replace it |

Rust parity for `dashboard/globalres.py` disk-growth/time-to-full estimates now
lives in `admiralty-observe` (`disk_rates.rs`); the file itself is retained until
the captain instructions and `dashboard/extras.py` stop invoking it.

## 2. Tracker Tasks for Remaining Parity and Source Retirement

| Task ID | Scope | Assigned Owner | Target |
| :--- | :--- | :--- | :--- |
| **`ADM-PY01`** | Retire `dashboard/*.py` (2,557 LOC) after remaining compatibility/parity criteria close; native production cutover is already complete | `2062267.agy_captain_admiralty` | Delete compatibility sources only after remaining parity/fixture/consumer retirement is proven |
| **`ADM-PY02`** | Port `scripts/check/*.py`, `scripts/demos/*.py`, and `scripts/release/*.py` (1,981 LOC) to Rust in `admiralty-cli` / `admiralty-testkit` | `2062267.agy_captain_admiralty` | Parity-tested Rust subcommands + delete `scripts/**/*.py` |
| **`ADM-PY03`** | Port `tools/sc_audit.py` (1,776 LOC) to Rust `admiralty-cli sc-audit` | `2062267.agy_captain_admiralty` | Parity-tested Rust binary + delete `tools/sc_audit.py` |
| **`ADM-PY04`** | Port `tools/{rollout_observe,rollout_packets,export_history,probe_opencode}.py` + `tools/bin/classify_screen.py` (1,928 LOC) to Rust | `2062267.agy_captain_admiralty` | Parity-tested Rust modules + delete `tools/**/*.py` |
| **`ADM-PY05`** | Retire `tests/legacy/*.py`, `tests/fixtures/legacy/verify.py`, and `tests/qualification/*.py` (1,737 LOC) into Rust integration tests | `2062267.agy_captain_admiralty` | Delete `tests/legacy/*.py` and `tests/qualification/*.py` with `ADM-PY01`/`ADM-PY04` |
| **`ADM-PY06`** | Port `tests/test_sc_audit_*.py` (1,028 LOC) to Rust integration tests alongside `ADM-PY03` | `2062267.agy_captain_admiralty` | Delete `tests/test_sc_audit_*.py` with `ADM-PY03` |

## 3. Retired (21 October 2026)

Deleted with Rust replacements and tests; the policy ceiling in `tests/fixtures/python-baseline.txt` dropped by the same four paths. Listed as bullets so the policy inventory parser does not treat them as current files.

- dashboard/serve.py (22 LOC) and the dashboard/run.sh launcher: superseded by `admiraltyd`, which already served production; nothing referenced them except the pinned legacy source manifest, which reads its historical base commit.
- dashboard/make_snapshot.py (10 LOC): unreferenced; the React build and `/v1/legacy*` routes replace the inline HTML snapshot.
- scripts/check/excursions-smoke.py (143 LOC): ported to `crates/admiralty-server/tests/excursions_binary.rs` (same requests and assertions against the real binary, including restart). Unlike the script, it now runs in every `cargo test`.
- scripts/release/qualify_native.py (188 LOC): ported to `admiralty release qualify` (`crates/admiralty-cli/src/release_ops.rs`) with an end-to-end test (`crates/admiralty-cli/tests/release_qualify.rs`, run with `--ignored`).
- The inline Python in `scripts/dev/live.sh` that created private preview state: replaced by `admiralty preview-state --dir DIR` with the same ownership, mode and exclusive-create rules. `pnpm live` no longer needs Python.
