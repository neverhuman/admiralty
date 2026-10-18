# SQLite backup and reopen proof

The `backup_reopen` integration test exercises the existing `admiralty-store::Store` API using only temporary, test-owned files. It appends a valid task, claim and coordination message with fixed source identity and observed timestamp, captures a snapshot, calls `Store::backup`, and checks that the returned snapshot equals both the pre-backup source snapshot and the snapshot obtained by opening the offline backup. It then appends another event to the source and verifies the backup retains the earlier event watermark and claimed projection. Rebuilding the opened backup must preserve its event watermark and projection while rotating the recovery generation.

The tests also verify existing public behavior: a second writer is rejected while the original store owns the database; an existing backup destination is preserved and a failed attempt removes its private stage file; symlink database/destination paths and a group/world-writable database parent are rejected. They do not access an installed database or service.

Fixtures use portable temporary directories restricted to mode `0700` on Unix,
with no required host path or `CARGO_TARGET_DIR` setting. The parent environment
is unchanged. Run `rtk cargo test -p admiralty-store --test backup_reopen`.

This is a SQLite snapshot and event-replay proof only. The returned `Snapshot` watermark does not prove artifact closure, schema/event compatibility across releases, a fresh externally protected recovery generation, executor fencing, remote mutation reconciliation, service continuity, or disaster-recovery readiness. No restore-over-live, fleet recovery, acceptance, or deployment claim follows from this test. Artifact closure and external fencing remain future qualification gates.
