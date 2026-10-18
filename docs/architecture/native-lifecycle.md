# Authoritative native task lifecycle

This component is the I1 durable decision seam. It does not activate managed
execution, authenticate a verifier by name, execute checks, publish a branch or
qualify the fleet. `native_execution_qualified` remains false.

`LifecycleCommand` uses protocol version2 and a SHA-256 CAS over the complete
current `TaskView`. That binds the contract, state, owner, attempt, candidate and
retained attempt history. Clients must use the exact current snapshot identity;
the trusted service constructs the command rather than guessing a revision.

`Store::open` denies native commands. The trusted service can install one
`LifecycleAuthority` at startup with `open_with_lifecycle_authority`. The writer
calls this adapter before source replay disclosure, CAS or mutation. It must
check current actor/project/root/action/source grants and revocation, actual
independent verifier execution and custody, supervised child/effect closure, and
the exact optional effect key, canonical payload, digests and schedule. A digest,
named verifier, passing provider output or deserialized boolean is insufficient.
The callback must be bounded and nonblocking; it runs on the single writer.
Authentication and runtime evidence adapters remain separate integration work.

`execute_lifecycle` authorizes the complete command/effect and then uses the
existing event/request/outbox transaction. An authorized exact source retry
returns the original receipt even though the task CAS has advanced. Changed
content collides. Both public raw append paths and the writer's raw dispatch
reject protected lifecycle events. Replay checks journal integrity and reapplies
the pure reducer; it does not execute effects or reauthorize historical facts.

Supported transitions:

- Decide a current candidate as accepted or failed, bound to the exact attempt,
  candidate, contract digest, pinned verification policy, check receipt, custody,
  source commit/tree and independent verifier. A separate supervisor closure
  digest is required; a check receipt does not establish process closure.
- Close a current claimed/running/awaiting attempt as failed or cancelled after
  the configured supervisor has established closure. Unknown outcomes stay in
  the existing active state and cannot authorize retry.
- Retry the exact closed attempt with its closure identity and remaining
  contract attempt budget. It clears current ownership/candidate only here and
  retains all admitted attempt IDs. Reusing an attempt ID is rejected.
- Cancel pending ready/blocked work that has no live attempt.
- Revise an unstarted, unreferenced task by exactly one contract revision,
  preserving project/root/task/authority/simulation identity. Tasks with any
  retained task-bound coordination cannot be revised, because old assignment
  envelopes need an explicit invalidation/epoch protocol. Registered workplan
  task identities remain subject to the existing immutable reciprocal guard;
  general workplan revision/adoption is not implemented by this command.

Acceptance atomically releases same-project blocked dependents only when every
dependency is accepted. Initial submission after accepted prerequisites also
starts ready. Accepted/referenced contracts cannot be revised, so unpinned v1
dependency identities cannot silently change meaning. Rejected, failed or
cancelled dependencies never count as accepted. Past contracts and closure
decisions remain in the event journal; retry/revision does not reset budgets.

## Protocol compatibility

The original TaskContract, CoordinationMessage and six-tag domain-event v1
schemas remain unchanged. `DomainEvent` is the internal journal decoder and now
recognizes an explicit `task_lifecycle` branch carrying protocol version2. That
branch is deliberately excluded from the frozen v1 schema. A separate
`domain-event-v2.schema.json` and standalone TypeScript guard describe the v2
union. `DomainEventV2` is a schema-only Rust type without a competing serde codec.

Unchanged legacy task views omit lifecycle metadata and preserve their serialized
shape. A snapshot containing native metadata advertises version2. `/v1/fleet`
and newly opened `/v1/events` return426 for that state; an already open v1 stream
emits `upgrade_required` and ends before exposing a native event. Authenticated
`/v2/fleet` and `/v2/events` carry the new state. The detailed legacy dashboard
bridge remains independent. The React v2 reader/terminal-state UX must land before
native activation; the old v1 client must not be fed a misleading version1 body.

## Verification and next integration

Focused Store tests cover default denial, raw-append rejection, stale bindings,
revocation before duplicate disclosure, source collisions, racing decisions,
budget/attempt reuse, stale assignments, dependency release, backup/reopen/replay,
and optional-effect authorization with late-SQL rollback of all transaction halves.
HTTP tests cover v1 upgrade during an open stream, authenticated v2 reads and
continued truthful qualification. Contract tests require frozen v1 rejection and
v2-only protocol acceptance. Test authority predicates are explicit doubles;
they do not establish actual provider/verifier/fleet qualification.

The next integration connects opaque runtime execution/verification/closure
evidence to the configured authority, binds admitted resource/workspace custody,
and exercises one real isolated task before canary activation and production
cutover. No generic `allow_all` authority belongs in the service.
