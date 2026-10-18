use admiralty_core::coordination::questions::{
    QuestionEvent, QuestionScope, QuestionStatus, reduce_question,
};
use admiralty_core::{MessageId, PrincipalId, ProjectId, TaskId};

fn scope() -> QuestionScope {
    QuestionScope {
        question_id: MessageId::new("q-1").expect("valid test value"),
        project_id: ProjectId::new("project-1").expect("valid test value"),
        task_id: TaskId::new("task-1").expect("valid test value"),
    }
}

#[test]
fn question_delivery_replay_and_answer_are_idempotent_and_scoped() {
    let scope = scope();
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
