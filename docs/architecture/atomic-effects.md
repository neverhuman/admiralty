# Atomic event and effect-intent storage

`Store::append_with_intent` records a domain event, one canonical outbox intent, and their immutable association in a single SQLite `IMMEDIATE` transaction. The event reducer runs before commit; the in-memory projection and watch notification advance only after commit. A failed insert or association constraint rolls back both rows. The method returns a durable append receipt, not an effect execution or acceptance receipt.

The event is serialized through a bounded writer capped at 1 MiB. The outbox payload is recursively canonicalized by sorted object keys and written through a 16 KiB capped writer; structural depth and node limits are checked before serialization. Keys are capped at 256 printable bytes, timestamps at 128 printable bytes, and `u64` sequence/deadline values are checked against SQLite's signed integer range. The association snapshots the event sequence, unique outbox row, idempotency key, payload digest, deadline, and a digest of that intent identity. Foreign keys and immutable update/delete triggers prevent reassignment.

Exact replay requires the same serialized event, observed timestamp, and effect association. A plain replay of an event that has an effect, an effect replay of a plain event, or any changed key, payload, or `not_before` schedule records an integrity incident and returns a source collision. An exact pre-existing outbox key cannot be attached to a second event, including when the original row was created through the legacy `outbox::enqueue` API. That collision is recorded and no second association is made. The public `Store::append` and `outbox::enqueue` APIs remain available with their prior responsibilities.

Migration `0003_atomic_effects.sql` is additive and idempotent. `Writer::open` explicitly applies it after foundation and outbox initialization. It intentionally leaves `PRAGMA user_version` at 1: that pragma remains under integration-owner control, and the current supported executable accepts versions through 1. A future coordinated global migration may advance it. Opening and rebuilding a store only initialize schema and replay domain events; they do not claim, deliver, or execute outbox work.

## Boundary

The outbox remains a durable proposal queue. This component does not launch providers, call a forge, perform network I/O, or establish publication authority. Claims, external execution, reconciliation, and acceptance are separate responsibilities.
