use admiralty_core::{
    AcceptanceCriterion, AttemptId, CheckSpec, CoordinationMessage, DomainEvent, MessageId,
    MessagePayload, PrincipalId, ProjectId, Projection, RootId, SchedulingState, TaskContract,
    TaskId, compile_task_capsule, content_digest, reduce,
};

fn contract() -> TaskContract {
    TaskContract {
        schema_version: 1,
        project_id: ProjectId::new("project.alpha").expect("fixture project ID"),
        root_id: RootId::new("root.alpha").expect("fixture root ID"),
        task_id: TaskId::new("task.alpha").expect("fixture task ID"),
        revision: 7,
        title: "Preserve exact protected facts".into(),
        objective: "Do not publish; retain the 5 ms limit and exact argv.".into(),
        non_goals: vec!["Do not modify forbidden/private paths".into()],
        owned_paths: vec!["crates/admiralty-core/src/context.rs".into()],
        forbidden_paths: vec!["secrets/".into(), "private/".into()],
        read_first: vec!["docs/protocol/context-mailbox-fixtures.md".into()],
        dependencies: vec![],
        criteria: vec![AcceptanceCriterion {
            id: "criterion.scope".into(),
            text: "Preserve the negative constraint and 5 ms threshold.".into(),
            check_ids: vec!["context_exact".into()],
        }],
        checks: vec![CheckSpec {
            id: "context_exact".into(),
            program: "cargo".into(),
            args: vec![
                "test".into(),
                "-p".into(),
                "admiralty-core".into(),
                "--test".into(),
                "context_mailbox_fixtures".into(),
                "--".into(),
                "--exact".into(),
            ],
            cwd: ".".into(),
            timeout_secs: 30,
        }],
        authority_ref: "grant:project.alpha/task.alpha/revision.7".into(),
        max_attempts: 2,
        simulated: true,
    }
}

fn submitted(contract: &TaskContract) -> Projection {
    reduce(
        &Projection::default(),
        &DomainEvent::TaskSubmitted {
            contract: contract.clone(),
        },
    )
    .expect("submit fixture task")
}

fn claimed(contract: &TaskContract) -> Projection {
    let state = submitted(contract);
    reduce(
        &state,
        &DomainEvent::TaskClaimed {
            task_id: contract.task_id.clone(),
            revision: contract.revision,
            attempt_id: AttemptId::new("attempt.alpha").expect("fixture attempt ID"),
            owner: PrincipalId::new("worker.alpha").expect("fixture owner ID"),
        },
    )
    .expect("claim fixture task")
}

fn assignment(
    contract: &TaskContract,
    id: &str,
    recipient: &str,
    epoch: &str,
) -> CoordinationMessage {
    CoordinationMessage {
        schema_version: 1,
        message_id: MessageId::new(id).expect("fixture message ID"),
        project_id: contract.project_id.clone(),
        sender: PrincipalId::new("worker.alpha").expect("fixture sender ID"),
        recipient: PrincipalId::new(recipient).expect("fixture recipient ID"),
        causation_id: None,
        context_epoch: epoch.into(),
        authority_ref: contract.authority_ref.clone(),
        payload: MessagePayload::TaskAssigned {
            task_id: contract.task_id.clone(),
            revision: contract.revision,
            contract_digest: content_digest(contract).expect("fixture contract digest"),
        },
    }
}

#[test]
fn submitted_claimed_message_and_capsule_keep_exact_scope_and_authority() {
    let value = contract();
    let submitted = submitted(&value);
    assert_eq!(
        submitted.tasks[&value.task_id].state,
        SchedulingState::Ready,
        "a submitted dependency-free fixture starts ready"
    );

    let state = claimed(&value);
    let task = &state.tasks[&value.task_id];
    assert_eq!(task.contract.project_id, value.project_id);
    assert_eq!(task.contract.root_id, value.root_id);
    assert_eq!(task.contract.task_id, value.task_id);
    assert_eq!(task.contract.revision, 7);
    assert_eq!(
        content_digest(&task.contract).expect("stored contract digest"),
        content_digest(&value).expect("fixture contract digest")
    );
    assert_eq!(
        task.owner.as_ref().expect("claim owner").as_str(),
        "worker.alpha"
    );

    let capsule = compile_task_capsule(&value, "epoch.alpha.1", 2048)
        .expect("complete fixture capsule fits declared budget");
    assert_eq!(capsule.task_id, "task.alpha");
    assert_eq!(capsule.revision, 7);
    assert_eq!(capsule.context_epoch, "epoch.alpha.1");
    assert_eq!(
        capsule.contract_digest,
        content_digest(&value).expect("contract digest")
    );
    for exact_fact in [
        "project=\"project.alpha\"",
        "root=\"root.alpha\"",
        "task=task.alpha revision=7 epoch=\"epoch.alpha.1\"",
        "grant:project.alpha/task.alpha/revision.7",
        "Do not publish",
        "5 ms",
        "secrets/",
        "private/",
        "criterion.scope",
        "--exact",
    ] {
        assert!(
            capsule.text.contains(exact_fact),
            "capsule lost protected fact {exact_fact:?}"
        );
    }

    let message = assignment(&value, "assignment.alpha", "captain.alpha", "epoch.alpha.1");
    let queued = reduce(
        &state,
        &DomainEvent::MessageQueued {
            message: message.clone(),
        },
    )
    .expect("queue task assignment using existing task scope");
    let stored = &queued.messages[&message.message_id];
    assert_eq!(stored.project_id, value.project_id);
    assert_eq!(stored.recipient.as_str(), "captain.alpha");
    assert_eq!(stored.context_epoch, "epoch.alpha.1");
    assert_eq!(stored.authority_ref, value.authority_ref);
    assert!(matches!(
        &stored.payload,
        MessagePayload::TaskAssigned { task_id, revision, contract_digest }
            if task_id == &value.task_id
                && *revision == value.revision
                && contract_digest == &content_digest(&value).expect("assignment digest")
    ));
}

#[test]
fn protected_facts_fit_or_compilation_refuses_without_silent_truncation() {
    let value = contract();
    let capsule = compile_task_capsule(&value, "epoch.budget", 2048)
        .expect("complete essentials fit the declared budget");
    assert!(capsule.estimated_tokens <= 2048);
    assert!(
        capsule
            .text
            .ends_with("do not publish or claim acceptance\n")
    );
    assert!(
        compile_task_capsule(&value, "epoch.too-small", 1).is_err(),
        "compiler must refuse when the whole essential capsule cannot fit"
    );

    let mut escaped_value = value.clone();
    escaped_value.objective = format!("{}required objective", "\n".repeat(100));
    let raw_growth = escaped_value.objective.len() - value.objective.len();
    let escaped_growth = serde_json::to_string(&escaped_value.objective)
        .expect("escaped objective fixture")
        .len()
        - serde_json::to_string(&value.objective)
            .expect("base objective fixture")
            .len();
    assert!(
        raw_growth < escaped_growth,
        "fixture distinguishes raw from escaped size"
    );
    let byte_budget = capsule.text.len() + raw_growth + 8;
    let token_budget = byte_budget.div_ceil(4);
    assert!(
        compile_task_capsule(&escaped_value, "epoch.escaped", token_budget).is_err(),
        "JSON-escaped newline growth must count against the renderer's byte bound"
    );
}

#[test]
fn near_ids_and_wrong_scope_or_epoch_do_not_reuse_messages_or_acknowledgments() {
    let value = contract();
    let current = claimed(&value);
    let capsule_v1 =
        compile_task_capsule(&value, "epoch.current", 2048).expect("current complete capsule");

    let mut other_project = value.clone();
    other_project.project_id = ProjectId::new("project.alpha-near").expect("near project ID");
    let other_project_capsule =
        compile_task_capsule(&other_project, "epoch.current", 2048).expect("near project capsule");
    assert_ne!(
        capsule_v1.contract_digest,
        other_project_capsule.contract_digest
    );
    assert_ne!(capsule_v1.text, other_project_capsule.text);

    let mut other_root = value.clone();
    other_root.root_id = RootId::new("root.alpha-near").expect("near root ID");
    let other_root_capsule =
        compile_task_capsule(&other_root, "epoch.current", 2048).expect("near root capsule");
    assert_ne!(
        capsule_v1.contract_digest,
        other_root_capsule.contract_digest
    );
    assert_ne!(capsule_v1.text, other_root_capsule.text);

    let capsule_v2 = compile_task_capsule(&value, "epoch.recovered", 2048)
        .expect("fresh full capsule for new epoch");
    assert_ne!(capsule_v1.context_epoch, capsule_v2.context_epoch);
    assert_ne!(capsule_v1.text, capsule_v2.text);
    assert_eq!(capsule_v1.contract_digest, capsule_v2.contract_digest);

    let valid = assignment(
        &value,
        "assignment.current",
        "captain.alpha",
        "epoch.current",
    );
    for mut forged in [
        {
            let mut message = valid.clone();
            message.project_id = ProjectId::new("project.alpha-near").expect("near project ID");
            message
        },
        {
            let mut message = valid.clone();
            message.authority_ref = "grant:other".into();
            message
        },
        {
            let mut message = valid.clone();
            message.payload = MessagePayload::TaskAssigned {
                task_id: TaskId::new("task.alpha-near").expect("near task ID"),
                revision: value.revision,
                contract_digest: content_digest(&value).expect("fixture digest"),
            };
            message
        },
        {
            let mut wrong_root_digest = other_root.clone();
            wrong_root_digest.task_id = value.task_id.clone();
            wrong_root_digest.project_id = value.project_id.clone();
            let mut message = valid.clone();
            message.payload = MessagePayload::TaskAssigned {
                task_id: value.task_id.clone(),
                revision: value.revision,
                contract_digest: content_digest(&wrong_root_digest).expect("wrong-root digest"),
            };
            message
        },
    ] {
        forged.message_id = MessageId::new(format!("forged-{}", forged.message_id))
            .expect("unique forged message ID");
        assert!(
            reduce(&current, &DomainEvent::MessageQueued { message: forged }).is_err(),
            "wrong project, root-bound digest, authority or near task ID must not queue"
        );
    }

    let queued = reduce(
        &current,
        &DomainEvent::MessageQueued {
            message: valid.clone(),
        },
    )
    .expect("queue current assignment");
    assert!(
        reduce(
            &queued,
            &DomainEvent::MessageAcknowledged {
                message_id: valid.message_id.clone(),
                recipient: PrincipalId::new("captain.near").expect("near recipient ID"),
                context_epoch: valid.context_epoch.clone(),
            }
        )
        .is_err(),
        "similar recipient cannot acknowledge this message"
    );
    assert!(
        reduce(
            &queued,
            &DomainEvent::MessageAcknowledged {
                message_id: valid.message_id.clone(),
                recipient: valid.recipient.clone(),
                context_epoch: "epoch.recovered".into(),
            }
        )
        .is_err(),
        "new epoch cannot acknowledge a message delivered in an old epoch"
    );

    let acked = reduce(
        &queued,
        &DomainEvent::MessageAcknowledged {
            message_id: valid.message_id.clone(),
            recipient: valid.recipient,
            context_epoch: valid.context_epoch,
        },
    )
    .expect("exact recipient and epoch acknowledge");
    assert_eq!(
        acked.acknowledgments[&MessageId::new("assignment.current").expect("message ID")],
        "epoch.current"
    );
    assert_eq!(acked.tasks, queued.tasks);
    assert_eq!(acked.tasks[&value.task_id].state, SchedulingState::Claimed);
    assert!(acked.tasks[&value.task_id].candidate_ref.is_none());
}

#[test]
fn reducer_rejects_duplicate_message_id_but_has_no_producer_replay_ledger() {
    let value = contract();
    let state = claimed(&value);
    let message = assignment(
        &value,
        "assignment.replay",
        "captain.alpha",
        "epoch.alpha.1",
    );
    let queued = reduce(
        &state,
        &DomainEvent::MessageQueued {
            message: message.clone(),
        },
    )
    .expect("first event with this message ID queues");

    assert!(
        reduce(
            &queued,
            &DomainEvent::MessageQueued {
                message: message.clone(),
            }
        )
        .is_err(),
        "the projection reducer rejects an exact duplicate message ID"
    );
    let mut changed = message;
    changed.context_epoch = "epoch.changed".into();
    assert!(
        reduce(&queued, &DomainEvent::MessageQueued { message: changed }).is_err(),
        "the projection reducer also rejects changed bytes under that ID"
    );
}

#[test]
fn epoch_change_recompiles_contract_and_preserves_declared_dependency() {
    let mut value = contract();
    value.dependencies = vec![TaskId::new("dependency.alpha").expect("fixture dependency ID")];
    let before =
        compile_task_capsule(&value, "epoch.before", 2048).expect("dependency-bearing capsule");
    assert!(before.text.contains("dependencies=[\"dependency.alpha\"]"));
    let after = compile_task_capsule(&value, "epoch.after-restart", 2048)
        .expect("new-epoch complete contract capsule");
    assert_ne!(before.context_epoch, after.context_epoch);
    assert!(after.text.contains("dependencies=[\"dependency.alpha\"]"));
    assert_eq!(before.contract_digest, after.contract_digest);

    // compile_task_capsule accepts only a contract, epoch and budget. It has
    // no active-question or evidence input to recover; do not invent one here.
}
