use admiralty_core::{
    AttemptId, CoordinationMessage, DomainEvent, MessageId, MessagePayload, PrincipalId, ProjectId,
    SourceIdentity, TaskContract, TaskId, content_digest,
};
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
