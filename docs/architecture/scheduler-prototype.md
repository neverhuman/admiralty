# Scheduler prototype

`admiralty_core::scheduler::propose_ready_tasks` is a pure prototype over a
caller supplied `SchedulerSnapshot`. It validates a bounded snapshot (at most
1000 tasks and 1 MiB of serialized typed input), checks each task contract and its digest, verifies project/root
scope and observed revision, and rejects duplicate, missing, self, or cyclic
dependencies. Its iterative graph walk is bounded by the task cap.
`parse_scheduler_snapshot` bounds source JSON to 1 MiB before decoding and
rejects duplicate or unknown fields, including unknown fields inside a held
lease. Callers reading a file or HTTP body must bound that read before creating
the byte slice. Direct typed callers receive the aggregate serialized size
check too; large nested contracts cannot bypass the parser's resource bound.

The proposal lists pending tasks whose dependencies are explicitly completed
and whose lease is explicitly absent. Active, blocked, and completed tasks are
never proposed. Held leases block even when their expiry tick has passed;
reconciliation is required before any later authority can decide what to do.
Unknown or zero quota/capacity is reported as a blocker. Otherwise the lexical
TaskId order is admitted up to the lower known quota/capacity limit, and
deferred tasks carry the limiting reason. The source digest binds the serialized
snapshot, including task delivery order; ready ordering is independent of that
delivery order. The input is not mutated and `execution_granted` is always
false.

Quota and capacity are remaining **task slots** supplied by the caller, after
existing reservations. Neither is a token estimate, CPU measurement, provider
capability, nor resource permit. Unknown capacity never falls back to a guessed
number, and one snapshot can produce the same proposal repeatedly. The module
does not deduplicate dispatch or reserve any of those slots.

There is deliberately no clock read or deadline/reclaim calculation. Lease tick
zero, a past tick, an unknown tick and a far-future tick all remain held until
the authoritative source reports an absent lease. A future reconciler must
use an explicit recorded clock and generation; elapsed wall time alone cannot
make an uncertain effect safe to repeat.

`Completed` is a caller-supplied observation. An adapter must derive it only
from the canonical accepted task state, never from a worker's completion claim,
candidate receipt, successful process exit or model reply. A digest proves
content identity, not source trust or acceptance. This module cannot establish
the truth of an arbitrary snapshot and does not emit acceptance events.

This prototype does not acquire claims, reserve resources, start processes, or
make acceptance decisions. Callers must independently establish snapshot and
capacity freshness, admission policy, authority generation, ownership,
provider/runner capability, and evidence availability before any effect. A
proposal is not proof that a process can safely start.

The worker-runtime integration remains unimplemented. Before enabling native
execution, the controller must build authenticated snapshots from the store,
bind their recovery generation and exact contract identities, atomically check
current claims and resource permits, enforce private configuration and
workspace custody at the effect boundary, and record a dispatch intent with
uncertain-effect reconciliation. Verifier/acceptance authority remains a
separate transition. Importing this pure component does not activate a
scheduler loop or qualify native fleet execution; `execution_granted: false`
is descriptive output, not a security capability that consumers may toggle.
