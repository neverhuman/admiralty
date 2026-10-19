# Native recovery diagnostics

`NativeExecutor::inspect_run_journal` inspects metadata for the existing
prepared, observed and closed journal files for one validated run ID. It uses
the retained journal directory descriptor and the three fixed hashed filenames;
callers cannot select arbitrary paths. Each result reports kind, byte length and
the SHA-256 of the original bytes. It does not parse or return their contents.

The reader rejects symlinks, nonregular files, unexpected ownership, additional
hard links, nonprivate modes and files larger than 512 KiB. It checks entry and
descriptor identity before and after the bounded read. These sequential checks
do not create an atomic snapshot of all three files or isolate a process from
another writer with the same UID. No missing file is synthesized as a completed
run, and no inspection creates a recovery or execution capability.

`ResourceUsageDiagnostics.history` retains up to eight observed resource samples
in attempt order. A sample enters history only when it contains an actual
dynamic counter or flag; configured limits and empty event containers alone do
not count. Observed zero and false values do count. Each entry preserves its own
identity, readings and sampling attempt without filling unknown fields from
another sample. Missing final cgroup files therefore do not erase the last
readable history. Existing before/latest/final readings keep their meanings.
Closing diagnostics freezes history as well as the existing fields.

Older serialized diagnostics without history decode as eight empty entries.
The serialized array has a fixed length of eight. History and inspected journal
metadata remain advisory: neither establishes a resource-exhaustion cause,
current authority, completion, a safe retry, released capacity or settled cost.
They install no recovery controller, new polling loop or default provider route.

The 19 October authorized `bcf5cce` diagnostic predates these changes. Its CLI
turn completed, but its code-mode helper reported EAGAIN and made no edits;
source preflight rejected the result. Initial resource counters were readable
and final counters were missing. The new history cannot recover measurements
that this older binary did not retain. The consumed one-run grant remains
consumed. A short completed run also does not qualify deadline enforcement:
its software executor deadline was 120 seconds, while the recorded systemd
maximum reflected the longer permit remainder. The repaired launch API now
requires the trusted executor duration and sets systemd's maximum to the smaller
of that duration and the original permit/resource-lease remainder. It floors
to positive milliseconds and rejects zero, submillisecond and expired bounds
before launch. Claims, resource caps and unknown-outcome custody stay unchanged.
The two existing callers supply their original configured deadlines. Wrapper
argument tests do not establish actual timeout or stop-grace enforcement; a
future provider run still requires a fresh exact authorization and binary.
