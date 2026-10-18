# Staged audit proposals

The runtime audit component evaluates bounded evidence for one exact staged index. `AuditScope` binds project, root, task and contract digest. The required identity binds the full base commit and staged tree IDs, SHA-256 profile, policy and protected oracle digests, plus the named auditor. Every identity field must match. Evidence attributed to a working tree, an index that does not match its staged tree, or a changed base/profile/policy/oracle/auditor yields an invalidated proposal; it cannot substitute a working preview for staged content.

The caller supplies trusted policy and validates the auditor's real identity.
The required identity also pins the exact staged-content capsule and custody
attestation digests. Evidence's capsule must match that protected expected
value; a different content hash cannot be substituted merely by repeating the
same staged tree ID. `evaluate_staged_audit_for` additionally binds an
independently authenticated auditor and verified custody reference supplied by
the controller. Those trusted arguments must not be copied from the same
worker JSON. The lower-level evaluation API checks internal consistency of
already-admitted records; it does not establish trust on its own.

The protected policy digest is recomputed over the required paths, hard finding
IDs, exact check argv/cwd/timeouts, affected-test IDs, negative controls and
optional score threshold. Changing any of these without changing the digest
is malformed. Both policy and evidence fingerprints bind
`AUDIT_POLICY_VERSION`; analysis semantics changes must advance that version.
The full audit fingerprint also binds project/root/task/contract and every
expected identity/custody/capsule field. A digest binds bytes, not authenticity.

This pure component does not authenticate Jankurai or any auditor, verify
attestations, materialize Git indexes, run hooks, invoke processes, or grant
acceptance. `acceptance_granted`, `execution_granted` and
`publication_granted` are always false. Proposal action fields are private,
exposed through read-only getters and serialization, and cannot be elevated by
deserializing worker JSON into an `AuditProposal`. The resulting proposal is
input to a later independent decision, never a task/candidate acceptance
receipt or an activated audit hook.

Missing required path coverage or full context returns `needs_full_context`. A profile that does not support the requested audit scope returns `unsupported`. Missing, failed or unknown required check results produce a bounded `reproduce` proposal containing only the declared `CheckSpec` argv/cwd/timeout and immutable evidence references; the reducer never executes those checks. Unknown exit status is not zero. Hard findings always block a pass proposal, regardless of score or threshold. Scores use canonical decimal strings and remain unknown when absent. Callers receive an explicit `score_unknown` outcome when policy requires a threshold and no score is present.

Policy must declare nonempty required paths, required checks, affected-test
IDs drawn from those checks, and negative controls with disjoint IDs. Each
negative control pins its own expected exit code; an expected failure exit may
be the correct negative-control result. `Passed` means the observed exit equals
that protected expected exit, including nonzero values, rather than a blanket
zero-exit heuristic. Each result must bind its exact executed `CheckSpec`, full
scope and staged identity, with an immutable output evidence reference for any
known exit. Missing negative results still require reproduction, even with a
high score. The controller's trusted profile must choose the complete affected
and negative check set; this component does not infer coverage or prove that a
claimed command actually ran.

A caller can supply a persisted prior audit ID and evidence digest, but the
outcome is always recomputed. A prior `pass_proposal` cannot override failed,
unknown, changed-scope or invalidated evidence; a mismatching supplied outcome
fails. The same ID with changed evidence fails as a collision.
`AuditRegistry` retains controller-owned digests across intervening audit IDs
and supplies exact replay without evicting old collision history. It holds at
most 1024 IDs; exhaustion rejects new IDs while existing exact replays remain
available. Use its `evaluate_for` variant at an authenticated admission
boundary. Registry construction/evaluation is local bookkeeping, not durable
publication; persistence and replay of independently admitted records remain
the integration owner's responsibility.

`decode_policy` and `decode_evidence` bound JSON to 1 MiB and reject unknown or
duplicate fields without echoing malformed source text. Combined typed inputs
receive the same streaming size bound without allocating a complete JSON
copy. Lists/argv are bounded to 1024 items, context strings to 4096 bytes, and
named IDs to portable ASCII identifiers of at most 128 bytes. Paths use a
bounded portable ASCII component subset, rejecting traversal, dot aliases,
repeated separators, drive/ADS syntax, controls and Windows device aliases.
Only a check's cwd may be the exact root marker `.`. This lexical policy is not
symlink resolution or filesystem custody. Callers must bound body/file reads
before creating byte slices for the parsers.

Jankurai qualification on Admiralty's Rust, SQLite and React profile remains pending, as do exact-index materialization across partial or alternate indexes, linked worktrees, unborn HEADs, renames/binaries, and preservation of existing hook chains and `core.hooksPath`. This packet deliberately provides only the pure staged-audit decision component and its protocol contract.
Full Git IDs are syntactically checked; object existence and index/content
equivalence are not verified here. Coverage/profile/index booleans and result
records remain producer assertions until the caller verifies their provenance.
Native integration still needs protected oracle custody, independent runner
evidence, actual affected-test selection, auditor grants, atomic append/replay
storage and exact staged-generation invalidation. No live quality tool,
pre-commit hook, worker audit flow or native fleet execution is qualified by
these unit transitions.
