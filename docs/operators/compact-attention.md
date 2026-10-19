# Durable compact attention

The Store records the existing attention governor's observations and delivery
decisions on its sole writer. `register_attention` installs an immutable
project/root/epoch and sorted recipient set. `apply_attention` requires its exact
configuration digest, command identity, prior cursor and explicit logical tick.
`attention_status` exposes the matching journal state.

Replay includes observations and quiet/deferred delivery decisions that consume
clock or budget state. Exact historical retries return the original immutable
receipt before the current-state compare-and-swap check. Only the outer
`replayed` flag changes. Changed content under a command ID is a collision.

A new wake and its new outbox intent commit in one SQLite transaction. A
conflicting or foreign existing effect rolls back the decision and budget.
Historical retries preserve actual pending/claimed/unknown/completed states and
never enqueue replacements. Open/rebuild/backup validate the bounded history.

Limits:64journals,4096commands and8MiB of accepted command+receipt JSON globally.
Capacity returns backpressure; history/configuration/epochs cannot be reset or
evicted to regain budget. Complete wake envelopes retain the existing16KiB
outbox bound. These are not disk/WAL/performance guarantees.

This trusted in-process seam grants no source/delivery authority. Services must
independently check current authentication, permission, project/root/recipient
scope and clock. Recording a wake establishes no provider input, Screen message,
delivery or application ACK. Budgets are per journal/recipient; global fleet
limits, safe-turn transport and restart reconciliation remain separate work.
P05/P06 and general autonomous execution remain open.
