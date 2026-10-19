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

The explicit controller `admit_one_slotted` binds the held registry model ID to
the exact declared outbox model and delegates the transaction to Store. This
declared-only entry point does not authenticate a prepared provider mapping.

`BoundNativeCodexPlan::bind_inference_slot` retains the original selected
`ModelSpec`, TaskView and descriptor-backed preparation. It compares the actual
Codex tuple's full-model digest separately from model selection's routing-field
digest. Registry model ID and provider-requested model are different namespaces;
neither becomes an observed model identity. Actual prepared argv/spec, full
kernel claims, original kernel reservation, inference claim, both commands and
planned effect are bound without conflating inference and kernel scope digests.
This Store profile requires the command source incarnation to match its kernel
scope; the Codex tuple's producer incarnation remains independently bound.

The returned private `NativeCodexSlotAdmission` cannot be constructed from JSON
or public hashes. Its pure `matches_plan` comparison freezes the exact request,
launch spec, complete resource claims/reservation, permit-authority Arc and
workspace metadata/head/tree. Live filesystem descriptor/inode custody and
current resource authorization remain the existing execution gates' job. Every
factory rejection returns the original bound preparation, proofs and claim.

`NativeTaskLoop::admit_slotted` returns the original plan and private slot
binding on pre-admission or registration rejection. Once registered, it retains
those capabilities, original commands/proofs, hold, physical key/generation and
receipt revisions through uncertain replies. Admission retry and dispatch use
the slotted controller path, with no budget-only fallback. They require the
exact current authorized hold/budget and unchanged generation/key; unrelated
global-ledger advances do not change the original receipts. The runtime repeats
the same check after the final controller clock sample. Unknown dispatch retains
all custody/occupancy/liability and cannot be reset by a retry.

The optional [paired admission commit fence](admission-fence.md) can retain one
cohort's own rules and epoch through the Store transaction. Its catalog has no
public constructor or authenticated production loader. This is separate from
external credential/physical-pool and dispatch/start fencing.

These are explicit trusted-local composition APIs. They install no startup
issuer, live registry/pool authentication, common policy fence, physical-start
linearization or default scheduler/provider route. Provider completion, usage
valuation, settlement/release, restart process reconciliation and full
P03/P10/P14 acceptance remain separate. No host names or model endpoints are
hard-coded.
