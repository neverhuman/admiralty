# Full local CI

Use Node24, pnpm11.15.1, the pinned Rust toolchain and Python3. Install Chrome or
Chromium for the browser journeys; `ADMIRALTY_BROWSER_BINARY` can select an
existing executable. From the repository root, run:

```sh
rtk proxy bash scripts/check/all.sh
```

The command installs frozen JavaScript dependencies and runs formatting, strict
workspace Clippy, all Rust tests, legacy and audit tests, fixture verification,
release-package and canary self-tests, actual native binary builds and smoke,
Rust/JavaScript contracts, frontend unit tests, both typechecks, the production
build and real browser/accessibility journeys. Browser tests use the freshly
built daemon and observer, hash-pin the daemon and verify the pinned axe bundle.
Local qualification does not claim a remote Linux run or platform-gated cases
that did not execute on this host.

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
