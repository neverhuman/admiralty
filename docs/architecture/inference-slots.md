# Durable managed inference slots

The optional Store admission path joins a task claim, budget reservation, managed
inference hold and exact launch intent in one sole-writer transaction. It commits
all four or rolls all four back. The ordinary and budget-only APIs remain
compatible, but an exact replay cannot upgrade or downgrade their protections.

`StoreAuthorities.inference_slots` defaults to `None`. Independently installed
opaque policy/proofs authorize pool allotments, model/configuration/claim digests
and exact scope/source/contract/attempt/owner/incarnation/effect bindings. A digest
or caller-supplied capacity key does not authenticate a physical inference pool.
Policy must establish that mapping and the managed allotment independently of
registry aliases or external utilization observations.

`record_inference_slot` accepts only `SetPool`; `Hold` requires
`claim_with_budget_and_slot`. `BudgetAndSlotAdmission` is not deserialized from
JSON. No production issuer, scheduler switch, model endpoint, CLI credential or
runtime execution is installed by these APIs.

Each pool's physical capacity key is immutable and unique in this journal.
Changing a generation preserves all prior holds. Reducing capacity below total
held occupancy fails. Unknown pools and stale-generation new holds fail closed.
There is no release, reset, expiry eviction, settlement or refund API. Stops,
cancellation, elapsed outbox leases and unknown execution retain occupancy and
budget liability. Capacity is an independently admitted managed allotment; this
component does not reconcile other users of the same physical backend.

New commands require exact ledger revision/digest. Exact retries return the
original receipts after current policy checks, including after later commands or
reopen, without a second debit/hold. The writer validates both association
directions on replay, rebuild, backup and protected operations, preserving the
actual original event, budget command and launch intent. A late SQL failure or
denial after staging the claim leaves the in-memory projection unpublished.
Authorized outbox collisions may commit their existing integrity incident alone;
they stage no new claim, debit, hold or launch.

Policy callbacks must be bounded, nonblocking, non-reentrant, perform no I/O and
only observe independently managed authorization. They must not directly or
indirectly mutate any admission authority. The writer samples independent
policies sequentially at explicit validation points. The final slot check sees
the actual staged `Claimed` task, rather than its previous `Ready` projection.
SQL atomicity does not make independent Boolean policies jointly linearizable
with revocation through COMMIT. Production policy needs a common retained
snapshot, epoch or lease fence; no such issuer/fence is activated here. Repeating
or reordering checks cannot replace it. Fault-injection test callbacks demonstrate
rollback at sampled boundaries, not arbitrary asynchronous revocation guarantees.

History is immutable and bounded without eviction: 128 pools, 1024 retained holds,
8192 journal commands and 64 MiB total journal text. Commands, receipts and ledger
have separate byte bounds and canonical digests/revisions; open and recovery
replay the journal and compare the materialized ledger. Checked sums and revision
overflow fail closed.

This is a Store component. It does not activate physical capacity admission,
NativeTaskLoop/ResourceGate/model-selection composition, provider execution,
usage valuation, settlement/release, restart process reconciliation or full
P03/P10/P14 acceptance. No host names or model endpoints are hard-coded.
