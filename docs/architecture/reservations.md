# Pure reservation component

`admiralty_runtime::reservations` is a bounded pure proposal reducer for budget
exposure. It performs no provider calls, accounting ingestion, CPU/resource
release or execution. Persisting its result requires the authoritative writer's
revision compare-and-swap and atomic event/state commit. Concurrent proposals
from one ledger revision are alternatives, not concurrently granted capacity.

Reservation scope binds project, root, task, attempt, owner, incarnation,
contract revision/digest, exact authority reference, account, native unit and
resource-claims digest. Every stop, settlement and correction carries the exact
scope, correlation and expected reservation revision. Accepted mutations update
that reservation's revision; stale or cross-attempt bindings are rejected.
Limits independently pin the unit's code and scale, including the first
reservation. No currencies, credit pools, quota or native token units are
implicitly combined or converted.

Amounts and ledger/reservation revisions use canonical decimal u64 strings.
Scope-keyed amount maps use ordered `[scope, amount]` tuples in JSON and reject
duplicate scopes, so complete projections round-trip without stringifying or
losing their structured identities. Missing/unknown amounts
are not zero and cannot be encoded as numeric settlement. Callers retain the
reservation when measurement is unknown. An explicit zero is a known-zero
assertion requiring the accounting source's custody. Cancellation and lease
expiry only request stop; they never free held exposure.

Admission sums known settled spend and unresolved liabilities at account,
project and root scope. Aggregate maps may include additional conservative
external exposure, but may never understate spend or liability implied by
actual reservation records. An exhausted outstanding reservation blocks new
account admission until its unbounded additional exposure is reconciled.
Observed usage above its original forecast is retained as measured spend,
including over-budget values; it is never truncated or rejected merely to
hide overrun. Final corrections can likewise record truthful totals above the
original forecast, with explicit scoped evidence.

Partial settlement moves measured spend from held liability to settled totals.
Final release additionally requires exact scope/resource-claims digests and
explicit `usage_complete`, `effects_reconciled` and
`resource_claims_reconciled` observations, all `Some(true)`. Missing or false
reconciliation never releases the remaining hold. `ConfirmedNoUsage` requires
both this settlement amount and prior accumulated spend to be zero. These are
caller-supplied evidence claims, not proof that this pure component has verified
a provider receipt or released real CPU/process/account claims. The future
trusted reconciler must authenticate and retrieve the actual evidence.

Events, previous ledger and proposed ledger are limited with the shared
streaming 1 MiB JSON admission utility. Each map retains at most 1,024 entries;
new state is revalidated before returning. Account IDs, event/reservation IDs
and correlations are bounded, units validated after deserialization, references
bounded, scope digests canonical, and arithmetic checked. Exact event replay
is idempotent; changed payload under one ID is a collision. History is never
silently evicted to bypass capacity, and failed proposals leave input unchanged.

Durable accounting, grant authentication, native usage adapters, real resource
claim custody, correction lineage and safe history retirement remain pending.
Reconstruct the ledger from a trusted journal; an arbitrary deserialized ledger
is not an authoritative balance. This component grants neither execution nor
acceptance and does not finish P10.
