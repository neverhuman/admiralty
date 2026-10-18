use admiralty_core::{
    AttemptId, DomainEvent, PrincipalId, ProjectId, RootId, SourceIdentity, TaskContract, TaskId,
};
use admiralty_store::{Cursor, Store, StoreError};
use std::{sync::Arc, thread};

const OBSERVED_UTC: &str = "2026-10-18T00:00:00Z";

fn contract(id: &str) -> TaskContract {
    serde_json::from_value(serde_json::json!({
        "schema_version": 1,
        "project_id": "durable-project",
        "root_id": "durable-root",
        "task_id": id,
        "revision": 1,
        "title": "Exercise durable lifecycle",
        "objective": "Persist and replay task lifecycle transitions",
        "non_goals": [],
        "owned_paths": ["src/lib.rs"],
        "forbidden_paths": [],
        "read_first": [],
        "dependencies": [],
        "criteria": [{"id":"C1","text":"Candidate remains unaccepted","check_ids":["unit"]}],
        "checks": [{"id":"unit","program":"cargo","args":["test"],"cwd":".","timeout_secs":30}],
        "authority_ref": "fixture:task-only",
        "max_attempts": 2,
        "simulated": true
    }))
    .expect("fixture contract deserializes")
}

fn source(producer: &str, sequence: u64) -> SourceIdentity {
    SourceIdentity {
        producer_id: producer.to_owned(),
        incarnation: "fixture-incarnation-1".into(),
        sequence: sequence.to_string(),
    }
}

fn append(
    store: &Store,
    producer: &str,
    sequence: u64,
    event: DomainEvent,
) -> admiralty_store::AppendReceipt {
    store
        .append(source(producer, sequence), event, OBSERVED_UTC.into())
        .expect("valid fixture event appends")
}

fn submit(store: &Store, producer: &str, sequence: u64, task: &str) {
    append(
        store,
        producer,
        sequence,
        DomainEvent::TaskSubmitted {
            contract: contract(task),
        },
    );
}

fn task_claim(task: &str, revision: u32, attempt: &str, owner: &str) -> DomainEvent {
    DomainEvent::TaskClaimed {
        task_id: TaskId::new(task).expect("fixture task ID"),
        revision,
        attempt_id: AttemptId::new(attempt).expect("fixture attempt ID"),
        owner: PrincipalId::new(owner).expect("fixture principal ID"),
    }
}

fn assert_database_private(path: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            path.metadata()
                .expect("fixture database metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}

fn private_tempdir() -> tempfile::TempDir {
    let directory = tempfile::tempdir().expect("private test directory");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
            .expect("set fixture directory mode");
        assert_eq!(
            directory
                .path()
                .metadata()
                .expect("fixture metadata")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
    directory
}

#[test]
fn submit_claim_start_candidate_survive_close_reopen_and_rebuild_without_acceptance() {
    let directory = private_tempdir();
    let database = directory.path().join("store.sqlite");
    let store = Store::open(&database).expect("open fixture store");
    assert_database_private(&database);
    let beginning = store.snapshot().expect("initial cursor");
    submit(&store, "lifecycle-producer", 1, "lifecycle-task");
    append(
        &store,
        "lifecycle-producer",
        2,
        task_claim("lifecycle-task", 1, "attempt-1", "worker-1"),
    );
    append(
        &store,
        "lifecycle-producer",
        3,
        DomainEvent::TaskStarted {
            task_id: TaskId::new("lifecycle-task").expect("fixture task ID"),
            attempt_id: AttemptId::new("attempt-1").expect("fixture attempt ID"),
        },
    );
    append(
        &store,
        "lifecycle-producer",
        4,
        DomainEvent::CandidateSubmitted {
            task_id: TaskId::new("lifecycle-task").expect("fixture task ID"),
            attempt_id: AttemptId::new("attempt-1").expect("fixture attempt ID"),
            candidate_ref: "sha256:fixture-candidate".into(),
        },
    );
    let durable = store.snapshot().expect("lifecycle snapshot");
    let task = durable
        .projection
        .tasks
        .get(&TaskId::new("lifecycle-task").expect("fixture task ID"))
        .expect("submitted task projects");
    assert_eq!(
        task.state,
        admiralty_core::SchedulingState::AwaitingVerification
    );
    assert_eq!(
        task.candidate_ref.as_deref(),
        Some("sha256:fixture-candidate")
    );
    drop(store);

    let reopened = Store::open(&database).expect("reopen closed fixture store");
    let reopened_snapshot = reopened.snapshot().expect("reopened snapshot");
    assert_eq!(reopened_snapshot.cursor, durable.cursor);
    assert_eq!(reopened_snapshot.projection, durable.projection);
    let page = reopened
        .events_since(&beginning.cursor, 16)
        .expect("read events after original cursor");
    assert_eq!(page.events.len(), 4);
    assert_eq!(page.cursor.event_seq, "4");
    let rebuilt = reopened.rebuild().expect("rebuild from durable events");
    assert_eq!(rebuilt.projection, durable.projection);
    assert_eq!(rebuilt.cursor.event_seq, durable.cursor.event_seq);
    assert_ne!(
        rebuilt.cursor.recovery_generation,
        durable.cursor.recovery_generation
    );
    assert!(matches!(
        reopened.events_since(&durable.cursor, 1),
        Err(StoreError::Invalid(_))
    ));
    let task = rebuilt
        .projection
        .tasks
        .get(&TaskId::new("lifecycle-task").expect("fixture task ID"))
        .expect("rebuilt task projects");
    assert_eq!(
        task.state,
        admiralty_core::SchedulingState::AwaitingVerification
    );
    drop(reopened);
}

#[test]
fn replay_collision_and_invalid_transitions_preserve_event_sequence_and_projection() {
    let directory = private_tempdir();
    let database = directory.path().join("store.sqlite");
    let store = Store::open(&database).expect("open fixture store");
    assert_database_private(&database);
    let event = DomainEvent::TaskSubmitted {
        contract: contract("guarded-task"),
    };
    let original = append(&store, "guard-producer", 1, event.clone());
    let snapshot = store.snapshot().expect("snapshot after submit");
    let replay = store
        .append(source("guard-producer", 1), event, OBSERVED_UTC.into())
        .expect("identical source replay is idempotent");
    assert!(replay.duplicate);
    assert_eq!(replay.event_seq, original.event_seq);
    assert_eq!(
        store.snapshot().expect("snapshot after duplicate"),
        snapshot
    );

    let conflict = store.append(
        source("guard-producer", 1),
        DomainEvent::TaskSubmitted {
            contract: contract("different-task"),
        },
        OBSERVED_UTC.into(),
    );
    assert!(matches!(conflict, Err(StoreError::SourceCollision { .. })));
    let after_collision = store.snapshot().expect("snapshot after recorded collision");
    assert_eq!(after_collision.cursor.event_seq, snapshot.cursor.event_seq);
    assert_eq!(after_collision.projection, snapshot.projection);
    assert_eq!(after_collision.integrity_incidents, "1");

    let stale_revision = task_claim("guarded-task", 2, "stale-attempt", "worker-1");
    assert!(matches!(
        store.append(
            source("stale-source", 1),
            stale_revision,
            OBSERVED_UTC.into()
        ),
        Err(StoreError::Transition(_))
    ));
    let foreign_task = task_claim("missing-task", 1, "foreign-attempt", "worker-1");
    assert!(matches!(
        store.append(
            source("foreign-source", 1),
            foreign_task,
            OBSERVED_UTC.into()
        ),
        Err(StoreError::Transition(_))
    ));
    let candidate_before_start = DomainEvent::CandidateSubmitted {
        task_id: TaskId::new("guarded-task").expect("fixture task ID"),
        attempt_id: AttemptId::new("not-claimed").expect("fixture attempt ID"),
        candidate_ref: "sha256:premature".into(),
    };
    assert!(matches!(
        store.append(
            source("candidate-source", 1),
            candidate_before_start,
            OBSERVED_UTC.into()
        ),
        Err(StoreError::Transition(_))
    ));
    let after_rejections = store
        .snapshot()
        .expect("snapshot after rejected transitions");
    assert_eq!(after_rejections.cursor.event_seq, snapshot.cursor.event_seq);
    assert_eq!(after_rejections.projection, snapshot.projection);
}

#[test]
fn source_gap_reports_only_contiguous_prefix_until_missing_event_arrives() {
    let directory = private_tempdir();
    let database = directory.path().join("store.sqlite");
    let store = Store::open(&database).expect("open fixture store");
    assert_database_private(&database);
    let late = append(
        &store,
        "gapped-producer",
        2,
        DomainEvent::TaskSubmitted {
            contract: contract("gap-task-2"),
        },
    );
    assert_eq!(late.source_contiguous_seq, "0");
    let first = append(
        &store,
        "gapped-producer",
        1,
        DomainEvent::TaskSubmitted {
            contract: contract("gap-task-1"),
        },
    );
    assert_eq!(first.source_contiguous_seq, "2");
    let duplicate = store
        .append(
            source("gapped-producer", 2),
            DomainEvent::TaskSubmitted {
                contract: contract("gap-task-2"),
            },
            OBSERVED_UTC.into(),
        )
        .expect("late event replay");
    assert!(duplicate.duplicate);
    assert_eq!(duplicate.source_contiguous_seq, "2");
}

#[test]
fn competing_claims_serialize_to_one_transition_and_one_rejection() {
    let directory = private_tempdir();
    let database = directory.path().join("store.sqlite");
    let store = Store::open(&database).expect("open fixture store");
    assert_database_private(&database);
    submit(&store, "claim-setup", 1, "race-task");
    let before = store.snapshot().expect("snapshot before claim race");
    let shared = Arc::new(store.clone());
    let results = thread::scope(|scope| {
        let mut handles = Vec::new();
        for (producer, owner) in [
            ("claim-source-a", "worker-a"),
            ("claim-source-b", "worker-b"),
        ] {
            let store = Arc::clone(&shared);
            handles.push(scope.spawn(move || {
                store.append(
                    source(producer, 1),
                    task_claim("race-task", 1, "race-attempt", owner),
                    OBSERVED_UTC.into(),
                )
            }));
        }
        handles
            .into_iter()
            .map(|handle| handle.join().expect("claim thread completes"))
            .collect::<Vec<_>>()
    });
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Err(StoreError::Transition(_))))
            .count(),
        1
    );
    let after = store.snapshot().expect("snapshot after claim race");
    assert_eq!(
        after
            .cursor
            .event_seq
            .parse::<u64>()
            .expect("decimal event sequence"),
        before
            .cursor
            .event_seq
            .parse::<u64>()
            .expect("decimal event sequence")
            + 1
    );
    let task = after
        .projection
        .tasks
        .get(&TaskId::new("race-task").expect("fixture task ID"))
        .expect("race task remains projected");
    assert_eq!(task.state, admiralty_core::SchedulingState::Claimed);
    assert!(task.owner.as_ref().is_some_and(|owner| owner
        == &PrincipalId::new("worker-a").expect("fixture principal")
        || owner == &PrincipalId::new("worker-b").expect("fixture principal")));
    drop(shared);
    drop(store);
}

#[test]
fn event_cursor_is_bound_to_recovery_generation_and_task_scope_types_are_real_ids() {
    let directory = private_tempdir();
    let database = directory.path().join("store.sqlite");
    let store = Store::open(&database).expect("open fixture store");
    assert_database_private(&database);
    submit(&store, "cursor-source", 1, "cursor-task");
    let snapshot = store.snapshot().expect("current cursor");
    let cursor = Cursor {
        recovery_generation: snapshot.cursor.recovery_generation.clone(),
        event_seq: "0".into(),
    };
    let batch = store.events_since(&cursor, 1).expect("bounded cursor read");
    assert_eq!(batch.events.len(), 1);
    assert_eq!(batch.cursor.event_seq, "1");
    assert!(ProjectId::new("durable-project").is_ok());
    assert!(RootId::new("durable-root").is_ok());
}
