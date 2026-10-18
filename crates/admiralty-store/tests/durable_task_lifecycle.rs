use admiralty_core::{
    AttemptId, DomainEvent, PrincipalId, ProjectId, RootId, SourceIdentity, TaskContract, TaskId,
};
use admiralty_store::{Cursor, Store, StoreError};
use std::{
    sync::{Arc, Barrier},
    thread,
};

const OBSERVED_UTC: &str = "2026-10-18T00:00:00Z";

mod native_v2 {
    use super::*;
    use admiralty_core::{
        CandidateDecisionEvidence, LifecycleAction, LifecycleCommand, SchedulingState, TaskView,
        content_digest,
    };
    use admiralty_store::LifecycleAuthority;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct FixtureAuthority(AtomicBool);
    impl LifecycleAuthority for FixtureAuthority {
        fn authorize(
            &self,
            source: &SourceIdentity,
            command: &LifecycleCommand,
            task: &TaskView,
            effect: Option<&admiralty_store::LifecycleEffect<'_>>,
        ) -> bool {
            // A trusted test double, not runtime execution qualification. Native
            // integration must replace this predicate with actual custody evidence.
            self.0.load(Ordering::SeqCst)
                && effect.is_none()
                && source.producer_id == "trusted-service"
                && task.contract.project_id.as_str() == "durable-project"
                && command.actor.as_str() == "independent-verifier"
                && match &command.action {
                    LifecycleAction::DecideCandidate { evidence, .. } => {
                        evidence.verification_policy_digest == digest("policy")
                            && evidence.custody_digest == digest("custody")
                            && evidence.closure_digest == digest("supervised-closure")
                    }
                    _ => true,
                }
        }
    }
    fn digest(text: &str) -> String {
        content_digest(&text).expect("digest")
    }
    fn authority() -> Arc<FixtureAuthority> {
        Arc::new(FixtureAuthority(AtomicBool::new(true)))
    }
    fn command(store: &Store, id: &str, action: LifecycleAction) -> LifecycleCommand {
        let task_id = TaskId::new(id).expect("task");
        let task = store
            .snapshot()
            .expect("snapshot")
            .projection
            .tasks
            .remove(&task_id)
            .expect("task exists");
        LifecycleCommand {
            protocol_version: 2,
            task_id,
            expected_task_digest: content_digest(&task).expect("CAS"),
            actor: PrincipalId::new("independent-verifier").expect("actor"),
            action,
        }
    }
    fn execute(
        store: &Store,
        seq: u64,
        command: LifecycleCommand,
    ) -> admiralty_store::Result<admiralty_store::AppendReceipt> {
        store.execute_lifecycle(
            source("trusted-service", seq),
            command,
            OBSERVED_UTC.into(),
            None,
        )
    }
    fn running(store: &Store, id: &str) {
        submit(store, id, 1, id);
        append(
            store,
            id,
            2,
            task_claim(id, 1, &format!("{id}-attempt-1"), "worker"),
        );
        append(
            store,
            id,
            3,
            DomainEvent::TaskStarted {
                task_id: TaskId::new(id).expect("native fixture operation"),
                attempt_id: AttemptId::new(format!("{id}-attempt-1"))
                    .expect("native fixture operation"),
            },
        );
    }
    fn candidate(store: &Store, id: &str) {
        running(store, id);
        append(
            store,
            id,
            4,
            DomainEvent::CandidateSubmitted {
                task_id: TaskId::new(id).expect("native fixture operation"),
                attempt_id: AttemptId::new(format!("{id}-attempt-1"))
                    .expect("native fixture operation"),
                candidate_ref: "candidate-one".into(),
            },
        );
    }
    fn decide(store: &Store, id: &str, accepted: bool) -> LifecycleCommand {
        command(
            store,
            id,
            LifecycleAction::DecideCandidate {
                attempt_id: AttemptId::new(format!("{id}-attempt-1"))
                    .expect("native fixture operation"),
                candidate_ref: "candidate-one".into(),
                accepted,
                evidence: CandidateDecisionEvidence {
                    contract_digest: content_digest(&contract(id))
                        .expect("native fixture operation"),
                    verification_policy_digest: digest("policy"),
                    receipt_digest: digest("actual-checks"),
                    custody_digest: digest("custody"),
                    closure_digest: digest("supervised-closure"),
                    source_commit_sha: "a".repeat(40),
                    source_tree_sha: "b".repeat(40),
                    verifier: PrincipalId::new("independent-verifier")
                        .expect("native fixture operation"),
                },
            },
        )
    }
    #[test]
    fn protected_decision_denies_default_store_and_both_raw_append_paths() {
        let dir = private_tempdir();
        let store =
            Store::open(dir.path().join("default.sqlite")).expect("native fixture operation");
        candidate(&store, "one");
        let cmd = decide(&store, "one", true);
        let before = store.snapshot().expect("native fixture operation");
        assert!(execute(&store, 1, cmd.clone()).is_err());
        assert!(
            store
                .append(
                    source("raw", 1),
                    DomainEvent::TaskLifecycle {
                        command: cmd.clone()
                    },
                    OBSERVED_UTC.into()
                )
                .is_err()
        );
        assert!(
            store
                .append_with_intent(
                    source("raw", 2),
                    DomainEvent::TaskLifecycle { command: cmd },
                    OBSERVED_UTC.into(),
                    admiralty_store::outbox::OutboxIntent {
                        idempotency_key: "no-bypass".into(),
                        payload: serde_json::json!({}),
                        not_before: 0
                    }
                )
                .is_err()
        );
        assert_eq!(store.snapshot().expect("native fixture operation"), before);
    }
    #[test]
    fn acceptance_releases_dependencies_is_idempotent_and_replays_after_backup() {
        let dir = private_tempdir();
        let db = dir.path().join("accept.sqlite");
        let auth = authority();
        let store = Store::open_with_lifecycle_authority(&db, auth.clone())
            .expect("native fixture operation");
        candidate(&store, "one");
        let mut dependent = contract("two");
        dependent
            .dependencies
            .push(TaskId::new("one").expect("native fixture operation"));
        append(
            &store,
            "two",
            1,
            DomainEvent::TaskSubmitted {
                contract: dependent,
            },
        );
        let cmd = decide(&store, "one", true);
        let accepted = execute(&store, 1, cmd.clone()).expect("native fixture operation");
        assert!(
            execute(&store, 1, cmd.clone())
                .expect("native fixture operation")
                .duplicate
        );
        let snapshot = store.snapshot().expect("native fixture operation");
        assert_eq!(snapshot.schema_version, 2);
        assert_eq!(
            snapshot.projection.tasks[&TaskId::new("one").expect("native fixture operation")].state,
            SchedulingState::Accepted
        );
        assert_eq!(
            snapshot.projection.tasks[&TaskId::new("two").expect("native fixture operation")].state,
            SchedulingState::Ready
        );
        let mut late = contract("late");
        late.dependencies
            .push(TaskId::new("one").expect("native fixture operation"));
        append(
            &store,
            "late",
            1,
            DomainEvent::TaskSubmitted { contract: late },
        );
        assert_eq!(
            store
                .snapshot()
                .expect("native fixture operation")
                .projection
                .tasks[&TaskId::new("late").expect("native fixture operation")]
                .state,
            SchedulingState::Ready
        );
        auth.0.store(false, Ordering::SeqCst);
        assert!(
            execute(&store, 1, cmd.clone()).is_err(),
            "revocation precedes duplicate disclosure"
        );
        auth.0.store(true, Ordering::SeqCst);
        let mut collision = cmd;
        collision.actor =
            PrincipalId::new("independent-verifier").expect("native fixture operation");
        if let LifecycleAction::DecideCandidate { evidence, .. } = &mut collision.action {
            evidence.receipt_digest = digest("changed");
        }
        assert!(matches!(
            execute(&store, 1, collision),
            Err(StoreError::SourceCollision { .. })
        ));
        assert_eq!(accepted.event_seq, snapshot.cursor.event_seq);
        let final_projection = store
            .snapshot()
            .expect("native fixture operation")
            .projection;
        let backup = dir.path().join("backup.sqlite");
        store.backup(&backup).expect("native fixture operation");
        assert_eq!(
            store
                .rebuild()
                .expect("native fixture operation")
                .projection,
            final_projection
        );
        drop(store);
        let restored = Store::open(&backup).expect("native fixture operation");
        assert_eq!(
            restored
                .snapshot()
                .expect("native fixture operation")
                .projection,
            final_projection
        );
        let reopened = Store::open(&db).expect("native fixture operation");
        assert_eq!(
            reopened
                .snapshot()
                .expect("native fixture operation")
                .projection,
            final_projection
        );
    }
    #[test]
    fn stale_or_forged_verifier_candidate_policy_and_contract_cannot_decide() {
        let dir = private_tempdir();
        let store =
            Store::open_with_lifecycle_authority(dir.path().join("bindings.sqlite"), authority())
                .expect("native fixture operation");
        candidate(&store, "one");
        let cmd = decide(&store, "one", true);
        let before = store.snapshot().expect("native fixture operation");
        for field in 0..8 {
            let mut forged = cmd.clone();
            if let LifecycleAction::DecideCandidate {
                candidate_ref,
                attempt_id,
                evidence,
                ..
            } = &mut forged.action
            {
                match field {
                    0 => *candidate_ref = "other-candidate".into(),
                    1 => {
                        *attempt_id =
                            AttemptId::new("other-attempt").expect("native fixture operation")
                    }
                    2 => evidence.contract_digest = digest("other-contract"),
                    3 => evidence.verification_policy_digest = digest("other-policy"),
                    4 => evidence.custody_digest = digest("other-custody"),
                    5 => {
                        evidence.verifier =
                            PrincipalId::new("worker").expect("native fixture operation");
                        forged.actor = evidence.verifier.clone();
                    }
                    6 => evidence.closure_digest = digest("not-closed"),
                    _ => forged.expected_task_digest = digest("stale"),
                }
            }
            assert!(execute(&store, field + 1, forged).is_err());
            assert_eq!(store.snapshot().expect("native fixture operation"), before);
        }
    }
    #[test]
    fn closure_retry_preserves_attempt_history_and_enforces_budget() {
        let dir = private_tempdir();
        let store =
            Store::open_with_lifecycle_authority(dir.path().join("retry.sqlite"), authority())
                .expect("native fixture operation");
        running(&store, "one");
        let id = AttemptId::new("one-attempt-1").expect("native fixture operation");
        let close = command(
            &store,
            "one",
            LifecycleAction::CloseAttempt {
                attempt_id: id.clone(),
                closure_digest: digest("closed"),
                reason: "known child failure".into(),
                cancelled: false,
            },
        );
        execute(&store, 1, close).expect("native fixture operation");
        let retry = command(
            &store,
            "one",
            LifecycleAction::Retry {
                attempt_id: id.clone(),
                closure_digest: digest("closed"),
            },
        );
        execute(&store, 2, retry).expect("native fixture operation");
        assert!(
            store
                .append(
                    source("one", 4),
                    task_claim("one", 1, "one-attempt-1", "worker"),
                    OBSERVED_UTC.into()
                )
                .is_err()
        );
        append(
            &store,
            "one",
            5,
            task_claim("one", 1, "one-attempt-2", "worker"),
        );
        let close = command(
            &store,
            "one",
            LifecycleAction::CloseAttempt {
                attempt_id: AttemptId::new("one-attempt-2").expect("native fixture operation"),
                closure_digest: digest("closed-two"),
                reason: "known spawn failure".into(),
                cancelled: true,
            },
        );
        execute(&store, 3, close).expect("native fixture operation");
        let retry = command(
            &store,
            "one",
            LifecycleAction::Retry {
                attempt_id: AttemptId::new("one-attempt-2").expect("native fixture operation"),
                closure_digest: digest("closed-two"),
            },
        );
        assert!(execute(&store, 4, retry).is_err());
        let task = &store
            .snapshot()
            .expect("native fixture operation")
            .projection
            .tasks[&TaskId::new("one").expect("native fixture operation")];
        assert_eq!(
            task.lifecycle
                .as_ref()
                .expect("native fixture operation")
                .attempts
                .len(),
            2
        );
    }
    #[test]
    fn revision_and_pending_cancel_cannot_reinterpret_live_or_referenced_work() {
        let dir = private_tempdir();
        let store =
            Store::open_with_lifecycle_authority(dir.path().join("revision.sqlite"), authority())
                .expect("native fixture operation");
        submit(&store, "one", 1, "one");
        let mut revised = contract("one");
        revised.revision = 2;
        let revise = command(
            &store,
            "one",
            LifecycleAction::ReviseContract {
                contract: revised.clone(),
            },
        );
        execute(&store, 1, revise).expect("native fixture operation");
        let mut child = contract("child");
        child
            .dependencies
            .push(TaskId::new("one").expect("native fixture operation"));
        append(
            &store,
            "child",
            1,
            DomainEvent::TaskSubmitted { contract: child },
        );
        revised.revision = 3;
        let revise = command(
            &store,
            "one",
            LifecycleAction::ReviseContract { contract: revised },
        );
        assert!(execute(&store, 2, revise).is_err());
        append(&store, "one", 2, task_claim("one", 2, "attempt", "worker"));
        let cancel = command(
            &store,
            "one",
            LifecycleAction::CancelPending {
                reason: "stop".into(),
            },
        );
        assert!(execute(&store, 3, cancel).is_err());
        let cancel = command(
            &store,
            "child",
            LifecycleAction::CancelPending {
                reason: "owner cancelled pending work".into(),
            },
        );
        execute(&store, 4, cancel).expect("native fixture operation");
    }
    #[test]
    fn concurrent_decisions_commit_only_one_current_cas() {
        let dir = private_tempdir();
        let store =
            Store::open_with_lifecycle_authority(dir.path().join("race.sqlite"), authority())
                .expect("native fixture operation");
        candidate(&store, "one");
        let cmd = decide(&store, "one", true);
        let barrier = Arc::new(Barrier::new(2));
        let joins: Vec<_> = (1..=2)
            .map(|seq| {
                let store = store.clone();
                let cmd = cmd.clone();
                let barrier = barrier.clone();
                thread::spawn(move || {
                    barrier.wait();
                    execute(&store, seq, cmd)
                })
            })
            .collect();
        let results: Vec<_> = joins
            .into_iter()
            .map(|t| t.join().expect("native fixture operation"))
            .collect();
        assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
        assert_eq!(
            store
                .snapshot()
                .expect("native fixture operation")
                .cursor
                .event_seq,
            "5"
        );
    }

    #[test]
    fn optional_effect_must_match_exact_authorized_key_payload_and_schedule() {
        use admiralty_store::{LifecycleEffect, outbox::OutboxIntent};
        struct ExactEffectAuthority;
        impl LifecycleAuthority for ExactEffectAuthority {
            fn authorize(
                &self,
                source: &SourceIdentity,
                command: &LifecycleCommand,
                task: &TaskView,
                effect: Option<&LifecycleEffect<'_>>,
            ) -> bool {
                FixtureAuthority(AtomicBool::new(true)).authorize(source, command, task, None)
                    && effect.is_some_and(|effect| {
                        effect.idempotency_key == "release-one"
                            && effect.payload == r#"{"release":"one"}"#
                            && effect.not_before == 7
                    })
            }
        }
        let dir = private_tempdir();
        let db = dir.path().join("effects.sqlite");
        let store = Store::open_with_lifecycle_authority(&db, Arc::new(ExactEffectAuthority))
            .expect("native fixture operation");
        candidate(&store, "one");
        let cmd = decide(&store, "one", true);
        let before = store.snapshot().expect("native fixture operation");
        let intent = OutboxIntent {
            idempotency_key: "release-one".into(),
            payload: serde_json::json!({"release":"one"}),
            not_before: 7,
        };
        let fault = rusqlite::Connection::open(&db).expect("native fixture operation");
        fault.execute_batch("CREATE TRIGGER reject_native_binding BEFORE INSERT ON event_request_bindings BEGIN SELECT RAISE(ABORT,'native late fault'); END;").expect("native fixture operation");
        let changes = store.subscribe_changes();
        assert!(matches!(
            store.execute_lifecycle(
                source("trusted-service", 1),
                cmd.clone(),
                OBSERVED_UTC.into(),
                Some(intent.clone())
            ),
            Err(StoreError::Sqlite(_))
        ));
        assert_eq!(store.snapshot().expect("native fixture operation"), before);
        assert!(!changes.has_changed().expect("native fixture operation"));
        let effects: i64 = fault
            .query_row("SELECT COUNT(*) FROM outbox_intents", [], |r| r.get(0))
            .expect("native fixture operation");
        assert_eq!(
            effects, 0,
            "late SQL failure rolls back event, effect, projection and wake together"
        );
        fault
            .execute_batch("DROP TRIGGER reject_native_binding;")
            .expect("native fixture operation");
        drop(fault);
        for field in 0..3 {
            let mut forged = intent.clone();
            match field {
                0 => forged.idempotency_key = "unrelated".into(),
                1 => forged.payload = serde_json::json!({"publish":"main"}),
                _ => forged.not_before = 8,
            }
            assert!(
                store
                    .execute_lifecycle(
                        source("trusted-service", 1),
                        cmd.clone(),
                        OBSERVED_UTC.into(),
                        Some(forged)
                    )
                    .is_err()
            );
            assert_eq!(store.snapshot().expect("native fixture operation"), before);
        }
        let receipt = store
            .execute_lifecycle(
                source("trusted-service", 1),
                cmd.clone(),
                OBSERVED_UTC.into(),
                Some(intent.clone()),
            )
            .expect("native fixture operation");
        assert!(
            store
                .execute_lifecycle(
                    source("trusted-service", 1),
                    cmd,
                    OBSERVED_UTC.into(),
                    Some(intent)
                )
                .expect("native fixture operation")
                .duplicate
        );
        let connection = rusqlite::Connection::open(&db).expect("native fixture operation");
        let associations: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM event_outbox_associations WHERE event_seq=?1",
                [receipt
                    .event_seq
                    .parse::<i64>()
                    .expect("native fixture operation")],
                |r| r.get(0),
            )
            .expect("native fixture operation");
        assert_eq!(associations, 1);
    }

    #[test]
    fn revision_refuses_to_leave_a_stale_pending_assignment() {
        use admiralty_core::{CoordinationMessage, MessageId, MessagePayload};
        let dir = private_tempdir();
        let store =
            Store::open_with_lifecycle_authority(dir.path().join("assignment.sqlite"), authority())
                .expect("native fixture operation");
        submit(&store, "one", 1, "one");
        append(
            &store,
            "one",
            2,
            DomainEvent::MessageQueued {
                message: CoordinationMessage {
                    schema_version: 1,
                    message_id: MessageId::new("assignment").expect("native fixture operation"),
                    project_id: contract("one").project_id,
                    sender: PrincipalId::new("manager").expect("native fixture operation"),
                    recipient: PrincipalId::new("worker").expect("native fixture operation"),
                    causation_id: None,
                    context_epoch: "epoch-one".into(),
                    authority_ref: contract("one").authority_ref,
                    payload: MessagePayload::TaskAssigned {
                        task_id: TaskId::new("one").expect("native fixture operation"),
                        revision: 1,
                        contract_digest: content_digest(&contract("one"))
                            .expect("native fixture operation"),
                    },
                },
            },
        );
        let before = store.snapshot().expect("native fixture operation");
        let mut revised = contract("one");
        revised.revision = 2;
        let cmd = command(
            &store,
            "one",
            LifecycleAction::ReviseContract { contract: revised },
        );
        assert!(execute(&store, 1, cmd).is_err());
        assert_eq!(store.snapshot().expect("native fixture operation"), before);
    }
}

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
    let start = Arc::new(Barrier::new(2));
    let results = thread::scope(|scope| {
        let mut handles = Vec::new();
        for (producer, owner) in [
            ("claim-source-a", "worker-a"),
            ("claim-source-b", "worker-b"),
        ] {
            let store = Arc::clone(&shared);
            let start = Arc::clone(&start);
            handles.push(scope.spawn(move || {
                start.wait();
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
