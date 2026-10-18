use admiralty_core::coordination::questions::{QuestionEvent, QuestionScope, QuestionStatus};
use admiralty_core::{MessageId, PrincipalId, ProjectId, RootId, TaskId};
use admiralty_store::questions::{
    JournalScope, QuestionStoreError, append, initialize, reconstruct,
};
use rusqlite::Connection;

#[test]
fn codex_round14_p05_p15() {
    let directory = tempfile::tempdir().expect("private deterministic inbox fixture");
    let database = directory.path().join("questions.sqlite3");
    let mut connection = Connection::open(&database).expect("open fixture journal");
    initialize(&connection).expect("initialize question and escalation journals");

    let question_scope = QuestionScope {
        question_id: MessageId::new("round14-review").expect("valid question ID"),
        project_id: ProjectId::new("project-round14").expect("valid project ID"),
        task_id: TaskId::new("task-round14").expect("valid task ID"),
    };
    let scope = JournalScope {
        root_id: RootId::new("root-round14").expect("valid root ID"),
        question: question_scope.clone(),
        authority_ref: "authority:round14".into(),
    };
    let opening = QuestionEvent::Open {
        event_id: MessageId::new("event-open").expect("valid event ID"),
        scope: question_scope.clone(),
        authority_ref: scope.authority_ref.clone(),
        asker: PrincipalId::new("captain").expect("valid asker ID"),
        recipient: PrincipalId::new("worker").expect("valid recipient ID"),
        allowed_recipients: vec![
            PrincipalId::new("worker").expect("valid worker ID"),
            PrincipalId::new("reviewer").expect("valid reviewer ID"),
        ],
        requirement: "Independently review release evidence".into(),
        question: "Does the candidate satisfy the pinned checks?".into(),
        options: vec!["hold".into(), "review".into()],
        deadline_utc: Some(100),
    };
    let first_open = append(
        &mut connection,
        &scope,
        &MessageId::new("command-open").expect("valid command ID"),
        &opening,
    )
    .expect("record opening");
    let open_replay = append(
        &mut connection,
        &scope,
        &MessageId::new("command-open-replay").expect("valid replay command ID"),
        &opening,
    )
    .expect("replaying original event returns retained receipt");
    assert!(open_replay.duplicate);
    assert_eq!(open_replay.event_seq, first_open.event_seq);
    assert_eq!(open_replay.revision, first_open.revision);

    let unauthorized_route = QuestionEvent::Route {
        event_id: MessageId::new("event-unauthorized-route").expect("valid event ID"),
        scope: question_scope.clone(),
        expected_revision: 1,
        authority_ref: scope.authority_ref.clone(),
        actor: PrincipalId::new("intruder").expect("valid actor ID"),
        recipient: PrincipalId::new("reviewer").expect("valid reviewer ID"),
    };
    assert!(matches!(
        append(
            &mut connection,
            &scope,
            &MessageId::new("command-unauthorized-route").expect("valid command ID"),
            &unauthorized_route,
        ),
        Err(QuestionStoreError::Reducer(_))
    ));

    let route = QuestionEvent::Route {
        event_id: MessageId::new("event-route").expect("valid route event ID"),
        scope: question_scope.clone(),
        expected_revision: 1,
        authority_ref: scope.authority_ref.clone(),
        actor: PrincipalId::new("worker").expect("valid current recipient ID"),
        recipient: PrincipalId::new("reviewer").expect("valid allowlisted reviewer ID"),
    };
    let routed = append(
        &mut connection,
        &scope,
        &MessageId::new("command-route").expect("valid route command ID"),
        &route,
    )
    .expect("route to allowlisted reviewer");
    let route_replay = append(
        &mut connection,
        &scope,
        &MessageId::new("command-route-replay").expect("valid route replay ID"),
        &route,
    )
    .expect("exact route replay is idempotent");
    assert!(route_replay.duplicate);
    assert_eq!(route_replay.revision, routed.revision);

    let mut foreign_scope = scope.clone();
    foreign_scope.question.task_id = TaskId::new("foreign-task").expect("valid foreign task");
    assert!(matches!(
        append(
            &mut connection,
            &foreign_scope,
            &MessageId::new("command-foreign-scope").expect("valid command ID"),
            &route,
        ),
        Err(QuestionStoreError::ScopeMismatch)
    ));

    let deadline = QuestionEvent::EvaluateDeadline {
        event_id: MessageId::new("event-deadline").expect("valid deadline event ID"),
        scope: question_scope.clone(),
        expected_revision: 2,
        authority_ref: scope.authority_ref.clone(),
        actor: PrincipalId::new("reviewer").expect("current reviewer ID"),
        observed_utc: Some(101),
    };
    let expired = append(
        &mut connection,
        &scope,
        &MessageId::new("command-deadline").expect("valid deadline command ID"),
        &deadline,
    )
    .expect("strictly expired deadline creates one escalation proposal");
    let escalation_ref = expired
        .escalation_ref
        .clone()
        .expect("expiry receipt binds escalation proposal");
    let deadline_replay = append(
        &mut connection,
        &scope,
        &MessageId::new("command-deadline-replay").expect("valid replay command ID"),
        &deadline,
    )
    .expect("deadline event replay returns deterministic escalation identity");
    assert!(deadline_replay.duplicate);
    assert_eq!(
        deadline_replay.escalation_ref.as_deref(),
        Some(escalation_ref.as_str())
    );

    let late_answer = QuestionEvent::Answer {
        event_id: MessageId::new("event-late-answer").expect("valid answer event ID"),
        scope: question_scope,
        expected_revision: 3,
        authority_ref: scope.authority_ref.clone(),
        actor: PrincipalId::new("reviewer").expect("valid current reviewer ID"),
        answer: "release approved".into(),
    };
    assert!(matches!(
        append(
            &mut connection,
            &scope,
            &MessageId::new("command-late-answer").expect("valid command ID"),
            &late_answer,
        ),
        Err(QuestionStoreError::Reducer(_))
    ));

    let inbox_count: i64 = connection
        .query_row(
            "SELECT count(*) FROM durable_question_escalations",
            [],
            |row| row.get(0),
        )
        .expect("read escalation inbox size");
    assert_eq!(
        inbox_count, 1,
        "replay cannot duplicate escalation inbox entries"
    );
    drop(connection);

    let reopened = Connection::open(&database).expect("reopen durable journal");
    let recovered = reconstruct(&reopened, &scope)
        .expect("integrity-checked deterministic replay")
        .expect("question remains in escalation inbox history");
    assert_eq!(recovered.state.revision, 3);
    assert_eq!(
        recovered.state.status,
        QuestionStatus::Expired { observed_utc: 101 }
    );
    assert_eq!(
        recovered.escalation_ref.as_deref(),
        Some(escalation_ref.as_str())
    );
    assert_eq!(recovered.event_count, "3");
}
