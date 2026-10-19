# CI runner pool

Both jobs in `.github/workflows/ci.yml` select the existing organization runner
group `neverhuman-lan` with the shared labels `self-hosted`, `Linux`, `X64` and
`lan-ci`. GitHub schedules any available matching runner. The workflow contains
no machine names, OS-image-specific labels or dedicated project runner labels.

The general launcher was inspected read-only on xbabe1, xbabe2 and xbabe3 on
2026-10-18. All three have the same `/opt/neverhuman-actions/launch.sh` SHA-256
`e51ee6d1f10faf7e1b051e2cf0b803bfb6b7cf67135effd413af253412dcb996`
and `common.sh` SHA-256
`06eaeb81a3dd5fb3831b69ab7d71448ff958ca1d5656b951097c6801e0ccd92e`.
Their registration payload specifies runner group ID `3`, shared labels
`self-hosted`, `linux`, `x64`, `lan-ci`, and a configured OS label. Browser
images add a separate browser label. The organization UI identifies group `3`
as `neverhuman-lan` and displays the default platform labels as `Linux` and `X64`.

The launcher creates a new ephemeral VM and runner per job without mounting
host files. Its common configuration admits the three configured physical
hosts. This confirms their pool configuration; service readiness and available
capacity remain live observations. At inspection, xbabe1/2 had active general
lanes, while xbabe3's lanes were in auto-restart. Enabling this workflow does
not repair, install, reconfigure or restart runner services.

## Repository eligibility

The group's existing policy uses selected repositories, already allows public
repositories, and allows all workflows. Admiralty must be selected in
[the existing runner group](https://github.com/organizations/neverhuman/settings/actions/runner-groups/3).
On 2026-10-18, `neverhuman/admiralty` was added, preserving all existing selections
and the existing public-repository/workflow policy. The settings UI confirmed
`Runner group updated` and seven selected repositories. Before this change the six selected
repositories were `ai-veox-app`, `demo-repository`, `dope`, `jailgun`, `jarambit`
and `joil`. Runner administration and credentials remain with the existing
pool operators. Normal GitHub event and fork approval policy still applies.

The local `gh` credential can read the repository runner listing but receives
HTTP 403 for organization runner administration because it lacks
`admin:org`. The already authenticated organization settings UI provides the
runner labels and repository selection. An empty repository runner listing
before admission is not proof that the organization pool is absent.

## Checks and job isolation

The Rust/legacy and contracts/dashboard checks retain their existing triggers,
job names, timeouts, pinned checkout/setup-node actions, pinned Rust/Node/pnpm,
read-only workflow token and concurrency cancellation. Private test scratch and
actual isolated-binary smoke scratch use a fresh owner-only canonical `/tmp`
directory, with ancestor custody recorded by CI. Rust homes and pinned pnpm use
job-owned directories under `RUNNER_TEMP`. Pinned pnpm is
installed into a job-owned prefix under `RUNNER_TEMP`, with its `bin` directory
added through `GITHUB_PATH`; the workflow does not modify a shared global npm
prefix. Each job prints the selected runner name, OS and architecture so a run
records actual placement rather than assuming a physical host.

The Rust job sets `CARGO_BUILD_JOBS=2` for compilation and linking.
`RUST_TEST_THREADS=1` separately controls test execution. The receiving-main
`3fdb101` job on `xbabe3-lan-1-20261019003841-10799` exceeded its existing
fifteen-minute deadline while Cargo was still compiling test binaries; no test
assertion had run. Its native annotation confirms the timeout, and cleanup
recorded multiple active compiler/linker children. The explicit build bound
limits concurrent compiler memory and I/O without changing runner limits,
test coverage or deadlines. A successful later run must establish whether it
resolves that runner's delay; the configuration alone is not a passing result.

Validate the workflow locally with:

```sh
rtk proxy mkdir -p target/research/ci-runners
rtk proxy python3 -c 'from pathlib import Path; Path("target/research/ci-runners/actionlint.yaml").write_text("self-hosted-runner:\n  labels:\n    - lan-ci\n")'
rtk proxy actionlint -config-file target/research/ci-runners/actionlint.yaml .github/workflows/ci.yml
rtk proxy git diff --check -- .github/workflows/ci.yml docs/operators/ci-runners.md
```

After the workflow reaches GitHub, inspect a real run's job details or the
`Record selected runner` step to confirm placement and check results. A static
workflow check or repository allowlist update does not prove a job dispatched,
passed, or ran on every eligible host. Do not turn a temporary pool readiness
issue into a host-specific workflow selector.
