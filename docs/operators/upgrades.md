# Upgrade and rollback boundary

Record source merge, built binary/package, installed version and running session
identities separately. A merged commit or private installed directory does not
prove activation. New-work aliases must not change the versions of active jobs.

Prepare a consistent SQLite backup and explicit artifact closure, then run
[offline recovery preflight](recovery.md). Preserve every active CLI, runtime,
provider adapter, prompt, policy, oracle and image pin for the job's lifetime.
Missing blobs, incompatible or unknown replay/adapter compatibility, unknown
session custody, unresolved remote effects and an unfenced old executor keep
activation held. Restore of an old database cannot fence external credentials.

For an eventual service handoff, stop new admissions for the selected owned
scope, drain or independently preserve active sessions, back up, obtain an
authenticated fresh external fence, reconcile effects and resume exactly once.
Rollback must fence the new admission/effect owner before returning scope
ownership to the old version. Never run both schedulers or infer custody from a
PID, terminal name or a backup's reader cursor.

Current checks establish real online backup/reopen, isolated restore/rebuild,
artifact verification and fail-closed recovery proposals. The independent
session daemon retains its child across client reconnects; that proves client
reconnect continuity only. Ordinary service restart, PTY survival, signed complete
bundle activation and production rollback for native execution remain
unqualified. `admiralty release activate`/`rollback` only switch the
`current` link of installed read-only console bundles (see
[deployment with systemd](deployment-systemd.md)); they perform no service
control, fencing or drain and grant no execution authority.
