# Native durable controller result

Base: `origin/codex/native-runtime-integration` at `aab2a6af65b70a5099143777ca9b602e0ce2a80d`, rebased on top of source commits `6ba83c5`, `0f17b9f`, and `8ee2f6b`.

## Implemented

- Added `crates/admiralty-server/src/services/controller.rs` with bounded admission that checks the current task/contract state, invokes trusted in-process admission, and atomically persists `TaskClaimed` plus an idempotent launch intent through the Store writer.
- Added `dispatch_one`, which claims at most one due intent through `Store::claim_outbox_due`, verifies the canonical payload digest and strict schema, rechecks the exact task/attempt/owner/contract against the current projection, and invokes a process-local current-admission check before runtime dispatch.
- Dispatch consumes a typed runtime receipt bound to task, attempt, and run. Candidate evidence and failure evidence complete only the outbox effect; they never change task acceptance or lifecycle state. Denied, malformed, mismatched, and uncertain outcomes are written as `unknown`, which the Store will not automatically redispatch.
- Added tests using the actual Store writer for a candidate receipt, no implicit task acceptance, no redelivery after completion, and durable fail-closed denial/unknown behavior. The runtime in these tests is a deterministic fake; it verifies the dispatch protocol, not real provider execution.
- Kept `services/mod.rs` unchanged because primary integration owns module registration. It was temporarily registered only in this private worktree to run tests and clippy.
- Preserved the I1 lifecycle composition hook: exact authorized effects are passed via `Store::execute_lifecycle`, and attempt closure remains a separate action.

## Verification

Using lane-local `CARGO_TARGET_DIR=target`, `CARGO_BUILD_JOBS=2`, and `TMPDIR=/home/ubuntu/.adm-native-controller-tmp`:

- `cargo test -p admiralty-server controller::tests -- --nocapture` — 5 passed, 0 failed.
- `cargo clippy -p admiralty-server --all-targets -- -D warnings` — passed.
- `rustfmt --edition 2024 crates/admiralty-server/src/services/controller.rs` — passed.
- `git diff --check` — passed.

## Remaining production integration boundary

This advances actual durable native dispatch, but the checkout still has no concrete server-side `NativeRuntimeExecutor`/`DispatchAdmission` composition. Runtime `ExecutionAdapter` requires a `PreparedCommand`, validated workspace, reservation, and live resource permit. `VerifiedResourcePermit::from_authenticated_issuer` is crate-private to `admiralty-runtime`; the server cannot construct the permit from an authenticated authority with current public APIs. In addition, `ExecutionAdapter::finish` currently uses blocking `wait`, so it is not a safe bounded dispatch-loop implementation for arbitrary provider runs.

Required follow-up: export a trusted runtime factory/issuer path for `VerifiedResourcePermit`, compose workspace preparation + reservation + trusted admission into a concrete server adapter, and change runtime finish to bounded observation/deadline semantics before wiring a real provider child. Until then this controller safely dispatches only when the server supplies those trusted adapters; this patch does not claim an end-to-end real-provider execution qualification. Primary integration still needs to register `services::controller` centrally.
