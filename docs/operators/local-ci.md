# Local application CI

Use Node24, pnpm11.15.1 and the pinned Rust toolchain. Install Chrome or
Chromium for the browser journeys; `ADMIRALTY_BROWSER_BINARY` can select an
existing executable. From the repository root, run:

```sh
rtk proxy pnpm check
```

`pnpm check` runs `cargo xtask check`, implemented in Rust by the
`admiralty-xtask` crate (the `xtask` alias is declared in `.cargo/config.toml`
as `run --quiet --locked -p admiralty-xtask --`). Both spellings accept the same
flags; pnpm forwards them, for example `pnpm check --list`. Building the xtask
binary itself uses the caller's Cargo environment.

| Former script | Command |
| --- | --- |
| `bash scripts/check/all.sh` | `cargo xtask check` (`pnpm check`) |
| `bash scripts/check/security-supply-chain.sh`, `bash tools/security-lane.sh` | `cargo xtask security` |
| `bash scripts/dev/demo.sh` | `cargo xtask demo` (`pnpm demo`) |
| `bash scripts/dev/live.sh --status … --index …` | `cargo xtask live --status … [--index …] [--models …]` (`pnpm live …`) |

## Stages

`cargo xtask check --list` prints the numbered matrix:

| Stage | Command |
| --- | --- |
| 01 | `pnpm install --frozen-lockfile --ignore-scripts` |
| 02 | `cargo fmt --all -- --check` |
| 03 | `cargo clippy --workspace --all-targets --locked -- -D warnings` |
| 04 | `cargo test --workspace --locked` |
| 05 | `cargo build -p admiralty-server -p admiralty-cli -p admiralty-observe -p admiralty-qualify --locked` |
| 06 | `$CARGO_TARGET_DIR/debug/admiralty-canary --admiraltyd-binary $CARGO_TARGET_DIR/debug/admiraltyd --admiralty-binary $CARGO_TARGET_DIR/debug/admiralty --scratch-root $SCRATCH/canary` |
| 07 | `cargo run -p admiralty-cli --locked -- contracts check` |
| 08 | `pnpm contracts:check` |
| 09 | `pnpm --dir packages/contracts test` |
| 10 | `pnpm scripts:check` (TypeScript check of `scripts/` and the demo media tests) |
| 11 | `pnpm --dir apps/dashboard typecheck` |
| 12 | `pnpm --dir apps/dashboard test --run` |
| 13 | `pnpm --dir apps/dashboard exec tsc -p e2e/tsconfig.json --noEmit` |
| 14 | `env NODE_ENV=production pnpm --dir apps/dashboard build` |
| 15 | In-process SHA-256 pin of `node_modules/axe-core/axe.min.js` |
| 16 | `pnpm --dir apps/dashboard test --run --config e2e/vitest.config.ts` |

The former Python stages (legacy and audit unit tests, fixture verification and
the release-package and canary self-tests) were ported to Rust. Their tests now
run in stage04, and stage06 smokes the real binaries with the Rust canary.
Stage16 receives `ADMIRALTY_E2E_ADMIRALTYD_BIN`, `ADMIRALTYD_BINARY`,
`ADMIRALTY_E2E_OBSERVER_BIN` and `ADMIRALTY_E2E_ADMIRALTYD_SHA256`. The daemon
hash is computed in Rust from the freshly built binary.

Flags for focused reruns:

- `--list` prints the stages and exits without checking tools.
- `--from N` starts at stageN. `--only N` runs only stageN and may be
  repeated. They cannot be combined. Stage numbers and log names stay fixed.
- `--skip-browser` skips stage16 on hosts without Chrome or Chromium. It
  prints a `16 SKIPPED …` line.

A complete run ends with `PASS: all 16 local CI stages; logs retained in …`. Any
partial run (`--from`, `--only` or `--skip-browser`) instead ends with
`PASS: <n> of 16 local CI stages (ran …; not run …); NOT a complete local CI
matrix; …`. A partial pass is useful feedback, not qualification.

Hosted browser qualification pins Node24.22.0 and Chrome for Testing155.0.8059.84
for Linux x64. It downloads the exact archive into private job scratch, verifies
SHA256 before extraction and asserts the browser's actual CDP version. Runner
caches and a floating stable channel do not select the qualification browser.
When updating it, qualify the replacement locally and update both the version
and archive hash in `.github/workflows/dashboard-e2e.yml`. A local run may use
another installed browser; set `ADMIRALTY_E2E_BROWSER_VERSION` to require an
exact CDP version and retain the actual version with its evidence.

On Linux, the real capture-overflow fixture requires `/dev/shm` (or its
canonical system alias) on tmpfs with at least32MiB available. It creates a
private0700 directory and fails clearly if that prerequisite is unavailable.
This one fixture checks real producer failure and the capture byte limit while
retaining all journal syncs and its30-second deadline. Other journal tests keep
their normal disk-backed fixtures. It is not a physical-storage throughput or
power-loss test. A failed prerequisite after directory creation can retain the
private empty directory for diagnosis; verify ownership before cleanup.

The command installs frozen JavaScript dependencies and runs formatting, strict
workspace Clippy, all Rust tests (including the ported legacy, audit, fixture,
release-package and canary tests), actual native binary builds and smoke,
Rust/JavaScript contracts, the repository-script typecheck, frontend unit tests,
both dashboard typechecks, the production build and real browser/accessibility
journeys. Browser tests use the freshly built daemon and observer, hash-pin the
daemon and verify the pinned axe bundle. Local qualification does not claim a
remote Linux run or platform-gated cases that did not execute on this host.

The separate pinned Linux Jankurai workflow reports the onboarding baseline in
advisory mode under explicit Admiral decision SC-DEC-186J. Tool/provisioning and
missing-report errors still fail. The original standard-policy score39 and
56blockers remain unresolved in the
[owned cleanup backlog](../../JANKURAI_CLEANUP_BACKLOG.md); an advisory pass is
not strict conformance. The Rust `python_policy` integration target runs in both
this local matrix and hosted Rust CI, rejecting net-new Python, missing/stale or
duplicate exceptions and incorrect migration counts. Its reviewed 40-path
ceiling requires an explicit maintainer exception before expansion. The
source-bound
[85-finding triage tracker](../../receipts/integration/jankurai-triage-20261019.json)
records owners, next actions, evidence-backed detector reviews and checkpoint
ETAs. Reviewed false positives are not silently removed from audit results.

## Environment and logs

Compiler jobs stay at two and Rust test threads at one: every stage receives
`CARGO_BUILD_JOBS=2`, `CARGO_PROFILE_DEV_DEBUG=0`, `CARGO_PROFILE_TEST_DEBUG=0`
and `RUST_TEST_THREADS=1`. Set `ADMIRALTY_TARGET_DIR` to an absolute private
build directory to reuse a warm target; a relative value is refused. The
command ignores a workstation-wide `CARGO_TARGET_DIR` for its stages, which
may point at an unavailable mount. It creates short canonical private scratch
(`/tmp/adm.XXXXXX`, or `/var/tmp` when `/tmp/.git` exists) under umask077 with
`logs/` and `canary/`. It then restores the caller's umask before any stage,
keeping permissive umask regressions visible, and exports `TMPDIR` to that
scratch. Stages run with closed stdin and through `rtk proxy` when `rtk` is on
`PATH`.

Node24 is required by default, matching `.github/workflows/ci.yml`. For local
experiments on another major, `ADMIRALTY_ALLOW_NODE_MAJOR=<n>` requires that
major instead. The command warns that the run is not CI-equivalent and records
`node_major_override=<n>` in `environment.log`. pnpm must be exactly 11.15.1.

Each stage fails immediately on a nonzero exit. The command prints its private
log directory and retains `environment.log` (start time, commit,
working-tree status, tool versions, target, build jobs, test threads and caller
umask), `commands.log` and one `NN.log` per stage. A failure prints
`NN FAIL exit=<code> log=<path>` and the last 80 log lines to stderr, then
exits with the stage's status. A dirty working-tree run is useful feedback, but
does not establish qualification of the printed commit alone. Inspect and fix
failures locally before pushing; preserve original failures alongside later
passes.

## Supply-chain lane

`cargo xtask security` runs the fail-closed lane used by
`.github/workflows/security.yml`. It refuses a symlinked `TMPDIR` before
anything else and requires `cargo`, `git`, `actionlint` and `gitleaks` on
`PATH`, with no missing-tool skip. `$HOME/.cargo/bin` and `$HOME/.local/bin`
are prepended unless `ADMIRALTY_SECURITY_NO_DEFAULT_PATH=1`. The lane checks
that `Cargo.lock` and `pnpm-lock.yaml` are non-empty and runs
`cargo metadata --locked`. Every workflow must contain `permissions:` and
`persist-credentials: false`. It then runs
`actionlint -config-file .github/actionlint.yaml` and a gitleaks history scan.
`ADMIRALTY_SECURITY_SCAN_BASE` must be a complete object ID: all zeros scans
`HEAD -m`, any other base must be an available ancestor and scans
`<base>..HEAD -m`, and without a base the scan covers `-1 -m`. Finally it writes
SHA-256 digests in `sha256sum` format to
`supply-chain-digests.sha256` in a fresh private0700 directory. `cargo audit`,
`pnpm audit`, `pip-audit` and `grype` are not claimed by this lane.
