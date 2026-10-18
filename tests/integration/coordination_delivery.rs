use admiralty_core::{
    AttemptId, CoordinationMessage, DomainEvent, MessageId, MessagePayload, PrincipalId, ProjectId,
    SourceIdentity, TaskContract, TaskId, content_digest,
};
use admiralty_server::services::coordination::{
    CoordinationAction, CoordinationAuthorizer, CoordinationService,
};
use admiralty_store::inbox::{InboxCursor, InboxScope, MAX_PENDING_PAGE, MAX_PENDING_SCAN};
use admiralty_store::{Store, StoreError};

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

fn private_fixture_dir() -> tempfile::TempDir {
    let base = std::fs::canonicalize(std::env::temp_dir()).expect("canonical temporary base");
    let mut builder = tempfile::Builder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    let directory = builder.tempdir_in(base).expect("private fixture directory");
    let path = directory.path();
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
    directory
}

#[test]
fn pending_assignment_and_ack_survive_store_restart_with_scoped_validation() {
    let directory = private_fixture_dir();
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

    // This low-level legacy event carries only message ID, recipient and epoch.
    // The action-authorized service additionally binds the exact project below.
    // Assignment validation separately binds task and contract authority.
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
    let before_ack_replay = reopened.snapshot().expect("before durable ACK replay");
    let ack_replay = append(&reopened, "coordinator", "inc-1", 4, correct_ack);
    assert!(
        ack_replay.duplicate,
        "durable ACK replay remains idempotent after reopen"
    );
    let after_ack_replay = reopened.snapshot().expect("after durable ACK replay");
    assert_eq!(after_ack_replay.cursor, before_ack_replay.cursor);
    assert_eq!(after_ack_replay.projection, before_ack_replay.projection);
}

#[test]
fn source_identity_payload_collision_does_not_advance_cursor_or_projection() {
    let directory = private_fixture_dir();
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

#[test]
fn question_delivery_replay_and_answer_are_idempotent_and_scoped() {
    use admiralty_core::coordination::decisions::{
        QuestionEvent, QuestionScope, QuestionStatus, is_terminal_decision, reduce_question,
    };
    let scope = QuestionScope {
        question_id: MessageId::new("q-1").expect("valid test value"),
        project_id: ProjectId::new("project-1").expect("valid test value"),
        task_id: TaskId::new("task-1").expect("valid test value"),
    };
    let captain = PrincipalId::new("captain").expect("valid test value");
    let worker = PrincipalId::new("worker").expect("valid test value");
    let opened = QuestionEvent::Open {
        event_id: MessageId::new("event-open").expect("valid test value"),
        scope: scope.clone(),
        authority_ref: "grant-1".into(),
        asker: captain.clone(),
        recipient: worker.clone(),
        allowed_recipients: vec![worker.clone()],
        requirement: "confirm output".into(),
        question: "Ready?".into(),
        options: vec!["yes".into(), "no".into()],
        deadline_utc: None,
    };
    let first = reduce_question(&None, &opened).expect("valid test value");
    assert_eq!(first.state.revision, 1);
    assert!(!first.replayed);
    assert!(!is_terminal_decision(&first.state));
    let replay = reduce_question(&Some(first.state.clone()), &opened).expect("valid test value");
    assert!(replay.replayed);
    assert_eq!(replay.state, first.state);

    let answered = QuestionEvent::Answer {
        event_id: MessageId::new("event-answer").expect("valid test value"),
        scope: scope.clone(),
        expected_revision: 1,
        authority_ref: "grant-1".into(),
        actor: worker,
        answer: "yes".into(),
    };
    let result = reduce_question(&Some(first.state), &answered).expect("valid test value");
    assert_eq!(result.state.revision, 2);
    assert!(is_terminal_decision(&result.state));
    assert_eq!(
        result.state.status,
        QuestionStatus::Answered {
            answer: "yes".into()
        }
    );

    let cross_task = QuestionEvent::Answer {
        event_id: MessageId::new("event-cross-task").expect("valid test value"),
        scope: QuestionScope {
            task_id: TaskId::new("other-task").expect("valid test value"),
            ..scope
        },
        expected_revision: 2,
        authority_ref: "grant-1".into(),
        actor: captain,
        answer: "no".into(),
    };
    assert!(reduce_question(&Some(result.state), &cross_task).is_err());
}

#[test]
fn loss_reorder_duplicate_storm_and_cross_project_isolation_converge_or_require_resnapshot() {
    use admiralty_core::coordination::{
        mailbox::{BoundedMailbox, DeliveryOutcome},
        message::validate_scoped_envelope,
        topics::{ScopedTopic, TopicAction, TopicAuthorizer, TopicDirectory},
    };
    use admiralty_server::services::coordination::CoordinationService;

    let task = contract();
    let project = task.project_id.clone();
    let worker = PrincipalId::new("worker").expect("valid worker");
    struct Policy {
        project: ProjectId,
        task: TaskId,
        worker: PrincipalId,
    }
    impl TopicAuthorizer for Policy {
        fn authorize(
            &self,
            actor: &PrincipalId,
            _action: TopicAction,
            topic: &ScopedTopic,
            subscriber: Option<&PrincipalId>,
        ) -> bool {
            actor == &self.worker
                && topic.project_id == self.project
                && topic.task_id.as_ref() == Some(&self.task)
                && subscriber.is_none_or(|value| value == &self.worker)
        }
    }
    let policy = Policy {
        project: project.clone(),
        task: task.task_id.clone(),
        worker: worker.clone(),
    };
    let mut topics = TopicDirectory::default();
    let topic = ScopedTopic {
        project_id: project.clone(),
        task_id: Some(task.task_id.clone()),
        channel: "progress".into(),
    };
    assert!(
        topics
            .subscribe(&worker, &policy, topic.clone(), worker.clone())
            .expect("subscribe in project")
    );
    let intruder = PrincipalId::new("intruder").expect("foreign actor");
    assert!(
        topics
            .resolve_page(&intruder, &policy, &topic, None, 10)
            .is_err()
    );
    assert_eq!(
        topics
            .resolve_page(&worker, &policy, &topic, None, 10)
            .expect("resolve subscribers")
            .subscribers,
        vec![worker.clone()]
    );

    let mut mailbox =
        BoundedMailbox::new(project.clone(), worker.clone(), 2).expect("bounded mailbox");
    let m1 = assignment(&task, "msg-1");
    let m2 = assignment(&task, "msg-2");
    let m3 = assignment(&task, "msg-3");
    validate_scoped_envelope(&m1, &project, Some(&worker)).expect("scoped envelope valid");

    // Out-of-order arrival (seq 2 before seq 1): buffered without advancing contiguous watermark.
    assert_eq!(
        CoordinationService::route_to_mailbox(&mut mailbox, 2, m2.clone()),
        DeliveryOutcome::BufferedOutOfOrder {
            seq: 2,
            contiguous_seq: 0
        }
    );
    assert!(mailbox.deliver_contiguous_batch(10).is_empty());

    // Duplicate of seq 2 is idempotent.
    assert_eq!(
        CoordinationService::route_to_mailbox(&mut mailbox, 2, m2),
        DeliveryOutcome::Duplicate { seq: 2 }
    );

    // Storm beyond unacknowledged capacity (capacity=2, sending 3rd unacked message) visibly requires resnapshot.
    assert_eq!(
        CoordinationService::route_to_mailbox(&mut mailbox, 3, m3.clone()),
        DeliveryOutcome::BufferedOutOfOrder {
            seq: 3,
            contiguous_seq: 0
        }
    );
    let m4 = assignment(&task, "msg-4");
    assert_eq!(
        CoordinationService::route_to_mailbox(&mut mailbox, 4, m4),
        DeliveryOutcome::RequiresResnapshot {
            capacity: 2,
            dropped_seq: 4
        }
    );
    assert!(mailbox.resnapshot_required());

    // A fresh mailbox epoch with enough history capacity accepts the missing prefix.
    let mut fresh_box = BoundedMailbox::new(project, worker, 3).expect("fresh bounded mailbox");
    assert_eq!(
        CoordinationService::route_to_mailbox(&mut fresh_box, 2, assignment(&task, "r-2")),
        DeliveryOutcome::BufferedOutOfOrder {
            seq: 2,
            contiguous_seq: 0
        }
    );
    assert_eq!(
        CoordinationService::route_to_mailbox(&mut fresh_box, 1, assignment(&task, "r-1")),
        DeliveryOutcome::Enqueued { seq: 1 }
    );
    let delivered = fresh_box.deliver_contiguous_batch(10);
    assert_eq!(delivered.len(), 2);
    assert!(fresh_box.ack_delivery(1));
    assert!(fresh_box.ack_delivery(2));
    assert!(fresh_box.deliver_contiguous_batch(10).is_empty());
}

struct ReadPolicy {
    scope: admiralty_server::services::coordination::CoordinationReadScope,
}

impl admiralty_server::services::coordination::CoordinationReadAuthorizer for ReadPolicy {
    fn authorize(
        &self,
        actor: &PrincipalId,
        scope: &admiralty_server::services::coordination::CoordinationReadScope,
    ) -> bool {
        actor == &self.scope.recipient && scope == &self.scope
    }
}

#[test]
fn recipient_queue_replay_is_bounded_scoped_and_survives_reopen() {
    use admiralty_server::services::coordination::{CoordinationReadScope, CoordinationService};
    let directory = private_fixture_dir();
    let database = directory.path().join("recipient-pages.sqlite");
    let store = Store::open(&database).expect("private real Store");
    let task = contract();
    let mut foreign_task = task.clone();
    foreign_task.project_id = ProjectId::new("foreign-project").expect("foreign project");
    foreign_task.task_id = TaskId::new("FOREIGN-TASK").expect("foreign task");
    foreign_task.authority_ref = "grant:foreign-task".into();
    append(
        &store,
        "paginator",
        "inc",
        1,
        DomainEvent::TaskSubmitted {
            contract: task.clone(),
        },
    );
    append(
        &store,
        "paginator",
        "inc",
        2,
        DomainEvent::TaskSubmitted {
            contract: foreign_task.clone(),
        },
    );
    let start = store.snapshot().expect("starting fixture cursor").cursor;
    let mine_one = assignment(&task, "mine-one");
    let mine_two = assignment(&task, "mine-two");
    let foreign = assignment(&foreign_task, "foreign-message");
    let mut wrong_recipient = assignment(&task, "different-recipient");
    wrong_recipient.recipient = PrincipalId::new("another-worker").expect("other worker");
    let mut wrong_epoch = assignment(&task, "different-epoch");
    wrong_epoch.context_epoch = "expired-epoch".into();
    let scope = CoordinationReadScope {
        project_id: task.project_id.clone(),
        recipient: mine_one.recipient.clone(),
        context_epoch: mine_one.context_epoch.clone(),
    };
    let policy = ReadPolicy {
        scope: scope.clone(),
    };
    let service = CoordinationService::new(&store);
    for (index, message) in [
        foreign,
        mine_one.clone(),
        wrong_recipient,
        wrong_epoch,
        mine_two.clone(),
    ]
    .into_iter()
    .enumerate()
    {
        service
            .enqueue_durable(
                source("paginator", "inc", index as u64 + 3),
                message,
                OBSERVED.into(),
            )
            .expect("durable valid queue event");
    }
    service
        .acknowledge_durable(
            source("paginator", "inc", 8),
            mine_one.message_id.clone(),
            mine_one.recipient.clone(),
            mine_one.context_epoch.clone(),
            OBSERVED.into(),
        )
        .expect("durable ACK");
    let first = service
        .queued_messages_since(&scope.recipient, &policy, &scope, &start, 2)
        .expect("first scoped page");
    assert_eq!(first.scanned_events, 2);
    assert_eq!(first.messages, vec![mine_one.clone()]);
    let second = service
        .queued_messages_since(&scope.recipient, &policy, &scope, &first.cursor, 2)
        .expect("filtered empty page");
    assert!(second.messages.is_empty());
    assert_eq!(second.scanned_events, 2);
    assert_ne!(
        second.cursor, first.cursor,
        "empty filtered pages advance the scanned durable cursor"
    );
    let third = service
        .queued_messages_since(&scope.recipient, &policy, &scope, &second.cursor, 2)
        .expect("last populated page");
    assert_eq!(third.messages, vec![mine_two]);
    assert_eq!(third.scanned_events, 2);
    let end = service
        .queued_messages_since(&scope.recipient, &policy, &scope, &third.cursor, 2)
        .expect("end page");
    assert!(end.messages.is_empty());
    assert_eq!(end.scanned_events, 0);
    assert_eq!(end.cursor, third.cursor);
    drop(store);
    let reopened = Store::open(&database).expect("reopen durable queue");
    let service = CoordinationService::new(&reopened);
    let replay = service
        .queued_messages_since(&scope.recipient, &policy, &scope, &start, 2)
        .expect("replay after reopen");
    assert_eq!(
        replay, first,
        "ACK does not erase immutable queue observations"
    );
    let old = replay.cursor;
    reopened.rebuild().expect("explicit projection recovery");
    assert!(
        service
            .queued_messages_since(&scope.recipient, &policy, &scope, &old, 2)
            .is_err(),
        "generation change requires a fresh cursor"
    );
}

#[test]
fn recipient_queue_pages_reject_bad_scope_limits_and_revoked_access_before_read() {
    use admiralty_server::services::coordination::{
        CoordinationReadScope, CoordinationService, MAX_COORDINATION_SCAN_EVENTS,
    };
    let directory = private_fixture_dir();
    let store = Store::open(directory.path().join("read-authority.sqlite")).expect("private Store");
    let cursor = store.snapshot().expect("empty cursor").cursor;
    let scope = CoordinationReadScope {
        project_id: ProjectId::new("allowed-project").expect("project"),
        recipient: PrincipalId::new("worker").expect("worker"),
        context_epoch: "epoch-one".into(),
    };
    let policy = ReadPolicy {
        scope: scope.clone(),
    };
    let service = CoordinationService::new(&store);
    let intruder = PrincipalId::new("intruder").expect("intruder");
    let mut invalid_cursor = cursor.clone();
    invalid_cursor.recovery_generation = "not-the-store-generation".into();
    let result = service
        .queued_messages_since(&intruder, &policy, &scope, &invalid_cursor, 1)
        .expect_err("denied before querying a bad cursor");
    assert!(result.to_string().contains("not authorized"));
    for limit in [0, MAX_COORDINATION_SCAN_EVENTS + 1, usize::MAX] {
        assert!(
            service
                .queued_messages_since(&scope.recipient, &policy, &scope, &cursor, limit)
                .is_err()
        );
    }
    for epoch in [String::new(), " ".into(), "e".repeat(16_385)] {
        let mut malformed = scope.clone();
        malformed.context_epoch = epoch;
        assert!(
            service
                .queued_messages_since(&scope.recipient, &policy, &malformed, &cursor, 1)
                .is_err()
        );
    }
    for (generation, sequence) in [
        ("g".repeat(65), "0".into()),
        (String::new(), "0".into()),
        (cursor.recovery_generation.clone(), "9".repeat(20)),
        (cursor.recovery_generation.clone(), "-1".into()),
        (cursor.recovery_generation.clone(), "01".into()),
        (cursor.recovery_generation.clone(), "+1".into()),
        (
            cursor.recovery_generation.clone(),
            "9223372036854775808".into(),
        ),
    ] {
        let malformed = admiralty_store::Cursor {
            recovery_generation: generation,
            event_seq: sequence,
        };
        assert!(
            service
                .queued_messages_since(&scope.recipient, &policy, &scope, &malformed, 1)
                .is_err()
        );
    }
    let page = service
        .queued_messages_since(
            &scope.recipient,
            &policy,
            &scope,
            &cursor,
            MAX_COORDINATION_SCAN_EVENTS,
        )
        .expect("bounded empty page");
    assert_eq!(page.scanned_events, 0);
    // A formerly permitted actor is re-evaluated on the next page.
    let revoked = ReadPolicy {
        scope: CoordinationReadScope {
            context_epoch: "new-epoch".into(),
            ..scope.clone()
        },
    };
    assert!(
        service
            .queued_messages_since(&scope.recipient, &revoked, &scope, &page.cursor, 1)
            .is_err()
    );
    for denied_scope in [
        CoordinationReadScope {
            project_id: ProjectId::new("foreign-project").expect("foreign project"),
            ..scope.clone()
        },
        CoordinationReadScope {
            recipient: intruder,
            ..scope.clone()
        },
        CoordinationReadScope {
            context_epoch: "stale-epoch".into(),
            ..scope.clone()
        },
    ] {
        assert!(
            service
                .queued_messages_since(&scope.recipient, &policy, &denied_scope, &page.cursor, 1)
                .is_err()
        );
    }
    assert_eq!(
        store.snapshot().expect("unchanged after reads").cursor,
        cursor
    );
}

#[test]
fn scoped_envelope_rejects_foreign_project_before_delivery() {
    use admiralty_core::coordination::message::validate_scoped_envelope;

    let task = contract();
    let message = assignment(&task, "foreign-project-message");
    let foreign_project = ProjectId::new("attacker-project").expect("valid fixture project");
    let worker = PrincipalId::new("worker").expect("valid fixture principal");
    let intruder = PrincipalId::new("intruder").expect("valid adversarial principal");

    assert!(validate_scoped_envelope(&message, &foreign_project, Some(&worker)).is_err());
    assert!(validate_scoped_envelope(&message, &task.project_id, Some(&intruder)).is_err());
}

#[test]
fn mailbox_capacity_accepts_limit_and_rejects_first_value_over_it() {
    use admiralty_core::coordination::mailbox::{
        BoundedMailbox, DeliveryOutcome, MAX_MAILBOX_ENTRIES,
    };

    let project = ProjectId::new("capacity-project").expect("valid fixture project");
    let worker = PrincipalId::new("capacity-worker").expect("valid fixture principal");
    assert!(BoundedMailbox::new(project.clone(), worker.clone(), MAX_MAILBOX_ENTRIES).is_ok());
    assert!(BoundedMailbox::new(project, worker, MAX_MAILBOX_ENTRIES + 1).is_err());

    let project = ProjectId::new("sequence-project").expect("valid fixture project");
    let worker = PrincipalId::new("sequence-worker").expect("valid fixture principal");
    let mut mailbox = BoundedMailbox::new(project, worker, 1).expect("valid mailbox capacity");
    assert!(matches!(
        admiralty_server::services::coordination::CoordinationService::route_to_mailbox(
            &mut mailbox,
            0,
            assignment(&contract(), "zero-sequence")
        ),
        DeliveryOutcome::InvalidSequence { seq: 0 }
    ));
}

/// Explicit grants injected by this offline fixture, not request-label auth.
struct PendingPolicy {
    scope: InboxScope,
    read: bool,
    acknowledge: bool,
}

impl CoordinationAuthorizer for PendingPolicy {
    fn authorize(
        &self,
        actor: &PrincipalId,
        action: CoordinationAction,
        scope: &InboxScope,
        message: Option<&MessageId>,
        source: Option<&SourceIdentity>,
    ) -> bool {
        if actor != &self.scope.recipient || scope != &self.scope {
            return false;
        }
        match action {
            CoordinationAction::ReadPending => self.read && message.is_none() && source.is_none(),
            CoordinationAction::AcknowledgeApplication => {
                self.acknowledge
                    && message.is_some()
                    && source.is_some_and(|source| {
                        source.producer_id == "authenticated-worker"
                            && source.incarnation == "actor-session-one"
                    })
            }
        }
    }
}

fn pending_scope(message: &CoordinationMessage) -> InboxScope {
    InboxScope {
        project_id: message.project_id.clone(),
        recipient: message.recipient.clone(),
        context_epoch: message.context_epoch.clone(),
    }
}

fn pending_policy(scope: &InboxScope) -> PendingPolicy {
    PendingPolicy {
        scope: scope.clone(),
        read: true,
        acknowledge: true,
    }
}

fn actor_source(sequence: u64) -> SourceIdentity {
    source("authenticated-worker", "actor-session-one", sequence)
}

fn pending_pass(
    service: &CoordinationService<'_>,
    policy: &PendingPolicy,
) -> Vec<CoordinationMessage> {
    let mut cursor = None;
    let mut messages = Vec::new();
    // Fixture size is bounded. An unexpected endless continuation is a failure.
    for _ in 0..16 {
        let page = service
            .pending_messages(
                &policy.scope.recipient,
                policy,
                policy.scope.clone(),
                cursor,
                1,
            )
            .expect("authorized bounded pending page");
        assert!(page.scanned_messages <= MAX_PENDING_SCAN as u32);
        assert!(page.messages.len() <= 1);
        messages.extend(page.messages);
        cursor = page.next_cursor;
        if cursor.is_none() {
            return messages;
        }
    }
    panic!("bounded fixture pending pass must terminate within sixteen pages");
}

#[test]
fn pending_service_preserves_payload_and_scoped_ack_across_reopen_and_recovery() {
    let directory = private_fixture_dir();
    let database = directory.path().join("pending-service.sqlite");
    let store = Store::open(&database).expect("private pending Store");
    let task = contract();
    let mut foreign_task = task.clone();
    foreign_task.project_id = ProjectId::new("other-project").expect("foreign project");
    foreign_task.task_id = TaskId::new("OTHER-TASK").expect("foreign task");
    foreign_task.authority_ref = "grant:other-task".into();
    for (index, contract) in [task.clone(), foreign_task.clone()].into_iter().enumerate() {
        append(
            &store,
            "pending-fixture",
            "inc",
            index as u64 + 1,
            DomainEvent::TaskSubmitted { contract },
        );
    }
    let service = CoordinationService::new(&store);
    let mine = assignment(&task, "m-assignment");
    let scope = pending_scope(&mine);
    let policy = pending_policy(&scope);
    // More unrelated records than a scan. An empty first page must still
    // advance rather than treating foreign data as an empty recipient inbox.
    for index in 0..=MAX_PENDING_SCAN {
        service
            .enqueue_durable(
                source("pending-fixture", "inc", index as u64 + 3),
                assignment(&foreign_task, &format!("a-foreign-{index:03}")),
                OBSERVED.into(),
            )
            .expect("persist unrelated bounded queue event");
    }
    let mut other_recipient = assignment(&task, "b-other-recipient");
    other_recipient.recipient = PrincipalId::new("another-worker").expect("second recipient");
    let mut other_epoch = assignment(&task, "c-other-epoch");
    other_epoch.context_epoch = "another-context-epoch".into();
    let question = CoordinationMessage {
        message_id: MessageId::new("n-question").expect("question ID"),
        causation_id: Some(mine.message_id.clone()),
        payload: MessagePayload::Question {
            task_id: task.task_id.clone(),
            requirement: "Do not publish or grant acceptance".into(),
            question: "Should the independently verified candidate remain private?\nYes means do not publish.".into(),
            options: vec!["yes, keep private".into(), "request owner review".into()],
        },
        ..mine.clone()
    };
    let next = assignment(&task, "o-next-assignment");
    for (index, message) in [
        other_recipient,
        other_epoch,
        mine.clone(),
        question.clone(),
        next.clone(),
    ]
    .into_iter()
    .enumerate()
    {
        service
            .enqueue_durable(
                source("pending-fixture", "inc", index as u64 + 260),
                message,
                OBSERVED.into(),
            )
            .expect("persist exact-scope or deliberately foreign queue envelope");
    }
    let before_read = store.snapshot().expect("snapshot before pending reads");
    let first = service
        .pending_messages(&scope.recipient, &policy, scope.clone(), None, 1)
        .expect("first bounded sparse page");
    assert!(first.messages.is_empty());
    assert_eq!(first.scanned_messages, MAX_PENDING_SCAN as u32);
    let old_cursor = first.next_cursor.expect("sparse page has continuation");
    let second = service
        .pending_messages(
            &scope.recipient,
            &policy,
            scope.clone(),
            Some(old_cursor.clone()),
            1,
        )
        .expect("seek past unrelated records");
    assert_eq!(second.messages, vec![mine.clone()]);
    assert_eq!(second.scanned_messages, 4);
    // Simulate a lost read response: discard it, start a fresh pass without a
    // cursor, and observe all unacknowledged bytes again. No read executes work.
    drop(second);
    assert_eq!(
        pending_pass(&service, &policy),
        vec![mine.clone(), question.clone(), next.clone()]
    );
    let after_reads = store
        .snapshot()
        .expect("snapshot after lost read and reconnect");
    assert_eq!(after_reads.cursor, before_read.cursor);
    assert_eq!(after_reads.projection, before_read.projection);

    let ack = service
        .acknowledge_application(
            &scope.recipient,
            &policy,
            actor_source(1),
            scope.clone(),
            mine.message_id.clone(),
            OBSERVED.into(),
        )
        .expect("authorized recipient application ACK");
    assert!(!ack.duplicate);
    let after_ack = store.snapshot().expect("snapshot after application ACK");
    let retry = service
        .acknowledge_application(
            &scope.recipient,
            &policy,
            actor_source(1),
            scope.clone(),
            mine.message_id.clone(),
            "2026-10-18T00:01:00Z".into(),
        )
        .expect("exact source retry tolerates a fresh receive timestamp");
    assert!(retry.duplicate);
    assert_eq!(retry.event_seq, ack.event_seq);
    let after_retry = store.snapshot().expect("snapshot after duplicate ACK");
    assert_eq!(after_retry.cursor, after_ack.cursor);
    assert_eq!(after_retry.projection, after_ack.projection);
    assert_eq!(after_ack.projection.tasks, before_read.projection.tasks);
    assert!(
        after_ack.projection.tasks[&task.task_id]
            .candidate_ref
            .is_none()
    );
    assert_eq!(
        pending_pass(&service, &policy),
        vec![question.clone(), next.clone()]
    );
    drop(store);

    let reopened = Store::open(&database).expect("reopen pending/ACK journal");
    let service = CoordinationService::new(&reopened);
    assert_eq!(
        pending_pass(&service, &policy),
        vec![question.clone(), next.clone()]
    );
    let before_recovery = reopened.snapshot().expect("before explicit recovery");
    assert_eq!(before_recovery.projection, after_ack.projection);
    assert_eq!(before_recovery.cursor, after_ack.cursor);
    reopened
        .rebuild()
        .expect("rebuild pending and ACK facts from durable events");
    let stale = service
        .pending_messages(
            &scope.recipient,
            &policy,
            scope.clone(),
            Some(old_cursor),
            1,
        )
        .expect_err("old pending continuation must require a fresh pass");
    assert!(stale.to_string().contains("snapshot_required"));
    assert_eq!(pending_pass(&service, &policy), vec![question, next]);
    let recovered = reopened.snapshot().expect("recovered facts");
    assert_ne!(
        recovered.cursor.recovery_generation,
        before_recovery.cursor.recovery_generation
    );
    assert_eq!(recovered.cursor.event_seq, before_recovery.cursor.event_seq);
    assert_eq!(recovered.projection, before_recovery.projection);
}

#[test]
fn pending_service_ack_requires_action_actor_source_and_exact_scope_even_on_replay() {
    let directory = private_fixture_dir();
    let store = Store::open(directory.path().join("pending-authority.sqlite"))
        .expect("private authorization Store");
    let task = contract();
    append(
        &store,
        "fixture",
        "inc",
        1,
        DomainEvent::TaskSubmitted {
            contract: task.clone(),
        },
    );
    let service = CoordinationService::new(&store);
    let first = assignment(&task, "ack-first");
    let second = assignment(&task, "ack-second");
    for (index, message) in [first.clone(), second.clone()].into_iter().enumerate() {
        service
            .enqueue_durable(
                source("fixture", "inc", index as u64 + 2),
                message,
                OBSERVED.into(),
            )
            .expect("queue scoped ACK fixture");
    }
    let scope = pending_scope(&first);
    let policy = pending_policy(&scope);
    let read_only = PendingPolicy {
        acknowledge: false,
        ..pending_policy(&scope)
    };
    let ack_only = PendingPolicy {
        read: false,
        ..pending_policy(&scope)
    };
    let before = store.snapshot().expect("before permission checks");
    assert_eq!(
        service
            .pending_messages(&scope.recipient, &read_only, scope.clone(), None, 1)
            .expect("read grant only")
            .messages,
        vec![first.clone()]
    );
    unchanged_after_rejection(
        &store,
        &before,
        service
            .acknowledge_application(
                &scope.recipient,
                &read_only,
                actor_source(1),
                scope.clone(),
                first.message_id.clone(),
                OBSERVED.into(),
            )
            .map(|_| ()),
    );
    unchanged_after_rejection(
        &store,
        &before,
        service
            .pending_messages(&scope.recipient, &ack_only, scope.clone(), None, 1)
            .map(|_| ()),
    );
    let intruder = PrincipalId::new("intruder").expect("intruder ID");
    unchanged_after_rejection(
        &store,
        &before,
        service
            .acknowledge_application(
                &intruder,
                &policy,
                actor_source(1),
                scope.clone(),
                first.message_id.clone(),
                OBSERVED.into(),
            )
            .map(|_| ()),
    );
    for bad_source in [
        source("other-producer", "actor-session-one", 1),
        source("authenticated-worker", "expired-session", 1),
    ] {
        unchanged_after_rejection(
            &store,
            &before,
            service
                .acknowledge_application(
                    &scope.recipient,
                    &policy,
                    bad_source,
                    scope.clone(),
                    first.message_id.clone(),
                    OBSERVED.into(),
                )
                .map(|_| ()),
        );
    }
    let denied = PendingPolicy {
        read: false,
        acknowledge: false,
        ..pending_policy(&scope)
    };
    let page = service
        .pending_messages(&scope.recipient, &policy, scope.clone(), None, 1)
        .expect("page before grant revocation");
    unchanged_after_rejection(
        &store,
        &before,
        service
            .pending_messages(
                &scope.recipient,
                &denied,
                scope.clone(),
                page.next_cursor,
                1,
            )
            .map(|_| ()),
    );

    let ack = service
        .acknowledge_application(
            &scope.recipient,
            &policy,
            actor_source(1),
            scope.clone(),
            first.message_id.clone(),
            OBSERVED.into(),
        )
        .expect("valid ACK before adversarial exact-source retries");
    let after_ack = store.snapshot().expect("after valid ACK");
    for foreign_scope in [
        InboxScope {
            project_id: ProjectId::new("foreign-project").expect("foreign project"),
            ..scope.clone()
        },
        InboxScope {
            recipient: intruder,
            ..scope.clone()
        },
        InboxScope {
            context_epoch: "other-epoch".into(),
            ..scope.clone()
        },
    ] {
        // Even a genuine grant for another scope cannot use the old valid
        // source ID to bypass the writer's expected-message scope check.
        let foreign_policy = pending_policy(&foreign_scope);
        unchanged_after_rejection(
            &store,
            &after_ack,
            service
                .acknowledge_application(
                    &foreign_scope.recipient,
                    &foreign_policy,
                    actor_source(1),
                    foreign_scope.clone(),
                    first.message_id.clone(),
                    OBSERVED.into(),
                )
                .map(|_| ()),
        );
        unchanged_after_rejection(
            &store,
            &after_ack,
            service
                .pending_messages(&scope.recipient, &policy, foreign_scope, None, 1)
                .map(|_| ()),
        );
    }
    unchanged_after_rejection(
        &store,
        &after_ack,
        service
            .acknowledge_application(
                &scope.recipient,
                &policy,
                actor_source(2),
                scope.clone(),
                MessageId::new("missing-message").expect("missing ID"),
                OBSERVED.into(),
            )
            .map(|_| ()),
    );
    let collision = service
        .acknowledge_application(
            &scope.recipient,
            &policy,
            actor_source(1),
            scope.clone(),
            second.message_id.clone(),
            OBSERVED.into(),
        )
        .expect_err("same source cannot acknowledge a different message");
    assert!(matches!(collision, StoreError::SourceCollision { .. }));
    let after_collision = store.snapshot().expect("after ACK source collision");
    assert_eq!(after_collision.cursor, after_ack.cursor);
    assert_eq!(after_collision.projection, after_ack.projection);
    let replay = service
        .acknowledge_application(
            &scope.recipient,
            &policy,
            actor_source(1),
            scope.clone(),
            first.message_id,
            OBSERVED.into(),
        )
        .expect("valid replay still resolves after denied attempts");
    assert!(replay.duplicate);
    assert_eq!(replay.event_seq, ack.event_seq);
    assert_eq!(pending_pass(&service, &policy), vec![second]);
}

struct MustNotAuthorize;

impl CoordinationAuthorizer for MustNotAuthorize {
    fn authorize(
        &self,
        _: &PrincipalId,
        _: CoordinationAction,
        _: &InboxScope,
        _: Option<&MessageId>,
        _: Option<&SourceIdentity>,
    ) -> bool {
        panic!("malformed request must be rejected before policy or Store access");
    }
}

#[test]
fn pending_service_admits_bounds_before_policy_and_denies_before_store_generation_read() {
    let directory = private_fixture_dir();
    let store =
        Store::open(directory.path().join("pending-bounds.sqlite")).expect("private bounded Store");
    let service = CoordinationService::new(&store);
    let message = assignment(&contract(), "bounded-ack");
    let scope = pending_scope(&message);
    let before = store.snapshot().expect("empty Store facts");
    for limit in [0, MAX_PENDING_PAGE + 1, usize::MAX] {
        unchanged_after_rejection(
            &store,
            &before,
            service
                .pending_messages(
                    &scope.recipient,
                    &MustNotAuthorize,
                    scope.clone(),
                    None,
                    limit,
                )
                .map(|_| ()),
        );
    }
    for epoch in [
        String::new(),
        " ".into(),
        "bad\nepoch".into(),
        "e".repeat(16_385),
    ] {
        let malformed = InboxScope {
            context_epoch: epoch,
            ..scope.clone()
        };
        unchanged_after_rejection(
            &store,
            &before,
            service
                .pending_messages(
                    &scope.recipient,
                    &MustNotAuthorize,
                    malformed.clone(),
                    None,
                    1,
                )
                .map(|_| ()),
        );
        unchanged_after_rejection(
            &store,
            &before,
            service
                .acknowledge_application(
                    &scope.recipient,
                    &MustNotAuthorize,
                    actor_source(1),
                    malformed,
                    message.message_id.clone(),
                    OBSERVED.into(),
                )
                .map(|_| ()),
        );
    }
    let digest = content_digest(&scope).expect("canonical inbox scope digest");
    for (generation, scope_digest) in [
        (String::new(), digest.clone()),
        ("g".repeat(65), digest.clone()),
        ("g\n".into(), digest.clone()),
        ("unknown-generation".into(), "sha256:abc".into()),
        (
            "unknown-generation".into(),
            format!("sha256:{}", "A".repeat(64)),
        ),
        (
            "unknown-generation".into(),
            format!("sha256:{}", "0".repeat(64)),
        ),
    ] {
        let cursor = InboxCursor {
            recovery_generation: generation,
            scope_digest,
            after_message_id: message.message_id.clone(),
        };
        unchanged_after_rejection(
            &store,
            &before,
            service
                .pending_messages(
                    &scope.recipient,
                    &MustNotAuthorize,
                    scope.clone(),
                    Some(cursor),
                    1,
                )
                .map(|_| ()),
        );
    }
    for malformed_source in [
        source("", "actor-session-one", 1),
        source(&"x".repeat(257), "actor-session-one", 1),
        source("authenticated-worker", "bad\nincarnation", 1),
        SourceIdentity {
            sequence: "01".into(),
            ..actor_source(1)
        },
        SourceIdentity {
            sequence: "9".repeat(20),
            ..actor_source(1)
        },
        SourceIdentity {
            sequence: "9223372036854775808".into(),
            ..actor_source(1)
        },
    ] {
        unchanged_after_rejection(
            &store,
            &before,
            service
                .acknowledge_application(
                    &scope.recipient,
                    &MustNotAuthorize,
                    malformed_source,
                    scope.clone(),
                    message.message_id.clone(),
                    OBSERVED.into(),
                )
                .map(|_| ()),
        );
    }
    for observed in [String::new(), "x".repeat(129), "bad\ntime".into()] {
        unchanged_after_rejection(
            &store,
            &before,
            service
                .acknowledge_application(
                    &scope.recipient,
                    &MustNotAuthorize,
                    actor_source(1),
                    scope.clone(),
                    message.message_id.clone(),
                    observed,
                )
                .map(|_| ()),
        );
    }
    let revoked = PendingPolicy {
        read: false,
        acknowledge: false,
        ..pending_policy(&scope)
    };
    let valid_but_stale = InboxCursor {
        recovery_generation: "not-current-generation".into(),
        scope_digest: digest,
        after_message_id: message.message_id,
    };
    let denied = service
        .pending_messages(
            &scope.recipient,
            &revoked,
            scope.clone(),
            Some(valid_but_stale),
            1,
        )
        .expect_err("grant denial must precede Store generation lookup");
    assert!(denied.to_string().contains("not authorized"));
    let after = store
        .snapshot()
        .expect("unchanged after admission/authority failures");
    assert_eq!(after.cursor, before.cursor);
    assert_eq!(after.projection, before.projection);
}

#[test]
fn retry_drop_fault_fires_on_its_configured_delivery_attempt_only() {
    use admiralty_testkit::faults::{FaultEffect, FaultPlan, FaultRule};

    let mut plan = FaultPlan::new(vec![FaultRule {
        point: "coordination.delivery".into(),
        occurrence: 2,
        effect: FaultEffect::Drop,
    }])
    .expect("valid deterministic delivery fault schedule");

    assert_eq!(plan.hit("coordination.delivery"), Ok(None));
    assert_eq!(
        plan.hit("coordination.delivery"),
        Ok(Some(FaultEffect::Drop))
    );
    assert_eq!(plan.hit("coordination.delivery"), Ok(None));
    assert_eq!(plan.hits("coordination.delivery"), Ok(3));
}
