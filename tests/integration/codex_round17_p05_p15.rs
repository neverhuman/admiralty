use admiralty_core::coordination::questions::{QuestionEvent, QuestionScope, QuestionStatus};
use admiralty_core::verification::{
    CheckResult, ReceiptProducer, RequiredNegativeCheck, VerificationInputs, VerificationPolicy,
    VerificationReceipt, parse_receipt, validate_receipt,
};
use admiralty_core::{
    AcceptanceCriterion, AttemptId, CheckSpec, MessageId, PrincipalId, ProjectId, RootId,
    TaskContract, TaskId, content_digest,
};
use admiralty_store::questions::{
    JournalScope, QuestionStoreError, append, initialize, reconstruct,
};
use rusqlite::Connection;

fn round17_contract() -> TaskContract {
    TaskContract {
        schema_version: 1,
        project_id: ProjectId::new("round17-project").expect("valid project ID"),
        root_id: RootId::new("round17-root").expect("valid root ID"),
        task_id: TaskId::new("round17-task").expect("valid task ID"),
        revision: 7,
        title: "Cross-packet release audit".into(),
        objective: "Review exact task evidence before release".into(),
        non_goals: vec![],
        owned_paths: vec!["src".into()],
        forbidden_paths: vec!["secrets".into()],
        read_first: vec![],
        dependencies: vec![],
        criteria: vec![AcceptanceCriterion {
            id: "C1".into(),
            text: "The required check passes".into(),
            check_ids: vec!["release-check".into()],
        }],
        checks: vec![CheckSpec {
            id: "release-check".into(),
            program: "round17-check".into(),
            args: vec!["--verify".into()],
            cwd: ".".into(),
            timeout_secs: 10,
        }],
        authority_ref: "authority:round17".into(),
        max_attempts: 1,
        simulated: false,
    }
}

#[test]
fn p02_p05_p15_round17_post_item5_release_audit_cross_packet_invariants() {
    let contract = round17_contract();
    contract
        .validate()
        .expect("P02 contract is valid and immutable");
    let contract_digest = content_digest(&contract).expect("canonical contract digest");
    let question = QuestionScope {
        question_id: MessageId::new("round17-question").expect("valid question ID"),
        project_id: contract.project_id.clone(),
        task_id: contract.task_id.clone(),
    };
    let scope = JournalScope {
        root_id: contract.root_id.clone(),
        question: question.clone(),
        authority_ref: contract.authority_ref.clone(),
    };
    let directory = tempfile::tempdir().expect("private release audit fixture");
    let database = directory.path().join("question-audit.sqlite3");
    let mut connection = Connection::open(&database).expect("open durable audit database");
    initialize(&connection).expect("initialize event and escalation journals");

    let worker = PrincipalId::new("worker").expect("valid worker ID");
    let reviewer = PrincipalId::new("reviewer").expect("valid reviewer ID");
    let opening = QuestionEvent::Open {
        event_id: MessageId::new("round17-open-event").expect("valid event ID"),
        scope: question.clone(),
        authority_ref: scope.authority_ref.clone(),
        asker: PrincipalId::new("captain").expect("valid asker ID"),
        recipient: worker.clone(),
        allowed_recipients: vec![worker.clone(), reviewer.clone()],
        requirement: format!("Review P02 contract {contract_digest}"),
        question: contract.objective.clone(),
        options: vec!["hold".into(), "release-review".into()],
        deadline_utc: Some(900),
    };
    let first = append(
        &mut connection,
        &scope,
        &MessageId::new("round17-open-command").expect("valid command ID"),
        &opening,
    )
    .expect("record exact P02 scope in P05 journal");
    let opening_replay = append(
        &mut connection,
        &scope,
        &MessageId::new("round17-open-replay-command").expect("valid replay command ID"),
        &opening,
    )
    .expect("same event replay returns original durable receipt");
    assert!(opening_replay.duplicate);
    assert_eq!(opening_replay.event_seq, first.event_seq);
    assert_eq!(opening_replay.revision, first.revision);

    let mut changed_opening = opening.clone();
    if let QuestionEvent::Open { question, .. } = &mut changed_opening {
        *question = "attacker replacement content".into();
    }
    assert!(matches!(
        append(
            &mut connection,
            &scope,
            &MessageId::new("round17-mutated-open-command").expect("valid command ID"),
            &changed_opening,
        ),
        Err(QuestionStoreError::IdentityCollision | QuestionStoreError::Reducer(_))
    ));

    let wrong_actor_route = QuestionEvent::Route {
        event_id: MessageId::new("round17-wrong-actor-event").expect("valid event ID"),
        scope: question.clone(),
        expected_revision: 1,
        authority_ref: scope.authority_ref.clone(),
        actor: PrincipalId::new("intruder").expect("valid adversarial actor"),
        recipient: reviewer.clone(),
    };
    assert!(matches!(
        append(
            &mut connection,
            &scope,
            &MessageId::new("round17-wrong-actor-command").expect("valid command ID"),
            &wrong_actor_route,
        ),
        Err(QuestionStoreError::Reducer(_))
    ));

    let route = QuestionEvent::Route {
        event_id: MessageId::new("round17-route-event").expect("valid route event ID"),
        scope: question.clone(),
        expected_revision: 1,
        authority_ref: scope.authority_ref.clone(),
        actor: worker,
        recipient: reviewer.clone(),
    };
    let routed = append(
        &mut connection,
        &scope,
        &MessageId::new("round17-route-command").expect("valid route command ID"),
        &route,
    )
    .expect("current recipient routes to allowlisted reviewer");
    let route_replay = append(
        &mut connection,
        &scope,
        &MessageId::new("round17-route-replay-command").expect("valid replay command ID"),
        &route,
    )
    .expect("exact route replay is stable");
    assert!(route_replay.duplicate);
    assert_eq!(route_replay.revision, routed.revision);

    for (suffix, observation) in [("unknown-clock", None), ("deadline-boundary", Some(900))] {
        let rejected = QuestionEvent::EvaluateDeadline {
            event_id: MessageId::new(format!("round17-{suffix}-event"))
                .expect("valid deterministic event ID"),
            scope: question.clone(),
            expected_revision: 2,
            authority_ref: scope.authority_ref.clone(),
            actor: reviewer.clone(),
            observed_utc: observation,
        };
        assert!(matches!(
            append(
                &mut connection,
                &scope,
                &MessageId::new(format!("round17-{suffix}-command"))
                    .expect("valid deterministic command ID"),
                &rejected,
            ),
            Err(QuestionStoreError::Reducer(_))
        ));
    }
    let still_routed = reconstruct(&connection, &scope)
        .expect("failed deadline probes preserve valid history")
        .expect("question remains present");
    assert_eq!(still_routed.state.revision, 2);
    assert_eq!(still_routed.event_count, "2");
    assert_eq!(still_routed.state.status, QuestionStatus::Routed);

    let expiration = QuestionEvent::EvaluateDeadline {
        event_id: MessageId::new("round17-expiration-event").expect("valid deadline event ID"),
        scope: question.clone(),
        expected_revision: 2,
        authority_ref: scope.authority_ref.clone(),
        actor: reviewer,
        observed_utc: Some(901),
    };
    let expired = append(
        &mut connection,
        &scope,
        &MessageId::new("round17-expiration-command").expect("valid deadline command ID"),
        &expiration,
    )
    .expect("strictly expired review becomes escalation proposal");
    let escalation_ref = expired
        .escalation_ref
        .clone()
        .expect("durable escalation reference exists");
    let replayed_expiration = append(
        &mut connection,
        &scope,
        &MessageId::new("round17-expiration-replay").expect("valid replay command ID"),
        &expiration,
    )
    .expect("replayed expiration reuses the single proposal");
    assert!(replayed_expiration.duplicate);
    assert_eq!(
        replayed_expiration.escalation_ref.as_deref(),
        Some(escalation_ref.as_str())
    );
    let proposal_json: String = connection
        .query_row(
            "SELECT proposal_json FROM durable_question_escalations WHERE escalation_ref=?1",
            [&escalation_ref],
            |row| row.get(0),
        )
        .expect("read escalation inbox proposal");
    let proposal: serde_json::Value =
        serde_json::from_str(&proposal_json).expect("proposal is valid JSON");
    assert_eq!(proposal["project_id"], contract.project_id.as_str());
    assert_eq!(proposal["root_id"], contract.root_id.as_str());
    assert_eq!(proposal["task_id"], contract.task_id.as_str());
    assert_eq!(proposal["question_id"], question.question_id.as_str());
    assert!(proposal.get("acceptance_granted").is_none());
    assert!(proposal.get("release_granted").is_none());
    drop(connection);

    let reopened = Connection::open(&database).expect("reopen released inbox journal");
    let recovered = reconstruct(&reopened, &scope)
        .expect("reconstruct and audit durable event chain")
        .expect("recovered question exists");
    assert_eq!(recovered.state.revision, 3);
    assert_eq!(recovered.event_count, "3");
    assert_eq!(
        recovered.state.status,
        QuestionStatus::Expired { observed_utc: 901 }
    );
    assert_eq!(
        recovered.escalation_ref.as_deref(),
        Some(escalation_ref.as_str())
    );

    let oracle_digest = format!("sha256:{}", "d".repeat(64));
    let policy = VerificationPolicy {
        schema_version: 1,
        verifier: PrincipalId::new("independent-reviewer").expect("valid verifier ID"),
        protected_oracle_digest: oracle_digest.clone(),
        required_negative_checks: vec![RequiredNegativeCheck {
            check: CheckSpec {
                id: "negative-check".into(),
                program: "fixture-negative".into(),
                args: vec![],
                cwd: ".".into(),
                timeout_secs: 10,
            },
            expected_exit_code: 1,
            fixture_digest: format!("sha256:{}", "9".repeat(64)),
        }],
    };
    let inputs = VerificationInputs {
        project_id: contract.project_id.clone(),
        root_id: contract.root_id.clone(),
        task_id: contract.task_id.clone(),
        attempt_id: AttemptId::new("round17-attempt").expect("valid attempt ID"),
        candidate_ref: "candidate:round17".into(),
        contract_revision: contract.revision,
        contract_digest,
        verification_policy_digest: content_digest(&policy).expect("policy digest"),
        source_commit_sha: "1".repeat(40),
        source_tree_sha: "2".repeat(40),
        source_artifact_digest: format!("sha256:{}", "c".repeat(64)),
        protected_oracle_digest: oracle_digest,
    };
    let receipt = VerificationReceipt {
        schema_version: 1,
        receipt_id: "round17-release-audit".into(),
        producer: ReceiptProducer::IndependentVerifier,
        verifier: PrincipalId::new("independent-reviewer").expect("valid verifier ID"),
        inputs: inputs.clone(),
        checks: contract
            .checks
            .iter()
            .cloned()
            .map(|check| CheckResult {
                check,
                exit_code: Some(0),
                stdout_digest: format!("sha256:{}", "e".repeat(64)),
                stderr_digest: format!("sha256:{}", "f".repeat(64)),
            })
            .collect(),
        negative_checks: policy
            .required_negative_checks
            .iter()
            .map(|negative| CheckResult {
                check: negative.check.clone(),
                exit_code: Some(1),
                stdout_digest: format!("sha256:{}", "1".repeat(64)),
                stderr_digest: format!("sha256:{}", "2".repeat(64)),
            })
            .collect(),
    };
    let validated = validate_receipt(&contract, &inputs, &policy, &receipt)
        .expect("P15 receipt matches exact P02 contract and pinned policy");
    assert!(validated.all_checks_passed());
    assert!(!validated.acceptance_granted());
    assert!(!validated.verifier_authenticated());
    assert!(!validated.execution_qualified());
    assert!(!validated.evidence_custody_verified());

    let mut provider_claim = receipt.clone();
    provider_claim.producer = ReceiptProducer::ProviderClaim;
    assert!(validate_receipt(&contract, &inputs, &policy, &provider_claim).is_err());
    let mut wrong_candidate = inputs;
    wrong_candidate.source_tree_sha = "3".repeat(40);
    assert!(validate_receipt(&contract, &wrong_candidate, &policy, &receipt).is_err());
    let mut forged_json = serde_json::to_value(receipt).expect("serialize receipt for audit");
    forged_json["acceptance_granted"] = serde_json::json!(true);
    assert!(
        parse_receipt(&serde_json::to_vec(&forged_json).expect("serialize forged receipt"))
            .is_err()
    );
}
