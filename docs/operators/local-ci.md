# Local application CI

Use Node24, pnpm11.15.1, the pinned Rust toolchain and Python3. Install Chrome or
Chromium for the browser journeys; `ADMIRALTY_BROWSER_BINARY` can select an
existing executable. From the repository root, run:

```sh
rtk proxy pnpm check
```

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
workspace Clippy, all Rust tests, legacy and audit tests, fixture verification,
release-package and canary self-tests, actual native binary builds and smoke,
Rust/JavaScript contracts, frontend unit tests, both typechecks, the production
build and real browser/accessibility journeys. Browser tests use the freshly
built daemon and observer, hash-pin the daemon and verify the pinned axe bundle.
Local qualification does not claim a remote Linux run or platform-gated cases
that did not execute on this host.

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

Compiler jobs stay at two and Rust test threads at one. Set
`ADMIRALTY_TARGET_DIR` to an absolute private build directory to reuse a warm
target. The script ignores a workstation-wide `CARGO_TARGET_DIR`, which may
point at an unavailable mount. It creates short canonical private scratch for
Unix sockets and restores the caller's umask before tests, keeping permissive
umask regressions visible.

Each stage fails immediately on a nonzero exit. The command prints its private
log directory and retains commands, source/working-tree status, tool versions
and one log per stage. A dirty working-tree run is useful feedback, but does not
establish qualification of the printed commit alone. Inspect and fix failures
locally before pushing; preserve original failures alongside later passes.

`ADMIRALTY_CHECK_SCRATCH_ROOT` may select an existing absolute temporary
directory outside Git when the default root disk is full. The command still
creates its own private subdirectory and retains every stage/log and the same
concurrency limits and assertions. For example, use an adequately sized local
tmpfs on a disposable Linux test host; this does not qualify physical-disk
crash behavior or change the separate `/dev/shm` capture-fixture requirement.
