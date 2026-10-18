use admiralty_core::{
    AttemptId, CoordinationMessage, DomainEvent, MessageId, MessagePayload, PrincipalId, ProjectId,
    SourceIdentity, TaskContract, TaskId, content_digest,
};
use admiralty_store::{Store, StoreError};
use std::path::Path;

const OBSERVED: &str = "2026-10-18T00:00:00Z";

fn contract() -> TaskContract {
    serde_json::from_value(serde_json::json!({
        "schema_version":1,
        "project_id":"restart-project",
        "root_id":"restart-root",
        "task_id":"RESTART-TASK",
        "revision":3,
        "title":"Restart coordination fixture",
        "objective":"Persist a pending assignment and its authorized acknowledgment",
        "non_goals":["Do not start a provider"],
        "owned_paths":["src/lib.rs"],
        "forbidden_paths":[],
        "read_first":[],
        "dependencies":[],
        "criteria":[{"id":"C1","text":"Assignment digest matches exact task contract","check_ids":["unit"]}],
        "checks":[{"id":"unit","program":"cargo","args":["test"],"cwd":".","timeout_secs":30}],
        "authority_ref":"grant:restart-test",
        "max_attempts":1,
        "simulated":true
    }))
    .expect("complete valid task contract fixture")
}

fn assignment(contract: &TaskContract, message_id: &str) -> CoordinationMessage {
    CoordinationMessage {
        schema_version: 1,
        message_id: MessageId::new(message_id).expect("valid message ID fixture"),
        project_id: contract.project_id.clone(),
        sender: PrincipalId::new("coordinator").expect("valid sender fixture"),
        recipient: PrincipalId::new("worker").expect("valid recipient fixture"),
        causation_id: None,
        context_epoch: "worker-epoch-7".into(),
        authority_ref: contract.authority_ref.clone(),
        payload: MessagePayload::TaskAssigned {
            task_id: contract.task_id.clone(),
            revision: contract.revision,
            contract_digest: content_digest(contract).expect("canonical contract digest"),
        },
    }
}

fn source(producer: &str, incarnation: &str, sequence: u64) -> SourceIdentity {
    SourceIdentity {
        producer_id: producer.into(),
        incarnation: incarnation.into(),
        sequence: sequence.to_string(),
    }
}

fn append(
    store: &Store,
    producer: &str,
    incarnation: &str,
    seq: u64,
    event: DomainEvent,
) -> admiralty_store::AppendReceipt {
    store
        .append(source(producer, incarnation, seq), event, OBSERVED.into())
        .expect("fixture event append succeeds")
}

fn unchanged_after_rejection(
    store: &Store,
    before: &admiralty_store::Snapshot,
    result: Result<(), StoreError>,
) {
    assert!(
        result.is_err(),
        "the unauthorized or conflicting event must be rejected"
    );
    let after = store.snapshot().expect("snapshot after rejected event");
    assert_eq!(
        after.cursor, before.cursor,
        "rejection cannot advance durable source/event cursor"
    );
    assert_eq!(
        after.projection, before.projection,
        "rejection cannot mutate projection"
    );
}

fn private_fixture_dir(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .expect("restrict fixture directory to its owner");
        assert_eq!(
            std::fs::metadata(path)
                .expect("fixture directory metadata")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
}

#[test]
fn pending_assignment_and_ack_survive_store_restart_with_scoped_validation() {
    let directory = tempfile::tempdir().expect("private fixture tempdir");
    private_fixture_dir(directory.path());
    let database = directory.path().join("coordination.sqlite");
    let task = contract();

    let store = Store::open(&database).expect("open real SQLite event store");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&database)
                .expect("database metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    append(
        &store,
        "coordinator",
        "inc-1",
        1,
        DomainEvent::TaskSubmitted {
            contract: task.clone(),
        },
    );
    append(
        &store,
        "coordinator",
        "inc-1",
        2,
        DomainEvent::TaskClaimed {
            task_id: task.task_id.clone(),
            revision: task.revision,
            attempt_id: AttemptId::new("attempt-1").expect("valid attempt fixture"),
            owner: PrincipalId::new("worker").expect("valid worker fixture"),
        },
    );
    let message = assignment(&task, "assignment-1");
    append(
        &store,
        "coordinator",
        "inc-1",
        3,
        DomainEvent::MessageQueued {
            message: message.clone(),
        },
    );
    let pending = store.snapshot().expect("pending message snapshot");
    assert_eq!(
        pending.projection.messages.get(&message.message_id),
        Some(&message)
    );
    assert!(
        !pending
            .projection
            .acknowledgments
            .contains_key(&message.message_id)
    );

    // The ACK API currently carries only message ID, recipient and epoch. Wrong
    // project/task/authority are therefore exercised at assignment validation,
    // where the existing API has those fields and rejects them before commit.
    for forged in [
        {
            let mut value = message.clone();
            value.message_id = MessageId::new("forged-project").expect("valid forged message ID");
            value.project_id = ProjectId::new("other-project").expect("valid forged project");
            value
        },
        {
            let mut value = message.clone();
            value.message_id = MessageId::new("forged-task").expect("valid forged message ID");
            value.payload = MessagePayload::TaskAssigned {
                task_id: TaskId::new("OTHER-TASK").expect("valid forged task"),
                revision: task.revision,
                contract_digest: content_digest(&task).expect("canonical task digest"),
            };
            value
        },
        {
            let mut value = message.clone();
            value.message_id = MessageId::new("forged-authority").expect("valid forged message ID");
            value.authority_ref = "grant:other".into();
            value
        },
    ] {
        let before = store.snapshot().expect("before forged assignment");
        unchanged_after_rejection(
            &store,
            &before,
            store
                .append(
                    source("coordinator", "inc-1", 4),
                    DomainEvent::MessageQueued { message: forged },
                    OBSERVED.into(),
                )
                .map(|_| ()),
        );
    }

    for (recipient, epoch) in [("intruder", "worker-epoch-7"), ("worker", "stale-epoch")] {
        let before = store.snapshot().expect("before forged acknowledgment");
        let event = DomainEvent::MessageAcknowledged {
            message_id: message.message_id.clone(),
            recipient: PrincipalId::new(recipient).expect("valid recipient fixture"),
            context_epoch: epoch.into(),
        };
        unchanged_after_rejection(
            &store,
            &before,
            store
                .append(source("coordinator", "inc-1", 4), event, OBSERVED.into())
                .map(|_| ()),
        );
    }

    let correct_ack = DomainEvent::MessageAcknowledged {
        message_id: message.message_id.clone(),
        recipient: message.recipient.clone(),
        context_epoch: message.context_epoch.clone(),
    };
    let ack_receipt = append(&store, "coordinator", "inc-1", 4, correct_ack.clone());
    let exact_replay = append(&store, "coordinator", "inc-1", 4, correct_ack.clone());
    assert!(exact_replay.duplicate, "exact source replay is idempotent");
    assert_eq!(exact_replay.event_seq, ack_receipt.event_seq);
    assert_eq!(
        store
            .snapshot()
            .expect("acknowledged snapshot")
            .projection
            .acknowledgments
            .get(&message.message_id),
        Some(&message.context_epoch)
    );
    let acknowledged_task = store.snapshot().expect("task state after application ACK");
    assert_eq!(acknowledged_task.projection.tasks, pending.projection.tasks);
    assert!(
        acknowledged_task.projection.tasks[&task.task_id]
            .candidate_ref
            .is_none(),
        "an application ACK does not submit or accept a candidate"
    );

    // The same message ID with changed bytes is rejected by the real reducer.
    let mut changed_message = message.clone();
    changed_message.context_epoch = "worker-epoch-8".into();
    let before_changed_message = store.snapshot().expect("before changed message ID reuse");
    unchanged_after_rejection(
        &store,
        &before_changed_message,
        store
            .append(
                source("coordinator", "inc-1", 5),
                DomainEvent::MessageQueued {
                    message: changed_message,
                },
                OBSERVED.into(),
            )
            .map(|_| ()),
    );

    // Producer sequence gaps are durable and contiguous watermarks can advance
    // after restart when the missing earlier event is later supplied.
    let gap_event = DomainEvent::MessageAcknowledged {
        message_id: message.message_id.clone(),
        recipient: message.recipient.clone(),
        context_epoch: message.context_epoch.clone(),
    };
    let gap = append(&store, "replay-producer", "inc-gap", 2, gap_event.clone());
    assert_eq!(gap.source_contiguous_seq, "0");
    let generation = store
        .snapshot()
        .expect("record generation before restart")
        .cursor
        .recovery_generation;
    drop(store);

    let reopened = Store::open(&database).expect("reopen real SQLite store");
    let after_restart = reopened.snapshot().expect("durable reopened snapshot");
    assert_eq!(
        after_restart.cursor.recovery_generation, generation,
        "recorded generation survives restart"
    );
    assert_eq!(
        after_restart.projection.messages.get(&message.message_id),
        Some(&message)
    );
    assert_eq!(
        after_restart
            .projection
            .acknowledgments
            .get(&message.message_id),
        Some(&message.context_epoch)
    );
    assert_eq!(
        after_restart
            .projection
            .tasks
            .get(&task.task_id)
            .expect("task projection")
            .state,
        admiralty_core::SchedulingState::Claimed
    );
    let gap_fill = append(&reopened, "replay-producer", "inc-gap", 1, gap_event);
    assert_eq!(gap_fill.source_contiguous_seq, "2");
    assert_eq!(
        reopened
            .snapshot()
            .expect("post gap-fill snapshot")
            .projection,
        after_restart.projection
    );
}

#[test]
fn source_identity_payload_collision_does_not_advance_cursor_or_projection() {
    let directory = tempfile::tempdir().expect("private collision fixture tempdir");
    private_fixture_dir(directory.path());
    let store = Store::open(directory.path().join("collision.sqlite"))
        .expect("open collision fixture store");
    let task = contract();
    append(
        &store,
        "producer",
        "inc",
        1,
        DomainEvent::TaskSubmitted {
            contract: task.clone(),
        },
    );
    let original = DomainEvent::TaskClaimed {
        task_id: task.task_id.clone(),
        revision: task.revision,
        attempt_id: AttemptId::new("attempt-collision").expect("valid attempt fixture"),
        owner: PrincipalId::new("worker").expect("valid worker fixture"),
    };
    append(&store, "producer", "inc", 2, original.clone());
    let before = store.snapshot().expect("before source collision");
    let changed = DomainEvent::TaskClaimed {
        task_id: task.task_id,
        revision: 2,
        attempt_id: AttemptId::new("attempt-collision").expect("valid attempt fixture"),
        owner: PrincipalId::new("worker").expect("valid worker fixture"),
    };
    let error = store
        .append(source("producer", "inc", 2), changed, OBSERVED.into())
        .expect_err("changed source bytes are a collision");
    assert!(matches!(error, StoreError::SourceCollision { .. }));
    let after = store.snapshot().expect("after source collision");
    assert_eq!(after.cursor.event_seq, before.cursor.event_seq);
    assert_eq!(after.projection, before.projection);
    assert_eq!(
        after
            .integrity_incidents
            .parse::<u64>()
            .expect("canonical incident count"),
        before
            .integrity_incidents
            .parse::<u64>()
            .expect("canonical incident count")
            + 1
    );
}
