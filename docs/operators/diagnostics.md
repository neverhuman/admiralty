# Recorded status diagnostics

`explain_status` turns one bounded, typed status record into fixed reason codes,
a short explanation, source provenance and a small set of typed next actions. It
uses only the caller-supplied clock tick and status evidence. It does not refresh
the source, infer unknown values, launch commands, access files or contact
providers. An unknown or future source tick remains explicitly unknown/future;
a stale source is reported as stale.

Tool wait means the recorded provider is waiting for a tool. It does not mean the
task is idle or stalled. Unknown process ownership recommends reconciliation;
the available actions never signal or restart a process. Unknown provider
liveness or evidence requests missing evidence rather than assuming success.

`verified_recorded` and `integrated_recorded` describe recorded claims. A
self-reported claim or one without independent provenance is called
unconfirmed. Even independently sourced evidence is not accepted by this API:
the output always has `execution_granted: false` and may request manual review.
Candidate heads and receipt digests are emitted only after SHA-256 validation.
Free-form logs, command lines, credentials, URLs and arbitrary diagnostic text
are neither inputs nor output fields. The typed API accepts canonical tick
strings through `Tick::new`/`Tick::as_str`; future JSON adapters must preserve
that decimal-string representation. Typed identifiers are validated by the
core ID constructors, and other text fields have explicit length and character
limits.

This pure component is not registered as a CLI command and performs no
subprocess or network work. CLI wiring, actual source adapters, native process
ownership reconciliation, provider qualification, live operational checks and
independent acceptance remain unimplemented and unqualified.
