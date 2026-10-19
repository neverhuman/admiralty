# Python Exceptions (`JEPSON_RULING_20261018T1940Z`)

Per Jepson's 2026-10-18 stack ruling, Admiralty's allowed language stack is **Rust + Vite/TypeScript/React**. No new Python files are permitted. The existing 40 legacy/smoke/qualification Python files below are temporarily retained solely to keep legacy dashboard tests, isolated binary smoke harnesses, release qualification scripts, and the `sc_audit` watchdog operational as compatibility/parity source after the owned production collector/server/observer already moved to Rust, and are scheduled for Rust replacement and deletion in [`PY_TO_RUST_PLAN.md`](PY_TO_RUST_PLAN.md).

`cargo test -p admiralty-testkit --test python_policy --locked` enforces the
reviewed ceiling in `tests/fixtures/python-baseline.txt` and exact exception/plan
coverage. It runs inside `pnpm check` and hosted workspace tests. A new exception
row cannot approve a new Python file; expanding the ceiling requires the
maintainer's explicit exception and ordinary exact-head review.

| File | One-Line Justification (Temporary Until Rust Port Completes) |
| :--- | :--- |
| `dashboard/collector.py` | Retired owned production collector; source retained for compatibility/parity tests, with no production reactivation (`ADM-PY01`). |
| `dashboard/extras.py` | Compatibility hierarchy/resource helper retained for remaining source parity; production feed is Rust (`ADM-PY01`). |
| `dashboard/globalres.py` | Legacy `18890` host resource probe retained until `admiralty-runtime::reservations` replaces it (`ADM-PY01`). |
| `dashboard/make_snapshot.py` | Legacy static HTML snapshot helper retained until React export replaces it (`ADM-PY01`). |
| `dashboard/rollout.py` | Legacy rollout manifest loader retained until `admiralty-server` native rollout projection replaces it (`ADM-PY01`). |
| `dashboard/serve.py` | Retired legacy HTTP server retained for compatibility/fixture retirement; Rust serves production (`ADM-PY01`). |
| `dashboard/token_yield.py` | Legacy SQLite token-yield aggregator retained until `admiralty-core::economics` native explorer cutover (`ADM-PY01`). |
| `scripts/check/canary-smoke.py` | Isolated HTTP/CLI canary smoke harness retained until ported to `admiralty-cli` / `admiralty-testkit` (`ADM-PY02`). |
| `scripts/check/excursions-smoke.py` | Isolated excursions CLI/HTTP smoke harness retained until ported to Rust in `admiralty-testkit` (`ADM-PY02`). |
| `scripts/demos/encode.py` | Actual production PNG-capture WebP/GIF encoder retained until ported to Rust `admiralty-cli` subcommand (`ADM-PY02`). |
| `scripts/demos/verify.py` | Actual capture/encoded-media integrity verifier retained until ported to Rust `admiralty-cli` subcommand (`ADM-PY02`). |
| `scripts/release/package.py` | Deterministic release archive builder retained until ported to Rust `admiralty-cli` (`ADM-PY02`). |
| `scripts/release/qualify_native.py` | Native release qualification helper retained until ported to Rust `admiralty-cli` (`ADM-PY02`). |
| `tools/sc_audit.py` | Fleet-wide Super-Captain watchdog retained until ported to a native Rust CLI binary (`ADM-PY03`). |
| `tools/rollout_observe.py` | Compatibility observer source retained for parity/shared callers; the owned production observer is retired (`ADM-PY04`). |
| `tools/rollout_packets.py` | Legacy packet-manifest reconciler retained until native `admiralty-cli workplans` replaces it (`ADM-PY04`). |
| `tools/bin/classify_screen.py` | Legacy screen classifier helper retained until native session supervision replaces it (`ADM-PY04`). |
| `tools/export_history.py` | Legacy history JSONL exporter retained until native store export replaces it (`ADM-PY04`). |
| `tools/probe_opencode.py` | Legacy OpenCode SSH probe retained until `admiralty-runtime::providers::opencode_ssh` replaces it (`ADM-PY04`). |
| `tests/fixtures/legacy/verify.py` | Legacy fixture verifier retained only while legacy dashboard files exist (`ADM-PY05`). |
| `tests/legacy/test_rollout.py` | Regression suite for `dashboard/rollout.py`; retired when `dashboard/*.py` is replaced by Rust (`ADM-PY05`). |
| `tests/legacy/test_rollout_live_workers.py` | Regression suite for `dashboard/rollout.py`; retired when `dashboard/*.py` is replaced by Rust (`ADM-PY05`). |
| `tests/legacy/test_rollout_observe.py` | Regression suite for `tools/rollout_observe.py`; retired when `tools/*.py` is ported to Rust (`ADM-PY05`). |
| `tests/legacy/test_rollout_packets.py` | Regression suite for `tools/rollout_packets.py`; retired when `tools/*.py` is ported to Rust (`ADM-PY05`). |
| `tests/legacy/test_stage_evidence_regressions.py` | Regression suite for `dashboard/collector.py`; retired when `dashboard/*.py` is replaced by Rust (`ADM-PY05`). |
| `tests/qualification/release_evidence.py` | Release qualification evidence verifier retained until ported to Rust integration tests (`ADM-PY05`). |
| `tests/qualification/test_release_evidence.py` | Unit tests for `tests/qualification/release_evidence.py`; ported to Rust integration tests (`ADM-PY05`). |
| `tests/test_sc_audit_ci_queue.py` | Parity test for `tools/sc_audit.py`; ported to Rust test suite with `ADM-PY03` (`ADM-PY06`). |
| `tests/test_sc_audit_dope.py` | Parity test for `tools/sc_audit.py`; ported to Rust test suite with `ADM-PY03` (`ADM-PY06`). |
| `tests/test_sc_audit_gpu.py` | Parity test for `tools/sc_audit.py`; ported to Rust test suite with `ADM-PY03` (`ADM-PY06`). |
| `tests/test_sc_audit_integrity.py` | Parity test for `tools/sc_audit.py`; ported to Rust test suite with `ADM-PY03` (`ADM-PY06`). |
| `tests/test_sc_audit_jepson.py` | Parity test for `tools/sc_audit.py`; ported to Rust test suite with `ADM-PY03` (`ADM-PY06`). |
| `tests/test_sc_audit_joil_flip.py` | Parity test for `tools/sc_audit.py`; ported to Rust test suite with `ADM-PY03` (`ADM-PY06`). |
| `tests/test_sc_audit_jopedime_mr.py` | Parity test for `tools/sc_audit.py`; ported to Rust test suite with `ADM-PY03` (`ADM-PY06`). |
| `tests/test_sc_audit_stale_pr.py` | Parity test for `tools/sc_audit.py`; ported to Rust test suite with `ADM-PY03` (`ADM-PY06`). |
| `tests/test_sc_audit_stall.py` | Parity test for `tools/sc_audit.py`; ported to Rust test suite with `ADM-PY03` (`ADM-PY06`). |
| `tests/test_sc_audit_stall_rollback.py` | Parity test for `tools/sc_audit.py` stall rollback; ported to Rust test suite with `ADM-PY03` (`ADM-PY06`). |
| `tests/test_sc_audit_stdin.py` | Parity test for `tools/sc_audit.py`; ported to Rust test suite with `ADM-PY03` (`ADM-PY06`). |
| `tests/test_sc_audit_survey.py` | Parity test for `tools/sc_audit.py`; ported to Rust test suite with `ADM-PY03` (`ADM-PY06`). |
| `tests/test_sc_audit_watchdog.py` | Parity test for `tools/sc_audit.py`; ported to Rust test suite with `ADM-PY03` (`ADM-PY06`). |
