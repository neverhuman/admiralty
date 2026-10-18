# Question lifecycle

`admiralty_core::coordination::questions` is a pure prototype for bounded,
scoped requests for information. It performs no I/O or model turns and is not
active fleet coordination until its caller supplies durable custody, actor
identity, grant validation and atomic delivery persistence. The reducer checks
consistency of recorded scope and authority references; matching strings alone
are not authorization.

Call `reduce_question(&current, &event)`. Every event includes its event ID and
`QuestionScope { question_id, project_id, task_id }`. Opening binds that scope,
exact authority reference, asker, initial recipient, explicit recipient
allowlist, blocking requirement, text, options and optional UTC deadline.
Subsequent events include the exact scope, authority reference, expected
revision and acting principal. A question ID or grant that happens to resemble
another question does not substitute for its full scope.

## Bounded data and history

The complete serialized event is limited to 16 KiB, counting JSON escaping,
options, authority, identities and all other fields. Individual text fields
must be nonblank and at most 16 KiB; there may be at most 32 options. These
individual caps do not permit the aggregate event to exceed 16 KiB. An authority
reference is nonempty, at most 512 UTF-8 bytes and contains no whitespace or
control characters. Text, negations, units, option order and blocking
requirements are preserved exactly; no fields are summarized or truncated.

The opening allowlist has 1 through 32 distinct principals, includes the
initial recipient and excludes the asker. The caller validates that these
principals are permitted by the referenced task grant before recording the
opening. The allowlist is immutable. Routing to an unlisted principal requires
a separately authorized new question rather than silently broadening this one.

Recipients cannot be revisited. One question therefore has at most 31 routes,
plus its opening and one terminal event: at most 33 accepted event receipts.
Receipts contain event IDs and SHA-256 fingerprints rather than full historical
text copies. They are never evicted to bypass a bound. Route admission reserves
one receipt for a terminal answer, cancellation, supersession or expiry. The
projection keeps bounded opening text and, if answered, its bounded answer.
Invalid or oversized events fail before copying text into new state.

## States, actors and revisions

Opening creates revision 1. Each accepted transition increments the revision
with checked arithmetic. Every subsequent transition must match scope,
authority and expected revision. Terminal states cannot transition again.

The asker or current recipient may route to an unvisited allowlisted
recipient. Only the current recipient may answer; a former recipient loses
that role when the question is routed. Only the asker may cancel or supersede.
The asker or current recipient may submit a recorded deadline observation.
The caller authenticates these actor identities, checks current grant
revocation and validates observation provenance; an untrusted producer cannot
assign itself another principal merely by writing an actor field.

Supersession is terminal and records a different question ID in the same
project/task scope. It does not open or validate the replacement question.
The caller records that separately under its own complete authority. An answer
is proposed information and never itself creates or authorizes a decision.

## Deadlines and replay time

`EvaluateDeadline` carries `observed_utc: Option<u64>` in the immutable event.
The reducer receives no out-of-band clock argument and never substitutes the
replay machine's current time. A missing observation, absent deadline or
observation at or before the deadline fails unchanged. Expiry requires a
strictly later recorded observation and stores that exact observation in
`QuestionStatus::Expired { observed_utc }`. A maximum integer deadline cannot
overflow into premature expiry.

Expiry returns an escalation proposal with the original project, task,
question, authority, asker and current recipient. Its `expired_at_utc` is the
recorded observation time, not a fabricated estimate of when the deadline was
first crossed. It grants no new authority and manufactures no answer or
decision. A late answer cannot erase terminal expiry.

Reconstructing from the same immutable events produces the same state and
proposal. Reapplying an accepted event to its current projection is idempotent:
it returns the existing state with `replayed = true`, without another revision
or escalation. Changing the deadline observation, scope, answer or any other
content under the same retained event ID is an integrity error. A distinct
event with an old expected revision is rejected.

The caller must atomically persist accepted event, resulting projection and
escalation/delivery outbox before acknowledging input or sending anything.
Duplicate reduction does not re-emit a lost proposal; transport retries belong
to that durable outbox. Reconstruct state from trusted journal events rather
than accepting arbitrary caller-edited projection fields. No automatic
durability, grant authentication, or fleet activation is claimed here.

Inline offline tests cover cross-question/project/task rejection, participant
routing and former-recipient exclusion, cycles, bounds including escaping and
aggregate text, terminal receipt reserve, terminal truth, exact authority,
recorded-time replay collisions, unknown clocks, overflow and unknown fields.
