# Provider supervision prototype

This pure component accepts bounded typed observations, seven independent
clocks, persisted detector state and an optional recorded logical tick. It
returns a classification and evidence request, never a process-control action.
It performs no I/O and is not active fleet supervision or a qualified provider
capability. Its proposal is serialize-only; private `may_signal`,
`may_send_input` and `execution_granted` flags remain false and have getters.

Scope includes project, root, task, attempt, owner, producer incarnation,
contract revision/digest and exact authority reference. Persisted state must
match that full scope and the hash of the complete detection policy. Policy
changes require an explicit migration, not a version-string-only reset.
The caller authenticates every identity and grant; matching fields are
consistency checks rather than proof of authority.

`evaluate_supervision(policy, observation, previous, Option<&Tick>)` requires
a known canonical decimal u64 tick. Unknown time, backwards time, future
observations, regressing known signal clocks and invalid previous clocks fail
unchanged. Missing samples retain previous known clocks for future rollback
checks while remaining missing evidence for current classification. A repeated
identical observation at the same tick returns `replayed=true` without
incrementing hysteresis or emitting another evidence request. Changed content
at one already-recorded tick is a collision.

All seven clocks are separately retained. New clock evidence resets repeated
stall samples. A quiet running tool is waiting, not dead; an exited/unknown
process contradicting a running tool requires investigation. Missing clocks
cannot establish health. Suspect-stall evidence requests need at least two
stale clocks, distinct repeated samples and the configured cooldown. The
hysteresis counter saturates at its validated threshold rather than growing
forever. Policy has positive stall duration and at most 1,024 hysteresis samples.

Inputs and persisted state use the shared streaming 1 MiB JSON admission bound,
including escaping, before cloning. Fingerprints store SHA-256 digests rather
than complete observation JSON. Approval/wait text is at most 4 KiB; policy
version at most 128 bytes; incarnation is a bounded typed-identifier string and
authority a whitespace/control-free reference of at most 512 bytes. Quoted,
historical, example or negated approval text never authorizes input or signals.

There are no historical PID actions in this interface. A future executor must
independently prove live child/process-group ownership and incarnation before
any signal, and must never cancel a reused historical PID based on timestamps.
Durable observation identities, atomic detector-state/evidence-outbox commits,
provider-specific detector qualification and clock-source custody remain
pending. Reconstruct trusted state from the journal rather than accepting an
arbitrary edited detector snapshot. No universal stall-detection or token-saving
claim is made by these offline tests.
