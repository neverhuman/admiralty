# Python-to-Rust Migration (`JEPSON_RULING_20261018T1940Z`) — completed

Per Jepson's 2026-10-18 stack ruling (`JEPSON_RULING_20261018T1940Z_stack_rust_jankurai.md`),
Admiralty's stack is **Rust** (backend, CLI, tools, compute, smoke and release
harnesses, repository automation) and **Vite + TypeScript + React** (UI and
the few Node scripts that drive the browser or generate contracts).

The migration finished on 21 October 2026. The repository contains no Python
and no logic-bearing shell scripts. The only executable script left is the fake
provider binary `tests/qualification/codex/P08Q_fake_codex.sh`, which
qualification tests run in place of a real CLI.

`cargo test -p admiralty-testkit --test python_policy --locked` keeps it that
way: any `*.py`/`*.pyi`/`*.pyw`/`*.ipynb` file, any Python shebang and any new
shell script fails the workspace tests, and `PYTHON_EXCEPTIONS.md` and
`tests/fixtures/python-baseline.txt` may not come back. There is no exception
list to extend; changing the stack means changing that test in review.

This file was the inventory of the 42 retained Python files (11,746 lines) and
tasks `ADM-PY01` to `ADM-PY06`. It is now the record of where each one went.
Earlier revisions of this file and `PYTHON_EXCEPTIONS.md` remain in history.

## How behaviour was preserved

Every port was checked against the original before the original was deleted:

- **Differential runs.** The audit, observer, tool and qualification ports were
  run side by side with the Python originals on the same inputs. Their outputs
  were byte-identical, apart from the differences listed in each crate's
  documentation. Fixtures captured from those runs are now ordinary Rust tests.
- **Ported test suites.** Every Python test case has a Rust counterpart in the
  same crate. The file names below say where.
- **No weakened assertions.** Where a port exposed a real bug in the original
  (a crash on unexpected JSON, a non-object panic, an unsafe URL accepted), the
  Rust version fixes it and the test asserts the fixed behaviour.

## Where each file went

### `ADM-PY01` — legacy dashboard (`dashboard/*.py`)

| Retired | Replacement | Tests |
| :--- | :--- | :--- |
| `dashboard/collector.py`, `extras.py`, `rollout.py`, `token_yield.py`, `make_snapshot.py` | `admiralty-observe` (already the production producer); legacy-only cases ported into `src/legacy_cases.rs` | `crates/admiralty-observe/src/{fleet,projects,rollout,usage}/legacy_parity.rs` |
| `dashboard/serve.py`, `dashboard/run.sh`, `dashboard/index.html` | `admiraltyd` / `admiralty serve`; the React **Fleet detail** view replaces the static page and `/legacy/` redirects to it when no page is configured | `tests/integration/api_stream.rs`, `crates/admiralty-server/tests/access_projection.rs` |
| `dashboard/globalres.py` | `admiralty fleet resources` (`admiralty-fleetops::resources`) | `crates/admiralty-fleetops/src/resources/tests.rs` |

### `ADM-PY02` — check, demo and release scripts

| Retired | Replacement | Tests |
| :--- | :--- | :--- |
| `scripts/check/canary-smoke.py` | `admiralty-canary` (`admiralty-qualify::canary`) | `crates/admiralty-qualify/src/canary_tests.rs` |
| `scripts/check/excursions-smoke.py` | `admiralty-canary excursions` | `crates/admiralty-qualify/src/excursions.rs` |
| `scripts/release/package.py` | `admiralty release source-package` (`admiralty-qualify::source_package`) | `src/source_package_self_tests.rs`, `tests/python_parity.rs` |
| `scripts/release/qualify_native.py` | `admiralty release qualify-native` | `crates/admiralty-qualify/src/native_qualify.rs` |
| `scripts/demos/encode.py`, `verify.py`, `requirements.txt` | `scripts/demos/{encode,verify,media,png,gif,webp}.ts` (no Pillow) | `scripts/demos/media.test.ts` |
| `scripts/check/all.sh`, `security-supply-chain.sh`, `tools/security-lane.sh`, `scripts/dev/{demo,live}.sh` | `cargo xtask check`, `cargo xtask security`, `cargo xtask demo`, `cargo xtask live` (`admiralty-xtask`) | `crates/admiralty-xtask/tests/{cli,scratch,security}.rs` |

### `ADM-PY03` — super-captain audit

| Retired | Replacement | Tests |
| :--- | :--- | :--- |
| `tools/sc_audit.py` | `admiralty sc-audit` (`admiralty-audit`) | `crates/admiralty-audit/tests/parity/*.rs` |
| `tools/native_queue_health.py` | `admiralty queue-health` | `tests/parity/native_queue_health.rs` |
| `captains/super_captain_tick.sh` | `admiralty sc-tick` | `tests/parity/ticker.rs` |

### `ADM-PY04` — operator tools

| Retired | Replacement | Tests |
| :--- | :--- | :--- |
| `tools/rollout_observe.py` | `admiralty rollout observe` (`admiralty-tools`) | `src/rollout_observe_tests.rs` |
| `tools/rollout_packets.py` | `admiralty rollout packets` | `src/rollout_packets_tests.rs` |
| `tools/export_history.py`, `tools/history_export_cron.sh` | `admiralty history export`, `admiralty history publish` | `src/history_export_tests.rs`, `tests/history_{export,publish}.rs` |
| `tools/probe_opencode.py` | `admiralty opencode probe` (the remote workload it ships is kept byte-identical) | `src/opencode_probe_tests.rs` |
| `tools/bin/classify_screen.py`, `tools/bin/{kick,screen_send,screen_snap,status}` | `admiralty fleet classify|kick|send|snap|status` (`admiralty-fleetops`) | `crates/admiralty-fleetops/src/**/tests.rs` |
| `tools/{owner_peek,peek_all,owner_send,mr_probe,codex-luna}.sh` | `admiralty fleet peek|paste|probe-models|codex-luna` | same |
| `tools/send_{add1,add2,add2b,compute,notice2,sc}.sh` | `admiralty fleet deliver --plan crates/admiralty-fleetops/plans/<name>.json` | `src/deliver/tests.rs` |
| `tools/sweep.sh` | `admiralty fleet sweep` | `src/sweep/tests.rs` |

### `ADM-PY05` and `ADM-PY06` — Python test suites

| Retired | Rust counterpart |
| :--- | :--- |
| `tests/legacy/test_rollout.py`, `test_rollout_live_workers.py`, `test_stage_evidence_regressions.py`, `tests/fixtures/legacy/verify.py` | `crates/admiralty-observe/src/*/legacy_parity.rs`, `src/legacy_cases.rs` |
| `tests/legacy/test_rollout_observe.py`, `test_rollout_packets.py` | `crates/admiralty-tools/src/rollout_{observe,packets}_tests.rs` |
| `tests/qualification/release_evidence.py`, `test_release_evidence.py` | `admiralty release evidence`, `crates/admiralty-qualify/src/release_evidence_tests.rs` |
| `tests/test_sc_audit_*.py` (13 files), `tests/test_native_queue_health.py` | `crates/admiralty-audit/tests/parity/*.rs` (one module per original file) |

### Repository scripts moved to TypeScript

`scripts/contracts/generate.mjs`, `packages/contracts/runtime.test.mjs` and
`scripts/demos/{browser,browser.test,capture}.mjs` became `.ts` files run by
Node 24's type stripping and checked by `pnpm scripts:check`. The generated
contracts are byte-identical (their header still names `generate.mjs` so the
checked-in output does not change).

## Running the replacements on a host

Install the single `admiralty` binary (see `docs/operators/single-binary.md`),
then switch callers:

- crontab: `23 * * * * /ABS/PATH/admiralty history publish`
- super-captain ticker: `screen -dmS super_captain_tick admiralty sc-tick`
- copies of the old scripts under `/home/ubuntu/xbabe2_kicker/bin` are replaced
  by `admiralty fleet …` (`docs/operators/fleet-ops.md`).


Integration retains the already-qualified native `admiralty release evidence` and
`admiralty release qualify` commands from PR171, including bounded no-follow
evidence reads and private installation. The ported `qualify-native` harness
remains separately available; it does not replace those stronger boundaries.
