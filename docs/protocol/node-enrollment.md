# Node enrollment bookkeeping

This document describes the pure `admiralty-core` enrollment proposal model. It records scoped enrollment tokens, node identity claims, generation changes, revocation, and timestamped capacity observations. It does not enroll a live machine or prove that a claimant possesses a key.

## Pure reducer

`NodeId` is a locally validated string identifier. `NodeScope` binds an enrollment to one project, root, task, and authority principal. An `EnrollmentToken` stores a SHA-256 digest of its one-time secret, never the plaintext secret. Enrollment requires an explicit known clock strictly before the expiry tick and an exact scope match. The first binding consumes the token. Repeating the same event is idempotent; replaying the token with a changed node, boot ID, key digest, or scope fails.

Node identity carries a boot ID, public-key digest, capability digest, and canonical decimal-string generation. Rotation and revocation require the current generation and advance it with checked arithmetic. Capacity observations bind to the exact node, boot, and generation, and require a known timestamp and current time within the declared freshness interval. Unknown time, stale boot, revoked identity, stale generation, malformed digest, noncanonical counter, overflow, unknown JSON fields, and oversized input fail closed. No missing observation is interpreted as free capacity.

The registry retains each node's exact enrollment scope, including after
revocation. At a request boundary `reduce_enrollment_for` requires that scope
and an independently authenticated authority principal; callers must resolve
them from trusted grants rather than copy them from a request. The primitive
`reduce_enrollment` is deterministic bookkeeping for already-admitted records,
not an authentication service. A client-supplied key/token digest is not proof
of possession of either secret.

Every new event requires a known recorded clock, including token issue,
rotation and revocation. Tick zero is valid. `last_tick` prevents later new
events from rolling time back; the event receipt hashes both event content and
its recorded clock. Reusing an event ID with a changed clock is a collision.
Exact replay of an older admitted event returns the current state without
rolling it back. The persistence layer must retain the original event clock;
it cannot substitute the current wall clock during replay.

Newly issued tokens cannot be pre-consumed or already expired. Initial node
generation is one. A revoked node needs a fresh unused token, its retained
scope and the exact next generation to return; the old consumed token cannot
resurrect it. A renewed node can subsequently report capacity under its new
generation. Capacity timestamps may not move backward, and changed evidence
at the same timestamp is an identity collision. Freshness uses the explicit
inclusive interval from observation tick through checked expiry; a zero-length
interval is usable only at that exact tick.

All serialized event structs reject unknown fields. `decode_event` caps JSON input at 1 MiB; strings and identifiers are bounded, and registry collections are capped at 1024. `reduce_enrollment` has no filesystem, clock, network, randomness, or process effects. Callers supply the current tick explicitly. Events return updated registry proposals only.
The 1 MiB streaming bound also applies to typed events; all registry maps,
including revocation and retained scope maps, are bounded before cloning and
the registry is limited to 8 MiB. IDs are portable ASCII and other context
strings reject controls. JSON decoding rejects duplicate fields. Registry
deserialization does not establish trust: only a controller-owned canonical
projection may be supplied as state. Older snapshots without retained scope
or clock history require replay/migration from authenticated records before
request admission; absent history must not be guessed.

## Not established here

- Secure token generation, delivery, storage, and secret custody.
- Private-key creation, persistence, protection, or proof of possession.
- Certificate issuance, renewal, revocation distribution, or trust-root custody.
- mTLS negotiation, peer authentication, protocol compatibility, or transport claims.
- Durable event ACK, sequence continuity, replay range transfer, backpressure, or artifact transfer.
- Host inventory probing, connection, auto-enrollment, GPU/runner grants, scheduling, or execution authority.

A registered identity or capacity proposal is not authenticated transport proof. A separate transport implementation must establish certificate custody, peer identity, durable ACK/replay semantics, and compatibility before making those claims.
The authenticated request adapter, secret-possession checks, durable recorded
clock envelope and atomic store integration are not implemented by this
component. Importing these APIs does not enroll a live host or enable native
fleet execution.
