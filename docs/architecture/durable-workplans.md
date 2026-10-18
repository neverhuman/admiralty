# Durable workplan revision registry

The offline registry records immutable JSON workplan revisions through the
existing Store writer. It preserves source bytes, canonical decoded contracts,
dependency order, task identities and command receipts. Registration does not
emit `TaskSubmitted` or any other domain event, move an event/watch cursor,
launch a task, run a check, qualify a provider or grant acceptance.

## Offline CLI

Preview remains backward compatible:

```sh
admiralty workplan --file /absolute/path/plan.json
admiralty workplan preview --file /absolute/path/plan.json
```

Register and inspect a registry in an owned, closed database:

```sh
admiralty workplan register \
  --database /absolute/private/admiralty.sqlite \
  --file /absolute/path/plan.json \
  --expected-revision 0

admiralty workplan show \
  --database /absolute/private/admiralty.sqlite \
  --project project-id --plan plan-id

admiralty workplan show \
  --database /absolute/private/admiralty.sqlite \
  --project project-id --plan plan-id --revision 1

admiralty workplan list \
  --database /absolute/private/admiralty.sqlite \
  --project project-id --limit 25
```

For the next file revision, supply the exact current revision using
`--expected-revision 1`, then `2`, and so on. Fresh revisions start at one and
advance by exactly one. `--command-id` is optional: the CLI otherwise derives a
stable identifier from the exact input/scope, expected revision and source and
canonical digests. An identical retry returns its historical receipt. Changing
source bytes, scope, expected revision or canonical content under a supplied
command ID is a collision. Whitespace changes source identity even when decoded
contracts are otherwise identical.

List uses an exclusive plan-ID keyset cursor: pass the output `next_after` as
`--after`. Entries are ordered by plan ID and confined to the named project.
`--limit` is 1–100. `show` without `--revision` returns current; a missing scope or
revision returns no record. A receipt describes its own recorded revision, not
necessarily the current revision.

These commands use the Store's single writer and refuse an already-owned live
database. Do not stop a service merely to use this offline interface or open a
side SQLite connection. Use a separately owned offline database while a future
authenticated live registry interface is pending. Workplan inputs use bounded
regular-file reads; previews and registration accept typed JSON only. TOML,
Markdown import, import UI and automatic task admission are not implemented by
this packet.

## Core preflight and preserved identity

The registry calls `admiralty_core::workplans::preview_workplan` and
`preview_workplan_with_existing`. It does not implement another DAG parser.
Core validation supplies supported schema/version checks, exact project binding,
duplicate IDs, immutable contract fingerprints, missing dependency/cycle checks,
and deterministic dependency-first order. The original `Workplan.tasks` order
and all root IDs, negations, paths, criteria, units, check argv and constraints
remain unchanged in the retained canonical document. The registry adds no
cross-root restrictions beyond core validation.

Plan identity is `(project_id, plan_id)`. Task IDs retain the existing core/domain
model's global identity: any different contract content, revision, root or project
under a retained `TaskId` is refused. Removing a task from a newer plan does not
forget that identity. An unchanged contract may appear in another revision or
plan; its full canonical contract must still match.

The Store writer passes authoritative existing domain-task digests to
`register_with_existing`, preventing a plan import from rebinding an already
submitted task. The reciprocal `validate_task_contract_in_transaction` guard is
available to the existing append transaction: a later `TaskSubmitted` cannot
rebind an ID first retained by the registry. It performs bounded read validation
without opening, committing or replacing the caller's transaction. The
autocommit `validate_task_contract` wrapper is also available. A missing retained
identity permits the caller's normal domain validation; the guard itself grants
no submission authority.

Startup and projection rebuild use the read-only `validate_task_contracts` batch
guard. It accepts references to the finite resident projection, bounds retained
registry metadata, ignores unrelated task IDs, deduplicates matched contracts and
validates each distinct anchor plan once. A task-anchor index avoids scanning all
revision task rows for each identity. An empty registry does not consume the
projection iterator, and a large unrelated projection does not acquire the
registry's 20,000-task limit. Rebuild validates these bindings before changing
the recovery generation or replacing the in-memory projection. These checks
remove repeated history validation; they are not full performance qualification.

## Atomic storage and replay

`RegisterWorkplanRequest` supplies command, project, plan, expected current
revision, raw source bytes and both expected digests. The source digest binds the
exact original bytes; the canonical digest binds the complete parsed Workplan.
The registry validates both. Command identity binds full scope, expected revision
and both digests without serializing the source as a JSON byte array.

One `IMMEDIATE` transaction records the immutable revision/source/receipt,
retained contracts, exact ordered task bindings and current pointer. Foreign keys
bind the pointer to the revision, revisions to their plan and task references to
their retained contract digests. New registrations require the exact expected
current revision. Concurrent attempts from separate SQLite connections serialize;
one CAS can commit and the other fails. Any late validation or write failure
rolls back the pointer, revision, new task identities and receipt together.

Reads and command replay verify the current pointer and contiguous retained
history, then re-run core source validation. Stored source/canonical digests,
canonical JSON, command identity, cached receipt, task digest/order rows and
retained contract bytes must agree. Cached receipts are compared to reconstructed
canonical output, including their sealed flags; unknown/forged fields or changed
bytes cannot grant authority. Historical replay returns the original receipt
without updating the current pointer or discarding later revisions. A corrupt
historical receipt also prevents a current plan read or fresh registration from
silently extending that history.

Every receipt contains exact project/plan/revision scope, source/canonical
digests, task identities and dependency order. Receipts are Serialize-only public
output with private fixed flags:

```json
{
  "recorded_only": true,
  "native_execution_qualified": false,
  "task_events_emitted": false,
  "execution_granted": false,
  "acceptance_granted": false
}
```

## Bounds and connection assumptions

Input source and canonical plan documents are capped at 1 MiB; core caps each
plan at 1,000 tasks. Streaming typed admission precedes canonical serialization.
Stored text is checked as borrowed SQLite bytes before allocating Rust strings.
Receipt JSON is capped at 512 KiB. Other opaque IDs are core-validated bounded
ASCII identifiers; revision counters use canonical decimal strings and core
`u32` revisions.

Prototype retention limits are 256 plans, 64 revisions per plan, 20,000 retained
task IDs, and list pages of at most 100 summaries. Retained revision text
(source, canonical and receipts) has a 64 MiB logical payload budget; retained
contract JSON has a separate 64 MiB budget. SQLite indexes, page overhead and WAL
are additional storage. No limit silently evicts revisions, contracts or command
IDs. Historical replay remains possible at capacity. A policy/limit change needs
an explicit schema/implementation review.

`initialize` is explicit and runs after the owning Store's other migrations in
autocommit. It enables foreign keys, verifies exact component-owned `STRICT`
tables and foreign-key closure, and leaves global `PRAGMA user_version` and other
components untouched. Each connection must retain foreign keys and a bounded
busy timeout of 0–5,000 ms; zero means immediate busy failure. Assumptions are
checked before opening a writer transaction. Caller-owned transactions are
refused by register/get/list, while the explicit reciprocal transaction guard
joins the already-owned append transaction.

All table references target `main`; temporary table shadows cannot redirect a
registry write. Unexpected triggers or older/partial component schemas fail
closed. Raw SQLite access, hostile extensions/VFS, a hostile filesystem or an
adversary able to rewrite all evidence consistently are outside this integrity
boundary. The caller must preserve database custody and use the owned writer.

## Validation and activation boundary

SQLite tests cover reopen and historical replay, exact source/command collisions,
scope and revision CAS, lifetime task identity after removal, exact mixed-root
contracts, reciprocal domain guards inside an append transaction, late-failure
rollback, cached receipt/source/order/pointer tampering, project-scoped paging,
bounded history and a real two-connection CAS race. Batch tests verify one actual
history validation for many contracts sharing an anchor, ignore more than 20,000
unrelated references, and reject conflicting/corrupt rebuild bindings without
mutation. They execute no task checks, providers or production services.

This packet supplies durable offline registration only. Live workplan APIs,
authoring/import UI, guided legacy format import, dispatch, task admission and
trusted verification remain separate integrations.
