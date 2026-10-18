# Node enrollment bookkeeping

This document describes the pure `admiralty-core` enrollment proposal model. It records scoped enrollment tokens, node identity claims, generation changes, revocation, and timestamped capacity observations. It does not enroll a live machine or prove that a claimant possesses a key.

## Pure reducer

`NodeId` is a locally validated string identifier. `NodeScope` binds an enrollment to one project, root, task, and authority principal. An `EnrollmentToken` stores a SHA-256 digest of its one-time secret, never the plaintext secret. Enrollment requires an explicit known clock strictly before the expiry tick and an exact scope match. The first binding consumes the token. Repeating the same event is idempotent; replaying the token with a changed node, boot ID, key digest, or scope fails.

Node identity carries a boot ID, public-key digest, capability digest, and canonical decimal-string generation. Rotation and revocation require the current generation and advance it with checked arithmetic. Capacity observations bind to the exact node, boot, and generation, and require a known timestamp and current time within the declared freshness interval. Unknown time, stale boot, revoked identity, stale generation, malformed digest, noncanonical counter, overflow, unknown JSON fields, and oversized input fail closed. No missing observation is interpreted as free capacity.

All serialized event structs reject unknown fields. `decode_event` caps JSON input at 1 MiB; strings and identifiers are bounded, and registry collections are capped at 1024. `reduce_enrollment` has no filesystem, clock, network, randomness, or process effects. Callers supply the current tick explicitly. Events return updated registry proposals only.

## Not established here

- Secure token generation, delivery, storage, and secret custody.
- Private-key creation, persistence, protection, or proof of possession.
- Certificate issuance, renewal, revocation distribution, or trust-root custody.
- mTLS negotiation, peer authentication, protocol compatibility, or transport claims.
- Durable event ACK, sequence continuity, replay range transfer, backpressure, or artifact transfer.
- Host inventory probing, connection, auto-enrollment, GPU/runner grants, scheduling, or execution authority.

A registered identity or capacity proposal is not authenticated transport proof. A separate transport implementation must establish certificate custody, peer identity, durable ACK/replay semantics, and compatibility before making those claims.
