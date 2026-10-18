# Durable asset custody and pins

`admiralty-store::custody` stores immutable asset bindings, active pins, retirement claims, and command receipts using a caller-owned SQLite connection. `initialize` creates the module's inline schema. Each mutation uses one `IMMEDIATE` transaction for its compare-and-swap, custody check, and write. Callers must configure a bounded SQLite busy timeout on writer connections.

An asset binding includes project, root, task, asset, owner, full lowercase `sha256:<64 hex>` content identity, and a positive canonical decimal generation. Registration is insert-only with expected revision `0`. Later operations require the exact binding, generation, and current revision. SQLite counters are checked against its signed 64-bit integer range; receipts serialize them as decimal strings.

Every request carries explicit observation time, decision time, freshness bound, and provenance. Missing, stale, foreign, dirty, or held observations are refused. Each successful mutation records the observation and provenance, advances the asset revision, and stores a command receipt. Replaying the same command ID with the same operation and serialized request returns the original receipt. Reusing that ID for different bytes or another operation is a collision.

Pin acquisition and retirement claims serialize on the same SQLite write transaction. Active pins block a retirement claim; a stored claim blocks later pins. Release updates only an active pin whose project/root/task/owner/generation binding matches the caller's request. A denied mutation rolls back without advancing the revision or partially changing a pin/claim. This module records a retirement claim only; it does not inspect or delete files and does not itself grant cleanup authority.

Focused tests exercise reopen/replay, command collisions, both sequential operation orderings, stale and unsafe evidence, compare-and-swap, and concurrent acquisition/claim attempts on separate SQLite connections. This is a storage candidate and does not constitute a cleanup qualification or acceptance grant.
