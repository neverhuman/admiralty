# Peer interface contracts

This core component records immutable interface revisions and provisional compatibility evidence. An interface is scoped to one project, root and producer task, and binds its authority reference, producer and exact consumer principals. Artifact and schema identities use `sha256:<64 hex digits>`. Revisions, state revisions and epochs travel as explicit recorded values; `DecimalU64` serializes counters as canonical decimal strings.

A caller validates the authority grant and recipient visibility before creating a `Publish` event. The event records the validated authority owner and the exact recipient epoch for every consumer. The reducer then permits publication by the producer or that owner only. ACK and rejection events must name the exact consumer, authority reference, recipient epoch, interface revision, artifact digest, schema digest and expected state revision. A stale value is an error. Rejection evidence is retained as a reason. An ACK is provisional compatibility evidence for this interface revision; it does not accept a task, candidate or delivery stage and grants no execution or publication authority.

At a request boundary use `reduce_interface_for` with the independently
authenticated actor, resolved authority owner and exact interface scope that
the actor's grant permits. The scope includes project, root, producer task and
authority reference. It must not be copied from the same worker request.
Matching actor/owner strings inside an event do not authenticate that worker or
authorize access to a foreign contract. The lower-level `reduce_interface`
remains a pure transition over already-admitted events, suitable for replay;
it cannot establish principal identity or grant validity on its own.

A higher interface revision supersedes the current one. The reducer clears prior compatibility evidence and returns a sorted invalidation proposal for each consumer with a provisional ACK, binding the old revision and digests to the new revision. It does not rewrite prior ACKs as current. The caller persists those proposals atomically with the event and delivers them through its authorized coordination path. Replaying an identical event ID and payload returns no proposals; reusing an event ID with changed bytes fails. Events and consumers are bounded, outputs use ordered maps, and the reducer performs no I/O, clock reads, tool invocation or external effect.

Workers can exchange the immutable schema/artifact reference directly: the producer publishes to its named consumers, each consumer retrieves the specific artifact through its authorized evidence path, and returns an ACK or structured rejection. A captain need not relay this exchange. The owning authority is involved only when the contract or ownership is disputed. This module supplies the deterministic state transition; the integration owner wires caller-side grant validation, durable event storage, proposal delivery, narrow `interface_get`/publication operations and any API, agent tool or CLI registration. Those integration surfaces remain intentionally outside this component.

`decode_event` bounds JSON to 1 MiB before strict typed decoding; direct typed
events receive the same streaming byte check. Unknown and duplicate JSON fields
fail closed. Context strings exclude controls and are at most 4096 bytes;
consumer/event collections are bounded to 1024 and trusted state to 8 MiB before
cloning. Callers must also bound file/body reads before supplying a byte slice.
State revisions use checked increments, and errors leave caller state intact.
The counters and epochs are recorded values, with no hidden clock reads or
inferred expiry. Publication, invalidation persistence and delivery are still
unwired: no native worker exchange, authenticated transport, atomic outbox
commit or fleet execution is qualified by these unit transitions.
