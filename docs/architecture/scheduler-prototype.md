# Scheduler prototype

`admiralty_core::scheduler::propose_ready_tasks` is a pure prototype over a
caller supplied `SchedulerSnapshot`. It validates a bounded snapshot (at most
1000 tasks), checks each task contract and its digest, verifies project/root
scope and observed revision, and rejects duplicate, missing, self, or cyclic
dependencies. Its iterative graph walk is bounded by the task cap.

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

This prototype does not acquire claims, reserve resources, start processes, or
make acceptance decisions. Callers must independently establish snapshot and
capacity freshness, admission policy, authority generation, ownership,
provider/runner capability, and evidence availability before any effect. A
proposal is not proof that a process can safely start.
