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

`NativeTaskLoop::admit_reserved` explicitly composes this path with actual
prepared-plan custody. `NativeBudgetAdmission` supplies the immutable command
and separate opaque process-local write and read proofs; the ordinary `admit` compatibility path remains
separate. Before registration, the loop checks the command's source/host
incarnation, scope, amount, correlation, reservation identity and exact launch
digest against the actual prepared plan, then samples the separate read capability
before registration. A mismatch or denied read returns that preparation.
It never silently rewrites producer identity or substitutes a proof.

The resource plan's reservation version is distinct from the durable budget
journal's version. A new durable reserve uses the frozen ledger CAS successor;
the loop checks that against the actual Store receipt and current authorized
ledger. Required account-limit setup may already have advanced that global
revision. The resulting reservation's stable fields must still match the plan.
Unrelated ledger updates do not change that reservation; stopping or changing it
prevents launch. Read and write authority are both required, default absent.

Reserved retries keep the original command, both proof roles and preparation and recheck
current writer authority rather than reuse cached authorization. Dispatch
reauthorizes the exact already-claimed replay, requires the original receipt and
no additional debit, and checks the current reservation. The concrete runtime
wrapper repeats this check after the controller's final clock read, before
calling the executor. Unknown/held dispatch is never reset to queued and does
not refund liability. These checks do not establish atomic physical launch with
Store revocation: production permit/fencing composition remains required.

Read authority is sampled observationally; it is not a lease through admission
or physical start. A write proof is never retried as a read proof. Expired or
revoked read capability after admission holds dispatch while preserving the
original reservation, intent, preparation and unknown liabilities. This matters
for the paired cohort, whose private read/write roles intentionally differ.

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

The writer samples authorization before writes and at final precommit validation
points, including incident-only commits. Independent policies are sampled
sequentially. Their callbacks must be observational and must not mutate or revoke
any admission authority. These Boolean interfaces do not supply joint
authorization or asynchronous revocation linearizability through COMMIT;
production needs a common retained snapshot, epoch or lease fence. An outbox
collision preserves its incident only after those validation points and stages
no reservation. Subsequent journal, policy or
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
