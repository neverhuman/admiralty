# Publication fences

This document describes the pure candidate contract in `admiralty-core::publication`.
It does not provide a forge client, credentials, an executor, or publication authority.
An eligible result is a proposal only and always has `execution_granted: false`.

## Admission evidence

A proposal is eligible only when the observed head and base are both present and
match the expected head and base. Every configured reviewer needs an approval for
the current head, with a nonempty independent provenance reference and no
dismissal. Every configured check needs known passing evidence for that same head.
Missing or unknown evidence blocks admission. An owner-only target blocks a
non-owner regardless of approval or prose. The requested authority generation
must be canonical decimal and at least the generation stored in the scope; callers
must retain and enforce the highest generation independently of expiring leases.

Inputs are bounded before deserialization to 1 MiB. Vectors are limited to 1,024
entries and strings to 4,096 bytes. Serialized counters use canonical decimal
strings so JavaScript clients do not lose integer precision. Arithmetic is checked;
generation overflow is an error.

## One unresolved effect slot

Intents are keyed by stable identity and request digest. Exact replay is
idempotent; reuse of a key for different bytes is a collision. The intent stages
are `proposed`, `claimed`, `ambiguous`, `reconciled_success`, and
`reconciled_no_effect`. A timeout after a possible remote effect moves the intent
to `ambiguous`. That state retains the target slot. Lease expiry and cancellation
do not release it.

Only a matching reconciliation receipt with an independent provenance reference
can resolve an ambiguous intent. A receipt is itself replay-safe; conflicting
receipts fail. A no-effect reconciliation permits a new attempt only after the
caller re-evaluates the current head, base, authority generation, and policy. A
success reconciliation records observed remote state and closes the slot.

Changed head or base evidence blocks a new proposal. The caller must invalidate
the prior candidate and re-evaluate checks and reviews. The reducer is pure and
does not contact a forge, create a permit, or perform a remote action.

## Evidence limits

Digest consistency does not authenticate a real forge or prove that a provenance
reference is independent. The production gateway must obtain evidence through a
trusted adapter, isolate write credentials, serialize the target slot durably,
and recheck the frozen head/base/generation at the effect boundary. No live or
disposable forge qualification is included in this candidate; that qualification
remains pending.
