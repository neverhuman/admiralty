# Native durable controller and executor result

Base: `origin/codex/native-runtime-integration` at `64c75fe74246f0db0f0981f25448ca81ad2161ec` (PR #70 source).

## Implemented

- The source branch provides bounded admission and Store-writer outbox dispatch. `dispatch_one` checks the canonical payload digest, strict schema, current task/attempt/owner/contract projection, and current admission before calling the process-local native executor. Candidate receipts do not advance task acceptance.
- Added `crates/admiralty-server/src/services/native_executor.rs`, a concrete adapter over `ResourceGate`, `SessionHost`, `VerifiedResourcePermit::verify_current`, and `RunningAdmission::prove_closed`.
- Plans are process-local trusted startup objects. Before launch, the adapter validates task, attempt, owner, contract digest/revision, context/policy, configured provider/model, run ID, reservation, live workspace custody, and opaque pinned `PreparedWorkspace` evidence.
- It writes a create-only mode-0600 recovery identity and fsyncs the containing directory before the resource-gated launch. It polls `SessionHost::status` without blocking, captures only complete bounded output after observed exit, and requires the gate's opaque closure proof before producing a receipt. The evidence journal binds the proof and output digest; declared provider/model are stored separately from explicitly absent observations.
- Uncertain launches remain in a bounded process-local registry holding `RunningAdmission`; no code path relaunches them. Unknown outcomes remain unknown. A successful child yields only a candidate disposition, never acceptance.
- Added a real pinned-workspace fixture test proving an unconfigured resource backend denies before spawning a deterministic child and leaves both the child marker and recovery journal absent.
- `services/mod.rs` was changed temporarily for local verification, then restored; primary integration owns activation and startup wiring.

## Verification

Using lane-local `CARGO_TARGET_DIR=target`, `CARGO_BUILD_JOBS=2`, and `TMPDIR=/home/ubuntu/.adm-native-controller-tmp` (tightened to mode 0700 for private-config tests):

- `cargo test -p admiralty-server services::native_executor::tests -- --nocapture` — 1 passed, 0 failed, 1 ignored helper.
- `cargo test -p admiralty-server -- --test-threads=1` — 24 passed, 0 failed, 1 ignored helper; includes existing access/startup integration tests.
- `cargo clippy -p admiralty-server --all-targets -- -D warnings` — passed.
- One parallel full-suite attempt had a transient model-registry `busy` failure; the serial rerun passed.

## Integration and deployment status

PR #70 is still open at `64c75fe74246f0db0f0981f25448ca81ad2161ec`. No live service was stopped or replaced. The native executor file is not registered in `services/mod.rs`; primary integration must wire it into trusted startup, supply configured `ResourceGate`, current permit authority, real prepared task plans and durable source sequencing. This lane does not record `TaskStarted` or `CandidateSubmitted`; those events require the primary's trusted Store/lifecycle decision composition.

The only executor test is intentionally fail-closed because this environment has no qualified configured Linux systemd resource backend. A real successful native child remains gated on the P10 Linux resource qualification and primary startup composition. The handoff's next deployment instructions apply only after PR #70 review and merge; no production deploy was attempted.
