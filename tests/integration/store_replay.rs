use admiralty_core::{
    AttemptId, DomainEvent, SchedulingState, SourceIdentity, TaskContract, TaskId,
};
use admiralty_store::{Store, StoreError};
use serde_json::json;

const NOW: &str = "2026-10-18T12:00:00Z";

fn source(sequence: u64) -> SourceIdentity {
    SourceIdentity {
        producer_id: "producer".into(),
        incarnation: "boot-1".into(),
        sequence: sequence.to_string(),
    }
}

fn task(id: &str) -> DomainEvent {
    let contract: TaskContract = serde_json::from_value(json!({
        "schema_version":1,"project_id":"project","root_id":"root","task_id":id,"revision":1,
        "title":"Implement bounded task","objective":"Deliver scoped change","non_goals":["No external publication"],
        "owned_paths":["src/task.rs"],"forbidden_paths":["secrets"],"read_first":["README.md"],"dependencies":[],
        "criteria":[{"id":"C1","text":"Named check passes","check_ids":["T1"]}],
        "checks":[{"id":"T1","program":"cargo","args":["test"],"cwd":".","timeout_secs":60}],
        "authority_ref":"policy:test","max_attempts":2,"simulated":true
    })).expect("valid test contract");
    DomainEvent::TaskSubmitted { contract }
}

#[test]
fn duplicate_and_collision_survive_reopen() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("state.sqlite");
    let store = Store::open(&path).expect("open");
    let first = store
        .append(source(1), task("T1"), NOW.into())
        .expect("append");
    let duplicate = store
        .append(source(1), task("T1"), "later observation".into())
        .expect("duplicate");
    assert_eq!(first.event_seq, duplicate.event_seq);
    assert!(duplicate.duplicate);
    assert!(matches!(
        store.append(source(1), task("different"), NOW.into()),
        Err(StoreError::SourceCollision { .. })
    ));
    assert_eq!(store.snapshot().expect("snapshot").integrity_incidents, "1");
    assert_eq!(store.events_after(0, 100).expect("events").len(), 1);
    let before = store.snapshot().expect("snapshot");
    drop(store);
    let reopened = Store::open(&path).expect("reopen");
    assert_eq!(reopened.snapshot().expect("snapshot"), before);
    assert!(
        reopened
            .append(source(1), task("T1"), NOW.into())
            .expect("duplicate after restart")
            .duplicate
    );
}

#[test]
fn source_ack_never_skips_gaps() {
    let directory = tempfile::tempdir().expect("tempdir");
    let store = Store::open(directory.path().join("state.sqlite")).expect("open");
    assert_eq!(
        store
            .append(source(2), task("T2"), NOW.into())
            .expect("seq2")
            .source_contiguous_seq,
        "0"
    );
    assert_eq!(
        store
            .append(source(1), task("T1"), NOW.into())
            .expect("seq1")
            .source_contiguous_seq,
        "2"
    );
    assert_eq!(
        store
            .append(source(4), task("T4"), NOW.into())
            .expect("seq4")
            .source_contiguous_seq,
        "2"
    );
    assert_eq!(
        store
            .append(source(3), task("T3"), NOW.into())
            .expect("seq3")
            .source_contiguous_seq,
        "4"
    );
    let duplicate = store
        .append(source(2), task("T2"), NOW.into())
        .expect("duplicate");
    assert_eq!(duplicate.event_seq, "1");
    assert_eq!(duplicate.source_contiguous_seq, "4");
    assert!(duplicate.duplicate);
}

#[test]
fn failed_transition_is_atomic_and_candidates_cannot_self_accept() {
    let directory = tempfile::tempdir().expect("tempdir");
    let store = Store::open(directory.path().join("state.sqlite")).expect("open");
    let task_id: TaskId = "T1".to_owned().try_into().expect("task id");
    let attempt_id: AttemptId = "attempt-1".to_owned().try_into().expect("attempt id");
    let start = DomainEvent::TaskStarted {
        task_id: task_id.clone(),
        attempt_id: attempt_id.clone(),
    };
    assert!(store.append(source(1), start.clone(), NOW.into()).is_err());
    assert_eq!(store.snapshot().expect("snapshot").cursor.event_seq, "0");
    store
        .append(source(1), task("T1"), NOW.into())
        .expect("submit");
    store
        .append(
            source(2),
            DomainEvent::TaskClaimed {
                task_id: task_id.clone(),
                revision: 1,
                attempt_id: attempt_id.clone(),
                owner: "worker".to_owned().try_into().expect("owner"),
            },
            NOW.into(),
        )
        .expect("claim");
    store.append(source(3), start, NOW.into()).expect("start");
    store
        .append(
            source(4),
            DomainEvent::CandidateSubmitted {
                task_id: task_id.clone(),
                attempt_id,
                candidate_ref: "self-report:verified-and-integrated".into(),
            },
            NOW.into(),
        )
        .expect("candidate");
    let snapshot = store.snapshot().expect("snapshot");
    assert_eq!(
        snapshot.projection.tasks[&task_id].state,
        SchedulingState::AwaitingVerification
    );
    assert_eq!(snapshot.cursor.event_seq, "4");
    let rebuilt = store.rebuild().expect("pure replay");
    assert_eq!(snapshot.projection, rebuilt.projection);
    assert_eq!(snapshot.cursor.event_seq, rebuilt.cursor.event_seq);
    assert_ne!(
        snapshot.cursor.recovery_generation,
        rebuilt.cursor.recovery_generation
    );
}

#[test]
fn one_owner_and_many_clones_share_a_serial_writer() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("state.sqlite");
    let store = Store::open(&path).expect("open");
    assert!(matches!(Store::open(&path), Err(StoreError::AlreadyOwned)));
    let workers: Vec<_> = (1..=16)
        .map(|number| {
            let store = store.clone();
            std::thread::spawn(move || {
                let mut identity = source(1);
                identity.producer_id = format!("worker-{number}");
                store
                    .append(identity, task(&format!("T{number}")), NOW.into())
                    .expect("concurrent append")
            })
        })
        .collect();
    for worker in workers {
        worker.join().expect("worker thread");
    }
    assert_eq!(
        store.snapshot().expect("snapshot").projection.tasks.len(),
        16
    );
    assert_eq!(store.events_after(0, 100).expect("events").len(), 16);
    assert_eq!(store.events_after(0, 0).expect("empty page").len(), 0);
    drop(store);
    Store::open(path).expect("last clone releases owner lock");
}

#[test]
fn backup_is_consistent_and_never_overwrites_existing_or_live_files() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("state.sqlite");
    let backup_path = directory.path().join("backup.sqlite");
    let store = Store::open(&path).expect("open");
    store
        .append(source(1), task("T1"), NOW.into())
        .expect("append");
    let backed_up = store.backup(&backup_path).expect("backup");
    let before = std::fs::read(&backup_path).expect("backup bytes");
    assert!(store.backup(&backup_path).is_err());
    assert_eq!(std::fs::read(&backup_path).expect("backup bytes"), before);
    assert!(store.backup(&path).is_err());
    let recovered = Store::open(&backup_path).expect("open backup");
    assert_eq!(recovered.snapshot().expect("backup snapshot"), backed_up);
    store
        .append(source(2), task("T2"), NOW.into())
        .expect("later append");
    assert_eq!(
        recovered
            .snapshot()
            .expect("backup snapshot")
            .projection
            .tasks
            .len(),
        1
    );
}

#[test]
fn tampered_durable_payload_fails_replay_without_silent_repair() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("state.sqlite");
    let store = Store::open(&path).expect("open");
    store
        .append(source(1), task("T1"), NOW.into())
        .expect("append");
    drop(store);
    let database = rusqlite::Connection::open(&path).expect("fault injection connection");
    database
        .execute(
            "UPDATE domain_events SET payload=?1 WHERE seq=1",
            [serde_json::to_string(&task("T2")).expect("serialize")],
        )
        .expect("tamper");
    drop(database);
    assert!(matches!(Store::open(path), Err(StoreError::Replay { .. })));
}

#[test]
fn sequence_encoding_rejects_noncanonical_and_out_of_sqlite_range() {
    let directory = tempfile::tempdir().expect("tempdir");
    let store = Store::open(directory.path().join("state.sqlite")).expect("open");
    for sequence in ["0", "01", "-1", "18446744073709551615"] {
        let mut identity = source(1);
        identity.sequence = sequence.into();
        assert!(store.append(identity, task("T1"), NOW.into()).is_err());
    }
    assert_eq!(store.snapshot().expect("snapshot").cursor.event_seq, "0");
}

#[test]
fn failed_sql_insert_never_publishes_an_in_memory_projection() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("state.sqlite");
    let store = Store::open(&path).expect("open");
    let fault = rusqlite::Connection::open(&path).expect("fault injection connection");
    fault.execute_batch("CREATE TRIGGER fail_event BEFORE INSERT ON domain_events BEGIN SELECT RAISE(FAIL,'injected persistence failure'); END;")
        .expect("install insert failure");
    assert!(store.append(source(1), task("T1"), NOW.into()).is_err());
    let rejected = store.snapshot().expect("snapshot after insert failure");
    assert_eq!(rejected.cursor.event_seq, "0");
    assert!(rejected.projection.tasks.is_empty());
    fault
        .execute_batch("DROP TRIGGER fail_event;")
        .expect("remove fault");
    store
        .append(source(1), task("T1"), NOW.into())
        .expect("same identity remains reusable after rollback");
    assert_eq!(
        store
            .snapshot()
            .expect("committed snapshot")
            .projection
            .tasks
            .len(),
        1
    );
}

#[test]
fn atomic_event_batches_reject_old_generations_and_preserve_pagination() {
    let directory = tempfile::tempdir().expect("tempdir");
    let store = Store::open(directory.path().join("state.sqlite")).expect("open");
    let initial = store.snapshot().expect("initial snapshot").cursor;
    store
        .append(source(1), task("T1"), NOW.into())
        .expect("first event");
    store
        .append(source(2), task("T2"), NOW.into())
        .expect("second event");
    let first = store.events_since(&initial, 1).expect("first page");
    assert_eq!(first.events.len(), 1);
    assert_eq!(first.cursor.event_seq, "1");
    let second = store.events_since(&first.cursor, 1).expect("second page");
    assert_eq!(second.events.len(), 1);
    assert_eq!(second.cursor.event_seq, "2");
    let empty = store.events_since(&second.cursor, 1).expect("empty page");
    assert!(empty.events.is_empty());
    assert_eq!(empty.cursor, second.cursor);
    let mut ahead = second.cursor.clone();
    ahead.event_seq = "3".into();
    assert!(
        matches!(store.events_since(&ahead, 10), Err(StoreError::Invalid(reason)) if reason.starts_with("snapshot_required"))
    );
    let rebuilt = store.rebuild().expect("rebuild");
    assert!(
        matches!(store.events_since(&second.cursor, 10), Err(StoreError::Invalid(reason)) if reason.starts_with("snapshot_required"))
    );
    assert!(
        store
            .events_since(&rebuilt.cursor, 10)
            .expect("new generation")
            .events
            .is_empty()
    );
}

#[test]
fn live_event_reads_refuse_tampered_payloads() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("state.sqlite");
    let store = Store::open(&path).expect("open");
    store
        .append(source(1), task("T1"), NOW.into())
        .expect("append");
    let mut cursor = store.snapshot().expect("snapshot").cursor;
    cursor.event_seq = "0".into();
    let database = rusqlite::Connection::open(&path).expect("fault injection");
    database
        .execute(
            "UPDATE domain_events SET payload=?1 WHERE seq=1",
            [serde_json::to_string(&task("forged")).expect("JSON")],
        )
        .expect("tamper");
    assert!(matches!(
        store.events_after(0, 10),
        Err(StoreError::Replay { .. })
    ));
    assert!(matches!(
        store.events_since(&cursor, 10),
        Err(StoreError::Replay { .. })
    ));
}

#[cfg(unix)]
#[test]
fn database_backup_and_sidecars_are_private_and_symlinks_refused() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("private/state.sqlite");
    let store = Store::open(&path).expect("new private directory");
    store
        .append(source(1), task("T1"), NOW.into())
        .expect("append");
    let backup = directory.path().join("backup.sqlite");
    store.backup(&backup).expect("backup");
    assert_eq!(
        path.parent()
            .expect("parent")
            .metadata()
            .expect("mode")
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    for file in [
        path.clone(),
        path.with_file_name("state.sqlite-wal"),
        path.with_file_name("state.sqlite-shm"),
        path.with_file_name("state.sqlite.writer.lock"),
        backup,
    ] {
        assert_eq!(
            file.metadata().expect("file exists").permissions().mode() & 0o077,
            0,
            "{} must be private",
            file.display()
        );
    }
    drop(store);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))
        .expect("fault permissions");
    assert!(matches!(Store::open(&path), Err(StoreError::Invalid(_))));
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("restore");
    let alias = directory.path().join("alias.sqlite");
    symlink(&path, &alias).expect("symlink");
    assert!(matches!(Store::open(&alias), Err(StoreError::Invalid(_))));
}
