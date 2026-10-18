# Native durable controller result

Base: `origin/main` at `f7ae3c1c86b46911051931dcbad0178ca4075d81` (after fetching and rebasing).

## Implemented

- Added `crates/admiralty-server/src/services/controller.rs` with a one-task bounded admission tick. It checks current task/contract state, calls a trusted in-process scoped admission adapter, then records `TaskClaimed` and a typed, idempotent launch outbox intent atomically through the real Store writer.
- Added fail-closed `DenyAdmission` for configurations without a qualified P14/resource adapter.
- Added an I1 lifecycle composition hook using `LifecycleAuthority`, `Store::execute_lifecycle`, and the exact optional `OutboxIntent`. Attempt closure is passed as its own lifecycle action and is not inferred from candidate checks or child exit. Candidate evidence continues to carry its separate closure digest.
- Added tests for actual Store persistence/replay across reopen, denial and stale contract no-write behavior, and I1 writer authorization of exact effect key/payload/digests/schedule with separate closure semantics.
- Did not retain module registration; `services/mod.rs` remains owned by primary integration. It was registered temporarily in this private checkout only while compiling and running the in-file tests.

## Verification

Using lane-local `CARGO_TARGET_DIR=target`, `CARGO_BUILD_JOBS=2`, and `TMPDIR=/home/ubuntu/.adm-native-controller-tmp`:

- `cargo test -p admiralty-server --lib services::controller::tests` — 3 passed, 0 failed.
- `cargo clippy -p admiralty-server --all-targets -- -D warnings` — passed.
- `cargo fmt --all -- --check` — passed with temporary module wiring; wiring was then restored to the committed baseline.
- `git diff --check` — passed.

## Integration boundary and required APIs

This candidate ends at the durable pending intent. Current Store exposes `outbox::claim_due` over a raw SQLite connection, but has no Store-writer request for claiming/completing an intent. I did not open a second production writer or spawn a child without a fenced claim. As a result this branch does not claim to launch, record `TaskStarted`/candidate, run checks, or qualify native execution; the fake-child spine remains unimplemented until those production seams are integrated.

Primary integration needs:

1. Add a writer-serialized Store API to claim one due intent with a fenced token and trusted clock, and to record completed/rejected/unknown outcomes. Expired or ambiguous launch claims must become unknown and must not be automatically retried.
2. Provide the integrated P14 admission/resource-gate adapter that atomically binds task read set, budgets, workspace custody and a live resource permit to the attempt and intent. The controller's `CurrentAdmission` is a trusted in-process seam; it is not a substitute for the transaction.
3. Integrate a runtime launcher adapter that accepts only the live Store claim plus current validated workspace/resource permit. The pushed execution candidate currently exposes `PreparedCommand`, `ExecutionAdmissionValidator`, and `ExecutionAdapter::{launch,finish}`; the server needs its exact module/export and durable-start evidence contract.
4. Integrate the independent trusted-check adapter and bind its opaque execution evidence, exact candidate, custody, and supervised closure into `LifecycleAuthority`. The I1 writer must continue to authorize `CloseAttempt` separately from `DecideCandidate`, including exact effect identity on retries.
5. Register `services::controller` centrally after the primary chooses the final module wiring. No source routes, `lib.rs`, or manifests were changed here.

The implementation is a durable launch-intent admission slice, not an end-to-end production controller qualification.
