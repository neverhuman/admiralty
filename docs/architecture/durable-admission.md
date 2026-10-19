# Durable budget admission

The native Store now persists the existing Rust reservation reducer's commands,
receipts and checked ledger. `StoreAuthorities.reservations` installs a current
process-local policy; its default is `None`. Opaque proofs are not serialized or
constructed from HTTP JSON. Policies must be bounded, nonblocking and
non-reentrant, with no I/O or nested Store calls.

`Store::claim_with_reservation` commits the exact task claim, immutable launch
intent, budget reservation and their associations in one writer transaction.
The controller's explicit `admit_one_reserved` path composes that operation.
`planned_launch_intent` and `outbox::intent_digest` compute the same bounded
canonical effect identity for a separately authenticated admission proof; they
grant no authority. The command binds actual Store task/project/root/contract,
attempt/owner/source, account/unit/resource claims and the launch intent digest.
The installed policy remains responsible for current budget/account/resource
authority. A public digest is not a permit.

New commands require the exact ledger revision and digest. Competing commands
cannot both spend the same snapshot. Limits and exposure must be known in the
configured account/project/root dimensions; missing exposure fails closed. Amounts
use the existing checked minor-unit reducer and canonical decimal strings.

Exact retries recheck current authority, return the original receipt and make no
second debit. Original event/intent/journal identities are checked in both
directions. Ordinary claims cannot be upgraded by retrying with a reservation;
standalone reservations cannot be attached later. Protected claims cannot be
retried through the ordinary append path. A historical attempt may match actual
Store lifecycle lineage only for exact immutable replay, with current policy
still required. Contract changes can revoke that scope.

The writer authenticates before writes and immediately before commit, including
incident-only commits. An outbox collision preserves its incident only when
currently authorized and stages no reservation. Subsequent journal, policy or
SQL failures roll back the new outbox, debit, claim and associations together.
In-memory task projections are published only after commit.

The journal is bounded without eviction; SQL text is checked before allocation.
Startup, reads, rebuild and backup validate canonical receipts and replay against
the ledger. Immutable history triggers and reverse association checks make
missing bindings fail closed. Opening or rebuilding a database does not reset
exposure or install grants.

The initial durable command surface supports `SetLimits`, `Reserve` and
`RequestStop`. Stop, cancellation and expiry retain outstanding liabilities.
No JSON-controlled settlement, refund or final release is exposed. Native usage
observations remain reported counters, separate from trusted settlement.

This is a durable admission component, not production activation. No default
budget issuer, fleet-wide inference-slot allocator, real provider qualification,
usage valuation/settlement or restart reconciliation is installed. The original
unreserved controller path remains a compatibility/testing seam and is not a
qualified production scheduler. Full P03/P10/P14 acceptance requires the remaining
policy/resource/provider/recovery composition and real evidence.

Focused checks use actual registered targets:

```sh
cargo test -p admiralty-store --test admission_atomic_claim --test admission_reservations -- --test-threads=1
cargo test -p admiralty-store --lib admission_reservations -- --test-threads=1
cargo test -p admiralty-server --lib services::controller::tests -- --test-threads=1
```

The historical lineage helper test is not a complete two-attempt execution
journey. No provider is spawned by these checks.
