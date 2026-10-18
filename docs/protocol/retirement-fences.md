# Retirement fences (pure component)

This protocol defines proposals for a registered asset identified by project,
root, task, asset ID and owner. Registration is evidence of declared custody;
asset discovery never creates registration or custody. The component consumes
explicit observations and has no filesystem, process, pin-store, backup or
clock access. It cannot quarantine or remove anything.

## Evidence and state transitions

An observation carries registration status, stable device/inode identity, the
registration digest and custody generation, pin revision and active pin count,
and known/unknown preservation facts. Missing evidence stays unknown. Unknown
registration, identity, pin state, active process state, or preservation fact
refuses a proposal. Dirty, ignored/private, unpushed, unmerged, unique, foreign,
backup-held or process-held content is retained.

The reducer uses an explicit generation compare-and-swap. Each command also has
a bounded idempotency key and canonical serialized digest: replaying that exact
key and command returns its recorded state; reusing the key with different
bytes is a collision. Generation increments use checked arithmetic and the
wire representation is a canonical decimal string.

The only clean path is `preview -> claimed -> quarantine_proposed ->
delete_proposed`. Claim records the identity and pin generation. An independent
observation after claim must match identity, pin revision and active-pin count
before quarantine can be proposed. A second independent observation must match
again before a delete proposal. Any change refuses the transition; a caller
must start a fresh preview and claim. `delete_proposed` always has
`deletion_authorized = false`: an action-boundary executor must separately earn
qualification and enforce the scoped capability.

## Snapshot and action-boundary TOCTOU

These observations are snapshots, not locks. Even repeated matching identity
and pin observations leave a time-of-check/time-of-use gap before a future
action. A production executor must bind the final check to the actual operation
with qualified no-follow filesystem handles, mount and symlink identity checks,
process identity stronger than PID, durable claim enforcement against new
references, and recovery for interrupted quarantine. It must recheck custody,
cleanliness, ignored/private content, remote Git state, process activity and
backup/candidate holds at that boundary. Until that executor is independently
qualified, proposals are records for review only and never authorize deletion.

The pure component intentionally does not scan paths, kill processes, remove
worktrees, call providers, enroll nodes, or perform any deletion.
