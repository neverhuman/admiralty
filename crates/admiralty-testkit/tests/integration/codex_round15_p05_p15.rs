use admiralty_core::coordination::questions::{QuestionEvent, QuestionScope, QuestionStatus};
use admiralty_core::{MessageId, PrincipalId, ProjectId, RootId, TaskId};
use admiralty_store::questions::{
    JournalScope, QuestionStoreError, append, initialize, reconstruct,
};
use rusqlite::Connection;

fn identity(prefix: &str, index: usize) -> String {
    format!("{prefix}-{index:02}")
}

fn msg(value: String) -> MessageId {
    MessageId::new(value).expect("deterministic bounded message identity")
}

#[test]
fn codex_round15_p05_p15_multi_step_release_smoke_and_escalation_replay() {
    const QUESTIONS: usize = 12;
    let directory = tempfile::tempdir().expect("private stress journal directory");
    let database = directory.path().join("release-smoke.sqlite3");
    let mut connection = Connection::open(&database).expect("open SQLite question journal");
    initialize(&connection).expect("initialize question and escalation inbox tables");

    let project = ProjectId::new("round15-project").expect("valid project scope");
    let root = RootId::new("round15-root").expect("valid root scope");
    let task = TaskId::new("round15-release-task").expect("valid task scope");
    let authority = "authority:round15-release";
    let worker = PrincipalId::new("worker").expect("valid worker identity");
    let reviewer = PrincipalId::new("reviewer").expect("valid reviewer identity");
    let release_owner = PrincipalId::new("release-owner").expect("valid release identity");
    let mut expected_escalations = Vec::with_capacity(QUESTIONS);

    for index in 0..QUESTIONS {
        let question = QuestionScope {
            question_id: msg(identity("round15-question", index)),
            project_id: project.clone(),
            task_id: task.clone(),
        };
        let scope = JournalScope {
            root_id: root.clone(),
            question: question.clone(),
            authority_ref: authority.into(),
        };
        let deadline_utc = 1_000 + index as u64;
        let opening = QuestionEvent::Open {
            event_id: msg(identity("round15-open-event", index)),
            scope: question.clone(),
            authority_ref: authority.into(),
            asker: PrincipalId::new("captain").expect("valid asker identity"),
            recipient: worker.clone(),
            allowed_recipients: vec![worker.clone(), reviewer.clone(), release_owner.clone()],
            requirement: format!("Review release candidate {index}"),
            question: "Are pinned checks and release evidence independently verified?".into(),
            options: vec!["hold".into(), "release-review".into()],
            deadline_utc: Some(deadline_utc),
        };
        let first_open = append(
            &mut connection,
            &scope,
            &msg(identity("round15-open-command", index)),
            &opening,
        )
        .expect("append deterministic question opening");
        let open_replay = append(
            &mut connection,
            &scope,
            &msg(identity("round15-open-retry", index)),
            &opening,
        )
        .expect("exact opening replay returns retained receipt");
        assert!(open_replay.duplicate);
        assert_eq!(open_replay.event_seq, first_open.event_seq);
        assert_eq!(open_replay.revision, first_open.revision);

        let denied = QuestionEvent::Route {
            event_id: msg(identity("round15-forged-route-event", index)),
            scope: question.clone(),
            expected_revision: 1,
            authority_ref: authority.into(),
            actor: PrincipalId::new("intruder").expect("valid adversarial actor"),
            recipient: reviewer.clone(),
        };
        assert!(matches!(
            append(
                &mut connection,
                &scope,
                &msg(identity("round15-forged-route-command", index)),
                &denied,
            ),
            Err(QuestionStoreError::Reducer(_))
        ));

        let first_route = QuestionEvent::Route {
            event_id: msg(identity("round15-route-one-event", index)),
            scope: question.clone(),
            expected_revision: 1,
            authority_ref: authority.into(),
            actor: worker.clone(),
            recipient: reviewer.clone(),
        };
        let routed = append(
            &mut connection,
            &scope,
            &msg(identity("round15-route-one-command", index)),
            &first_route,
        )
        .expect("current worker routes to allowlisted reviewer");
        let route_replay = append(
            &mut connection,
            &scope,
            &msg(identity("round15-route-one-replay", index)),
            &first_route,
        )
        .expect("route replay is idempotent");
        assert!(route_replay.duplicate);
        assert_eq!(route_replay.revision, routed.revision);

        let second_route = QuestionEvent::Route {
            event_id: msg(identity("round15-route-two-event", index)),
            scope: question.clone(),
            expected_revision: 2,
            authority_ref: authority.into(),
            actor: reviewer.clone(),
            recipient: release_owner.clone(),
        };
        let final_routed = append(
            &mut connection,
            &scope,
            &msg(identity("round15-route-two-command", index)),
            &second_route,
        )
        .expect("reviewer routes to the explicitly allowlisted release owner");
        assert_eq!(final_routed.revision, "3");

        let before_deadline = QuestionEvent::EvaluateDeadline {
            event_id: msg(identity("round15-premature-deadline-event", index)),
            scope: question.clone(),
            expected_revision: 3,
            authority_ref: authority.into(),
            actor: release_owner.clone(),
            observed_utc: Some(deadline_utc),
        };
        assert!(matches!(
            append(
                &mut connection,
                &scope,
                &msg(identity("round15-premature-deadline-command", index)),
                &before_deadline,
            ),
            Err(QuestionStoreError::Reducer(_))
        ));

        let expiration = QuestionEvent::EvaluateDeadline {
            event_id: msg(identity("round15-expiration-event", index)),
            scope: question.clone(),
            expected_revision: 3,
            authority_ref: authority.into(),
            actor: release_owner.clone(),
            observed_utc: Some(deadline_utc + 1),
        };
        let expired = append(
            &mut connection,
            &scope,
            &msg(identity("round15-expiration-command", index)),
            &expiration,
        )
        .expect("strictly expired question yields an escalation inbox proposal");
        let escalation_ref = expired
            .escalation_ref
            .clone()
            .expect("expiry receipt includes escalation identity");
        let replayed_expiration = append(
            &mut connection,
            &scope,
            &msg(identity("round15-expiration-replay", index)),
            &expiration,
        )
        .expect("exact expiry replay recovers original escalation reference");
        assert!(replayed_expiration.duplicate);
        assert_eq!(
            replayed_expiration.escalation_ref.as_deref(),
            Some(escalation_ref.as_str())
        );
        expected_escalations.push((scope, escalation_ref));
    }

    let escalation_count: i64 = connection
        .query_row(
            "SELECT count(*) FROM durable_question_escalations",
            [],
            |row| row.get(0),
        )
        .expect("read escalation inbox count");
    assert_eq!(escalation_count, QUESTIONS as i64);
    let unique_escalations: i64 = connection
        .query_row(
            "SELECT count(DISTINCT escalation_ref) FROM durable_question_escalations",
            [],
            |row| row.get(0),
        )
        .expect("read unique escalation identities");
    assert_eq!(unique_escalations, QUESTIONS as i64);
    drop(connection);

    let reopened = Connection::open(&database).expect("reopen journal after stress sequence");
    for (index, (scope, escalation_ref)) in expected_escalations.iter().enumerate() {
        let recovered = reconstruct(&reopened, scope)
            .expect("replay validates immutable event chain")
            .expect("question remains queryable in durable inbox history");
        assert_eq!(recovered.event_count, "4");
        assert_eq!(recovered.state.revision, 4);
        assert_eq!(
            recovered.state.status,
            QuestionStatus::Expired {
                observed_utc: 1_001 + index as u64
            }
        );
        assert_eq!(
            recovered.escalation_ref.as_deref(),
            Some(escalation_ref.as_str())
        );

        let proposal_json: String = reopened
            .query_row(
                "SELECT proposal_json FROM durable_question_escalations WHERE escalation_ref=?1",
                [escalation_ref],
                |row| row.get(0),
            )
            .expect("read immutable escalation proposal");
        let proposal: serde_json::Value =
            serde_json::from_str(&proposal_json).expect("proposal is bounded JSON");
        assert_eq!(proposal["project_id"], "round15-project");
        assert_eq!(proposal["task_id"], "round15-release-task");
        assert_eq!(proposal["question_id"], scope.question.question_id.as_str());
        assert!(proposal.get("acceptance_granted").is_none());
        assert!(proposal.get("release_granted").is_none());
    }
}
