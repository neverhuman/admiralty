# Testing and local repair

Run `pnpm check` from the repository root with Node24 and pnpm11.15.1. It invokes
the complete [local application CI command](operators/local-ci.md), retaining
source/tool metadata, commands and per-stage logs in short private scratch.
Use `ADMIRALTY_TARGET_DIR` for an absolute private warm target. `cargo xtask
check` bounds compiler jobs to two and Rust test threads to one; `--list` shows
its stages and `--from`/`--only`/`--skip-browser` select them.

For a source change, first run the smallest affected registered target, then the
required wider matrix. Typical commands are `cargo test -p admiralty-server
--lib <filter> --locked`, `pnpm --dir apps/dashboard test --run` and
`pnpm contracts:check`. Actual binary canaries and browser/axe journeys are
included in the full command. Consult [integration quality](operators/integration-quality.md)
and the [deterministic testkit](architecture/deterministic-testkit.md) for proof
boundaries. Existing integration tests do not establish every property,
provider, performance, recovery or soak criterion in the master plan.

Run `actionlint` for workflow edits; `.github/actionlint.yaml` declares the
actual custom LAN runner labels without suppressing other diagnostics. Untrusted
fork pull requests cannot schedule jobs on the private LAN runner group.
The pinned Linux Jankurai workflow retains standard-mode failures and report
artifacts. Classify each finding with source evidence and an owner; synthetic
negative fixtures are not live credentials, and passing application tests do
not dismiss genuine audit findings.

For failures, preserve the first log and source identity. Record timeout,
assertion, missing prerequisite and unknown result distinctly. Fix the cause;
repeat only affected or newly changed qualification. Report actual pass/fail/
ignored counts, command/CWD/toolchain/resource limits and artifact hashes.
