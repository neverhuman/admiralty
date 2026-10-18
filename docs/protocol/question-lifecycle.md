# Question lifecycle

Questions are durable, task-scoped requests for information. A question binds
its immutable question ID, project ID, task ID, authority reference, asker,
recipient, blocking requirement, text, optional choices and optional absolute
UTC deadline. Project, task, participants and authority are copied into every
subsequent lifecycle decision; callers must not resolve the question by a
similar-looking ID or silently substitute a newer grant.

Question text and each option are nonempty UTF-8 strings of at most 16 KiB.
There may be at most 32 options. The deadline is the recorded UTC Unix second
or absent; the reducer does not read a clock.

## States and revisions

The states are `open`, `routed`, `answered`, `superseded`, `expired` and
`cancelled`. Opening creates revision 1. Each accepted transition increments
the revision by one. Every transition after opening carries the expected
revision and exact authority reference. A mismatch is rejected without
changing state. Terminal states cannot transition again.

Only the current recipient may answer. The asker may route the request or
cancel it; the current recipient may route it onward. Supersession is an
explicit terminal transition that records a different question ID. The caller
must separately open that new question with its own immutable scope and ID.
An answer records a proposed response; it does not itself create or authorize
a decision.

## Deadlines and escalation

Deadline evaluation requires a known caller-supplied UTC second and a recorded
deadline. The question remains live at the exact deadline second and expires
only when `now_utc > deadline_utc`. An unknown clock, missing deadline or
not-yet-passed deadline is an error with no state change.

Expiry produces an escalation proposal scoped to the same project, task,
question and authority, with the asker and current recipient recorded. It does
not manufacture an answer, alter authority, or commit a decision. Routing and
escalation policy decides who receives a proposal next.

## Replay

Lifecycle events have IDs. Reapplying the same event ID with identical content
returns the current state as a replay without incrementing the revision or
creating another escalation. Reusing an event ID with changed content is an
integrity error. A distinct event using an old expected revision is rejected.

The pure reducer lives in `admiralty_core::coordination::questions`; module
registration is owned by the integration lane. It performs no I/O and has no
live effects.
