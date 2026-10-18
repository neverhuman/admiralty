# Native resource admission result

Branch: `codex/native-resource-admission`, rebased onto current `origin/main`.

## Implemented

Added `crates/admiralty-runtime/src/resource_gate.rs` with a fail-closed,
non-deserializable permit token and exact project/task/attempt/node/lease,
launch digest, host boot incarnation, expiry, CPU, memory, task, and concurrency
checks. The only backend is a delegated Linux systemd/cgroup v2 service using a
direct argv vector. It reads back the admitted cgroup's effective controls and
process membership, rejects paths outside the validated subtree, and retains
capacity on uncertain outcomes until explicit reconciliation. In-file tests
cover identity and launch binding, expiration, CPU scope, argv handling, and
malformed/unbounded observations.

This gate is resource control, not a full sandbox. It does not isolate
filesystem access, network, credentials, namespaces, devices, or descendants
that escape the managed cgroup. It does not treat visible CPUs or
`CARGO_BUILD_JOBS` as a permit.

## Host and permit availability

The current host reports `/proc/self/cgroup` as
`0::/system.slice/ssh.service`. `systemd-run` and `systemctl` exist, and the
cgroup v2 controller list includes `cpuset`, `memory`, and `pids`, but
`systemctl --user show-environment` fails with `Failed to connect to bus: No
medium found`. No writable delegated user cgroup or authenticated resource
permit/issuer receipt is available in the inspected runtime interfaces. No
child was launched and no permit was fabricated.

To enable admission, integration must supply an authenticated configured
issuer result tied to the current reservation and authority generation,
including issuer reference, permit ID, reservation reference, exact project,
task, attempt, node, current lease ID and expiry, host incarnation/boot ID,
registered CPU scope, permitted CPU set, memory maximum, task maximum,
concurrency limit, and exact prepared `LaunchSpec` digest. The host must also
provide an active user systemd manager and a writable delegated cgroup v2
subtree with `cpuset`, `memory`, and `pids` controllers. The central runtime
integration must register `resource_gate` and invoke its crate-private
authenticated issuer constructor; module registration and execution-worker
changes were explicitly outside this task's write scope.

## Verification

Ran `git fetch origin && git rebase origin/main` successfully and
`cargo fmt --all -- --check` plus `git diff --check` successfully. Ran focused
package validation with a lane-private target and the existing private
acceptance temporary directory:

```sh
CARGO_TARGET_DIR="$PWD/target" TMPDIR=/home/ubuntu/.adm-main-acceptance-tmp \
  CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=line-tables-only \
  cargo test -p admiralty-runtime -- --test-threads=2
```

The package suite reported 121 passed and 6 failed. Failures were in unchanged
session daemon tests (`UnexpectedEof: failed to fill whole buffer`) and the
unchanged slow-input test's 10-second timing assertion. An initial attempt with
the lane-local TMPDIR also failed because existing lane ancestors are group
writable and the daemon tests require non-writable ancestors; using the existing
private acceptance TMPDIR resolved those path-admission failures. Ran
`cargo clippy -p admiralty-runtime --tests -- -D warnings` successfully.

Neither package command compiles the new module because the required central
`lib.rs` registration is out of scope. The in-file tests therefore remain
pending central registration; the existing package suite and clippy results do
not establish that this module compiled. No native qualification, deployment,
or merge is claimed.
