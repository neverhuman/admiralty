use admiralty_core::*;
use admiralty_testkit::task_fixture;
use serde_json::json;

fn submitted(contract: TaskContract) -> Projection {
    reduce(
        &Projection::default(),
        &DomainEvent::TaskSubmitted { contract },
    )
    .expect("valid fixture submission")
}

fn claim(task: &TaskContract, attempt: &str, owner: &str) -> DomainEvent {
    DomainEvent::TaskClaimed {
        task_id: task.task_id.clone(),
        revision: task.revision,
        attempt_id: AttemptId::new(attempt).expect("fixture attempt"),
        owner: PrincipalId::new(owner).expect("fixture principal"),
    }
}

fn message(task: &TaskContract) -> CoordinationMessage {
    CoordinationMessage {
        schema_version: 1,
        message_id: MessageId::new("message-one").expect("fixture message"),
        project_id: task.project_id.clone(),
        sender: PrincipalId::new("captain-one").expect("fixture sender"),
        recipient: PrincipalId::new("worker-one").expect("fixture recipient"),
        causation_id: None,
        context_epoch: "epoch-one".into(),
        authority_ref: task.authority_ref.clone(),
        payload: MessagePayload::TaskAssigned {
            task_id: task.task_id.clone(),
            revision: task.revision,
            contract_digest: content_digest(task).expect("fixture digest"),
        },
    }
}

fn rejects_without_mutating(state: &Projection, event: DomainEvent) {
    let before = state.clone();
    assert!(
        reduce(state, &event).is_err(),
        "event must be rejected: {event:?}"
    );
    assert_eq!(
        *state, before,
        "rejection cannot partly mutate input projection"
    );
}

#[test]
fn duplicate_task_identity_cannot_replace_scope_or_revision() {
    let contract = task_fixture("TASK-DUPLICATE");
    let state = submitted(contract.clone());
    rejects_without_mutating(
        &state,
        DomainEvent::TaskSubmitted {
            contract: contract.clone(),
        },
    );
    let mut revision = contract;
    revision.revision += 1;
    revision.owned_paths = vec!["src/different.rs".into()];
    rejects_without_mutating(&state, DomainEvent::TaskSubmitted { contract: revision });
}

#[test]
fn stale_claim_and_future_revision_claim_leave_task_ready() {
    let contract = task_fixture("TASK-REVISION");
    let state = submitted(contract.clone());
    for revision in [0, contract.revision + 1] {
        let event = DomainEvent::TaskClaimed {
            task_id: contract.task_id.clone(),
            revision,
            attempt_id: AttemptId::new("attempt-stale").expect("fixture ID"),
            owner: PrincipalId::new("worker-stale").expect("fixture ID"),
        };
        rejects_without_mutating(&state, event);
    }
    assert_eq!(state.tasks[&contract.task_id].state, SchedulingState::Ready);
}

#[test]
fn a_second_owner_cannot_claim_an_unresolved_attempt() {
    let contract = task_fixture("TASK-CLAIM");
    let state = reduce(
        &submitted(contract.clone()),
        &claim(&contract, "attempt-one", "worker-one"),
    )
    .expect("first claim");
    rejects_without_mutating(&state, claim(&contract, "attempt-two", "worker-two"));
    assert_eq!(
        state.tasks[&contract.task_id]
            .owner
            .as_ref()
            .map(PrincipalId::as_str),
        Some("worker-one")
    );
    assert_eq!(
        state.tasks[&contract.task_id]
            .attempt_id
            .as_ref()
            .map(AttemptId::as_str),
        Some("attempt-one")
    );
}

#[test]
fn only_current_claim_can_start_and_only_running_attempt_can_submit() {
    let contract = task_fixture("TASK-ATTEMPT");
    let state = submitted(contract.clone());
    let one = AttemptId::new("attempt-one").expect("fixture ID");
    let other = AttemptId::new("attempt-other").expect("fixture ID");
    rejects_without_mutating(
        &state,
        DomainEvent::TaskStarted {
            task_id: contract.task_id.clone(),
            attempt_id: one.clone(),
        },
    );
    let claimed = reduce(&state, &claim(&contract, "attempt-one", "worker-one")).expect("claim");
    rejects_without_mutating(
        &claimed,
        DomainEvent::TaskStarted {
            task_id: contract.task_id.clone(),
            attempt_id: other.clone(),
        },
    );
    rejects_without_mutating(
        &claimed,
        DomainEvent::CandidateSubmitted {
            task_id: contract.task_id.clone(),
            attempt_id: one.clone(),
            candidate_ref: "sha256:fixture".into(),
        },
    );
    let running = reduce(
        &claimed,
        &DomainEvent::TaskStarted {
            task_id: contract.task_id.clone(),
            attempt_id: one.clone(),
        },
    )
    .expect("current attempt start");
    rejects_without_mutating(
        &running,
        DomainEvent::CandidateSubmitted {
            task_id: contract.task_id.clone(),
            attempt_id: other,
            candidate_ref: "sha256:fixture".into(),
        },
    );
    rejects_without_mutating(
        &running,
        DomainEvent::CandidateSubmitted {
            task_id: contract.task_id.clone(),
            attempt_id: one.clone(),
            candidate_ref: String::new(),
        },
    );
    let candidate = reduce(
        &running,
        &DomainEvent::CandidateSubmitted {
            task_id: contract.task_id.clone(),
            attempt_id: one,
            candidate_ref: "sha256:fixture".into(),
        },
    )
    .expect("current running candidate");
    assert_eq!(
        candidate.tasks[&contract.task_id].state,
        SchedulingState::AwaitingVerification
    );
    assert_eq!(
        running.tasks[&contract.task_id].state,
        SchedulingState::Running
    );
}

#[test]
fn missing_dependency_is_rejected_and_existing_dependency_blocks_claim() {
    let upstream = task_fixture("TASK-UPSTREAM");
    let mut dependent = task_fixture("TASK-DEPENDENT");
    dependent.dependencies = vec![upstream.task_id.clone()];
    rejects_without_mutating(
        &Projection::default(),
        DomainEvent::TaskSubmitted {
            contract: dependent.clone(),
        },
    );
    let state = reduce(
        &submitted(upstream),
        &DomainEvent::TaskSubmitted {
            contract: dependent.clone(),
        },
    )
    .expect("known dependency");
    assert_eq!(
        state.tasks[&dependent.task_id].state,
        SchedulingState::Blocked
    );
    rejects_without_mutating(
        &state,
        claim(&dependent, "attempt-dependent", "worker-dependent"),
    );
}

#[test]
fn cross_project_dependency_requires_an_explicit_scoped_contract() {
    let upstream = task_fixture("TASK-FOREIGN-UPSTREAM");
    let mut dependent = task_fixture("TASK-FOREIGN-DEPENDENT");
    dependent.project_id = ProjectId::new("unrelated-project").expect("fixture project");
    dependent.dependencies = vec![upstream.task_id.clone()];
    rejects_without_mutating(
        &submitted(upstream),
        DomainEvent::TaskSubmitted {
            contract: dependent,
        },
    );
}

#[test]
fn duplicate_or_self_dependencies_cannot_enter_projection() {
    let upstream = task_fixture("TASK-DEPS-ROOT");
    let state = submitted(upstream.clone());
    let mut dependent = task_fixture("TASK-DEPS-INVALID");
    dependent.dependencies = vec![upstream.task_id.clone(), upstream.task_id];
    rejects_without_mutating(
        &state,
        DomainEvent::TaskSubmitted {
            contract: dependent.clone(),
        },
    );
    dependent.dependencies = vec![dependent.task_id.clone()];
    rejects_without_mutating(
        &state,
        DomainEvent::TaskSubmitted {
            contract: dependent,
        },
    );
}

#[test]
fn message_ack_requires_exact_recipient_and_current_context_epoch() {
    let contract = task_fixture("TASK-MESSAGE");
    let message = message(&contract);
    let state = reduce(
        &submitted(contract),
        &DomainEvent::MessageQueued {
            message: message.clone(),
        },
    )
    .expect("queue valid message");
    for (recipient, epoch) in [("worker-other", "epoch-one"), ("worker-one", "epoch-stale")] {
        rejects_without_mutating(
            &state,
            DomainEvent::MessageAcknowledged {
                message_id: message.message_id.clone(),
                recipient: PrincipalId::new(recipient).expect("fixture recipient"),
                context_epoch: epoch.into(),
            },
        );
    }
    assert!(state.acknowledgments.is_empty());
    let acknowledged = reduce(
        &state,
        &DomainEvent::MessageAcknowledged {
            message_id: message.message_id.clone(),
            recipient: message.recipient.clone(),
            context_epoch: message.context_epoch.clone(),
        },
    )
    .expect("matching acknowledgment");
    assert_eq!(
        acknowledged.acknowledgments[&message.message_id],
        "epoch-one"
    );
    rejects_without_mutating(&state, DomainEvent::MessageQueued { message });
}

#[test]
fn cross_project_message_cannot_route_another_projects_task() {
    let contract = task_fixture("TASK-PRIVATE-PROJECT");
    let mut message = message(&contract);
    message.project_id = ProjectId::new("unrelated-project").expect("fixture project");
    rejects_without_mutating(&submitted(contract), DomainEvent::MessageQueued { message });
}

#[test]
fn message_cannot_assign_a_task_that_does_not_exist() {
    let contract = task_fixture("TASK-NOT-SUBMITTED");
    rejects_without_mutating(
        &Projection::default(),
        DomainEvent::MessageQueued {
            message: message(&contract),
        },
    );
}

#[test]
fn assignment_message_must_pin_current_contract_revision_and_digest() {
    let contract = task_fixture("TASK-ASSIGNMENT-REVISION");
    let state = submitted(contract.clone());
    let mut stale_revision = message(&contract);
    if let MessagePayload::TaskAssigned { revision, .. } = &mut stale_revision.payload {
        *revision += 1;
    }
    rejects_without_mutating(
        &state,
        DomainEvent::MessageQueued {
            message: stale_revision,
        },
    );
    let mut stale_digest = message(&contract);
    if let MessagePayload::TaskAssigned {
        contract_digest, ..
    } = &mut stale_digest.payload
    {
        *contract_digest = "sha256:wrong-contract".into();
    }
    rejects_without_mutating(
        &state,
        DomainEvent::MessageQueued {
            message: stale_digest,
        },
    );
}

#[test]
fn answer_requires_existing_question_and_matching_project_participants() {
    let contract = task_fixture("TASK-QUESTION-ROUTING");
    let mut question = message(&contract);
    question.payload = MessagePayload::Question {
        task_id: contract.task_id.clone(),
        requirement: "C1".into(),
        question: "Which exact interface revision applies?".into(),
        options: vec!["v1".into(), "v2".into()],
    };
    let mut answer = question.clone();
    answer.message_id = MessageId::new("answer-one").expect("fixture message");
    answer.sender = question.recipient.clone();
    answer.recipient = question.sender.clone();
    answer.causation_id = Some(question.message_id.clone());
    answer.payload = MessagePayload::Answer {
        question_id: question.message_id.clone(),
        answer: "v1".into(),
        decision_ref: Some("decision-v1".into()),
    };
    let initial = submitted(contract);
    rejects_without_mutating(
        &initial,
        DomainEvent::MessageQueued {
            message: answer.clone(),
        },
    );
    let queued = reduce(&initial, &DomainEvent::MessageQueued { message: question })
        .expect("scoped question");
    let mut wrong_project = answer.clone();
    wrong_project.project_id = ProjectId::new("unrelated-project").expect("fixture project");
    rejects_without_mutating(
        &queued,
        DomainEvent::MessageQueued {
            message: wrong_project,
        },
    );
    let mut wrong_participant = answer.clone();
    wrong_participant.sender = PrincipalId::new("unrelated-worker").expect("fixture sender");
    rejects_without_mutating(
        &queued,
        DomainEvent::MessageQueued {
            message: wrong_participant,
        },
    );
    assert!(reduce(&queued, &DomainEvent::MessageQueued { message: answer }).is_ok());
}

#[test]
fn unknown_acceptance_events_and_self_report_fields_are_deserialization_errors() {
    for kind in [
        "task_accepted",
        "acceptance_claimed",
        "self_report_pass",
        "verification_passed",
    ] {
        let value = json!({"kind":kind,"task_id":"TASK-SELF-REPORT","status":"PASS"});
        assert!(
            serde_json::from_value::<DomainEvent>(value).is_err(),
            "{kind} must not create trusted acceptance"
        );
    }
    let contract = task_fixture("TASK-UNKNOWN-FIELD");
    let mut value =
        serde_json::to_value(DomainEvent::TaskSubmitted { contract }).expect("serialize fixture");
    value["accepted"] = json!(true);
    assert!(serde_json::from_value::<DomainEvent>(value).is_err());
    let mut contract =
        serde_json::to_value(task_fixture("TASK-SELF-REPORT")).expect("serialize contract");
    contract["verified"] = json!(true);
    assert!(serde_json::from_value::<TaskContract>(contract).is_err());
}

#[test]
fn source_sequence_stays_exact_and_rejects_noncanonical_values() {
    let identity = |sequence: &str| SourceIdentity {
        producer_id: "fixture-producer".into(),
        incarnation: "boot-one".into(),
        sequence: sequence.into(),
    };
    assert_eq!(
        identity("18446744073709551615")
            .validate()
            .expect("maximum u64"),
        u64::MAX
    );
    for sequence in ["0", "01", "+1", "1.0", " 1", "18446744073709551616"] {
        assert!(
            identity(sequence).validate().is_err(),
            "sequence {sequence} must fail"
        );
    }
    assert_eq!(
        serde_json::to_value(identity("9007199254740993")).expect("serialize identity")["sequence"],
        "9007199254740993"
    );
}
