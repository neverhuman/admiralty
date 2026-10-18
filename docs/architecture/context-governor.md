# Deterministic attention governor

`admiralty_core::governor` is a pure prototype for deciding when a worker,
captain or super captain needs a bounded model turn. It does not run models,
send messages, infer acceptance, authenticate source authority, or establish
agent liveness. It is not active fleet policy until the coordinator wires and
qualifies it.

The caller first durably records and validates each `AttentionUpdate`, including
its producer/incarnation/sequence, project/task/revision, recipient role and
principal, authority reference, and immutable evidence reference. The governor
never opens evidence or receives terminal transcripts. A question carries only
bounded typed facts: its exact question, options, protected constraints and
contract digest. Negations, units and authority are not summarized or rewritten.

Construct `AttentionGovernor::new(GovernorConfig)`, then call
`observe(update, Some(recorded_logical_tick))` and
`take_delivery(&recipient, Some(recorded_logical_tick))`. Logical ticks come from
the authoritative recorded input sequence, not a wall clock consulted inside
the governor. Their unit is deployment policy. Unknown or backwards ticks fail
without consuming attention budget; never substitute zero or reset a window.

Default examples allocate each recipient, independently, the following budget
per 60 logical ticks:

| Role | Ordinary wakes | Critical reserve | Maximum references per wake |
| --- | ---: | ---: | ---: |
| Task worker | 12 | 2 | 16 |
| Captain | 4 | 2 | 16 |
| Super captain | 2 | 2 | 8 |

All values are configurable within validated bounds. They are policy examples,
not measured optimal settings. Delivery bodies also have a byte limit, 8 KiB
by default. This is a serialized reference limit, not an exact provider token
count or a permission to omit essential context. An essential reference or
health lineage that does not fit returns `DeliveryTooLarge` without consuming
budget. Shorten a reference through its authoritative registry or explicitly
increase the budget. Nothing is truncated.

Progress updates coalesce into the latest reference for each recipient and
project/task/revision. They never create a wake by themselves. A later actionable
delivery may include that topic's latest progress if it fits. Healthy and
unchanged health observations also create no model turn. If red health clears
before attention, the obsolete health wake is cancelled while both transitions
remain available as references for later delivery. Pending health history for
one scoped component is attached atomically: a small delivery cannot expose an
old red reference while leaving its later green clear behind. An optional
history that cannot fit remains queued; an essential history that cannot fit
fails explicitly.

Blocked, question, candidate and decision transitions retain separate source
references. Exact duplicate source IDs are idempotent; the same identity with
different content is a collision. Changing authority inside one existing scoped
revision is rejected. A different revision or recipient remains separately
scoped. These checks are consistency gates, not proof that an upstream caller
holds the referenced grant.

Duplicate question attention uses a SHA-256 fingerprint over the exact scope,
recipient, authority and complete protected facts. Stored facts are also
compared exactly before declaring equivalence. Different negations, units,
options, ordering, constraints or contract revisions never receive a heuristic
semantic merge. Equivalent questions retain their separate source references
and point to a canonical source; the governor does not copy answers or resolve
their lifecycles automatically. Each question ID retains its own open lifecycle.
Closing one ID never retires equivalent open IDs; if the canonical question
closes, the earliest remaining open source becomes the canonical reference.
Unmatched closes remain deliverable evidence without erasing another ID.

`take_delivery` returns `Quiet`, `Deferred`, or `Wake`. A wake always includes an
actionable typed reason and compact source/evidence references, not a generated
summary. Critical work may consume the configured reserve after ordinary wakes
are spent. Warning/unknown health cannot consume the critical reserve by itself.
Exhausted budgets defer delivery and retain its references. Reference order is
recorded as a precision-safe decimal string; consumers retrieve authoritative
current projections and evidence rather than treating an old reference as
current task state.

Memory limits are explicit: at most 2,000 scoped topics, 4,096 pending references,
16,384 remembered source identities, 2,000 active exact-question fingerprints and
16 KiB per serialized update. Lower bounds may be configured. Repeated progress
does not grow the pending queue, but source collision history remains bounded.
Reaching a bound returns backpressure without acknowledging the new input.
The upstream durable journal must retain it for retry. Do not evict source
identity history or reset this governor just to bypass a bound; durable
checkpoint/retirement integration must define a safe lifetime before long-running
fleet activation.

Non-actionable retained history, such as equivalent questions arriving after
the canonical wake or already-cleared health, may remain quiet until another
actionable delivery can carry it. It is never silently dropped to make room.
At capacity even a new critical update is rejected with explicit backpressure;
the durable input journal must keep that unacknowledged update. The integrator
must expose this stalled condition mechanically and define durable retirement
and replay before enabling fleet policy. Increasing a model's wake budget does
not authorize erasing queued facts or resetting source-collision history.

The caller must atomically persist accepted input, governor decisions and its
delivery outbox before performing a wake. Replay requires the same configuration,
update order, recorded logical ticks and delivery requests; replaying only input
updates cannot reconstruct previously consumed attention budget. Failed delivery
and transport acknowledgement need the durable outbox's own reconciliation.
The governor's wake result is a proposal and does not claim delivery succeeded.

Inline offline tests cover progress storms, exact question equivalence,
protected-field changes, independent equivalent-question lifecycles, critical
transition preservation, role/recipient budget isolation, reserve exhaustion,
atomic green-health cancellation history, unknown/backwards clocks, identity collisions,
counter overflow, bounded admission, untruncated references and deterministic
replay. They do not establish prospective fleet token savings.
