# Local application CI

Use Node24, pnpm11.15.1, the pinned Rust toolchain and Python3. Install Chrome or
Chromium for the browser journeys; `ADMIRALTY_BROWSER_BINARY` can select an
existing executable. From the repository root, run:

```sh
rtk proxy pnpm check
```

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
