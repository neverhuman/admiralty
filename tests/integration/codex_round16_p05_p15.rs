use admiralty_core::coordination::questions::{QuestionEvent, QuestionScope, QuestionStatus};
use admiralty_core::verification::{
    CheckResult, ReceiptProducer, RequiredNegativeCheck, VerificationInputs, VerificationPolicy,
    VerificationReceipt, validate_receipt,
};
use admiralty_core::{
    AcceptanceCriterion, AttemptId, CheckSpec, MessageId, PrincipalId, ProjectId, RootId,
    TaskContract, TaskId, content_digest,
};
use admiralty_store::questions::{
    JournalScope, QuestionStoreError, append, initialize, reconstruct,
};
use rusqlite::Connection;

fn contract() -> TaskContract {
    TaskContract {
        schema_version: 1,
        project_id: ProjectId::new("round16-project").expect("valid project ID"),
        root_id: RootId::new("round16-root").expect("valid root ID"),
        task_id: TaskId::new("round16-release").expect("valid task ID"),
        revision: 4,
        title: "Round 16 release review".into(),
        objective: "Verify the exact candidate before release".into(),
        non_goals: vec![],
        owned_paths: vec!["src".into()],
        forbidden_paths: vec!["secrets".into()],
        read_first: vec![],
        dependencies: vec![],
        criteria: vec![AcceptanceCriterion {
            id: "C1".into(),
            text: "The pinned release check succeeds".into(),
            check_ids: vec!["release-check".into()],
        }],
        checks: vec![CheckSpec {
            id: "release-check".into(),
            program: "fixture-release-check".into(),
            args: vec!["--verify".into()],
            cwd: ".".into(),
            timeout_secs: 5,
        }],
        authority_ref: "authority:round16".into(),
        max_attempts: 1,
        simulated: false,
    }
}

#[test]
fn p02_p05_p15_round16_end_to_end_cutover_fault_containment() {
    let contract = contract();
    let question = QuestionScope {
        question_id: MessageId::new("round16-question").expect("valid question ID"),
        project_id: contract.project_id.clone(),
        task_id: contract.task_id.clone(),
    };
    let scope = JournalScope {
        root_id: contract.root_id.clone(),
        question: question.clone(),
        authority_ref: contract.authority_ref.clone(),
    };
    let directory = tempfile::tempdir().expect("private cutover fixture");
    let database = directory.path().join("release.sqlite3");
    let mut connection = Connection::open(&database).expect("open SQLite journal");
    initialize(&connection).expect("initialize durable question and escalation inbox");

    let worker = PrincipalId::new("worker").expect("valid worker ID");
    let reviewer = PrincipalId::new("reviewer").expect("valid reviewer ID");
    let release_owner = PrincipalId::new("release-owner").expect("valid release owner ID");
    let opening = QuestionEvent::Open {
        event_id: MessageId::new("round16-open-event").expect("valid event ID"),
        scope: question.clone(),
        authority_ref: scope.authority_ref.clone(),
        asker: PrincipalId::new("captain").expect("valid asker ID"),
        recipient: worker.clone(),
        allowed_recipients: vec![worker.clone(), reviewer.clone(), release_owner.clone()],
        requirement: format!(
            "Review contract {}",
            content_digest(&contract).expect("contract digest")
        ),
        question: contract.objective.clone(),
        options: vec!["hold".into(), "release-review".into()],
        deadline_utc: Some(500),
    };
    append(
        &mut connection,
        &scope,
        &MessageId::new("round16-open-command").expect("valid command ID"),
        &opening,
    )
    .expect("persist scoped P02 work question");

    let route_to_reviewer = QuestionEvent::Route {
        event_id: MessageId::new("round16-route-reviewer-event").expect("valid event ID"),
        scope: question.clone(),
        expected_revision: 1,
        authority_ref: scope.authority_ref.clone(),
        actor: worker,
        recipient: reviewer.clone(),
    };
    append(
        &mut connection,
        &scope,
        &MessageId::new("round16-route-reviewer-command").expect("valid command ID"),
        &route_to_reviewer,
    )
    .expect("route from current worker to allowed reviewer");
    let route_to_release_owner = QuestionEvent::Route {
        event_id: MessageId::new("round16-route-release-event").expect("valid event ID"),
        scope: question.clone(),
        expected_revision: 2,
        authority_ref: scope.authority_ref.clone(),
        actor: reviewer,
        recipient: release_owner.clone(),
    };
    append(
        &mut connection,
        &scope,
        &MessageId::new("round16-route-release-command").expect("valid command ID"),
        &route_to_release_owner,
    )
    .expect("complete multi-hop release review routing");

    // Inject a deterministic inbox sink failure. The expiration event,
    // projection, command receipt and escalation insert must all roll back.
    connection
        .execute_batch(
            "CREATE TRIGGER reject_round16_escalation BEFORE INSERT ON durable_question_escalations \
             BEGIN SELECT RAISE(ABORT, 'round16 injected escalation sink failure'); END;",
        )
        .expect("install deterministic fault trigger");
    let expiration = QuestionEvent::EvaluateDeadline {
        event_id: MessageId::new("round16-expiration-event").expect("valid event ID"),
        scope: question.clone(),
        expected_revision: 3,
        authority_ref: scope.authority_ref.clone(),
        actor: release_owner.clone(),
        observed_utc: Some(501),
    };
    let expiration_command =
        MessageId::new("round16-expiration-command").expect("valid command ID");
    assert!(matches!(
        append(&mut connection, &scope, &expiration_command, &expiration),
        Err(QuestionStoreError::Sqlite(_))
    ));
    let unchanged = reconstruct(&connection, &scope)
        .expect("failed cutover leaves reconstructable history")
        .expect("question remains present after rollback");
    assert_eq!(unchanged.state.revision, 3);
    assert_eq!(unchanged.state.status, QuestionStatus::Routed);
    assert_eq!(unchanged.event_count, "3");
    let absent_inbox_entry: i64 = connection
        .query_row(
            "SELECT count(*) FROM durable_question_escalations",
            [],
            |row| row.get(0),
        )
        .expect("read escalation inbox after failed insert");
    assert_eq!(absent_inbox_entry, 0);

    connection
        .execute_batch("DROP TRIGGER reject_round16_escalation;")
        .expect("remove injected sink fault");
    let cutover = append(&mut connection, &scope, &expiration_command, &expiration)
        .expect("same command safely retries after atomic rollback");
    assert_eq!(cutover.revision, "4");
    let escalation_ref = cutover
        .escalation_ref
        .clone()
        .expect("successful cutover returns durable escalation identity");
    let replay = append(
        &mut connection,
        &scope,
        &MessageId::new("round16-expiration-replay").expect("valid replay command"),
        &expiration,
    )
    .expect("exact replay returns the same escalation receipt");
    assert!(replay.duplicate);
    assert_eq!(
        replay.escalation_ref.as_deref(),
        Some(escalation_ref.as_str())
    );

    let terminal_answer = QuestionEvent::Answer {
        event_id: MessageId::new("round16-late-answer-event").expect("valid event ID"),
        scope: question,
        expected_revision: 4,
        authority_ref: scope.authority_ref.clone(),
        actor: release_owner,
        answer: "release approved".into(),
    };
    assert!(matches!(
        append(
            &mut connection,
            &scope,
            &MessageId::new("round16-late-answer-command").expect("valid command ID"),
            &terminal_answer,
        ),
        Err(QuestionStoreError::Reducer(_))
    ));
    let inbox_count: i64 = connection
        .query_row(
            "SELECT count(*) FROM durable_question_escalations",
            [],
            |row| row.get(0),
        )
        .expect("read final escalation inbox count");
    assert_eq!(inbox_count, 1);
    drop(connection);

    let reopened = Connection::open(&database).expect("reopen after release cutover");
    let recovered = reconstruct(&reopened, &scope)
        .expect("verify event chain and escalation binding after restart")
        .expect("recover question and escalation");
    assert_eq!(
        recovered.state.status,
        QuestionStatus::Expired { observed_utc: 501 }
    );
    assert_eq!(
        recovered.escalation_ref.as_deref(),
        Some(escalation_ref.as_str())
    );
    assert_eq!(recovered.event_count, "4");

    // P15 validators establish check consistency only; the release transition
    // still needs separate trust and cutover evidence.
    let oracle_digest = format!("sha256:{}", "d".repeat(64));
    let policy = VerificationPolicy {
        schema_version: 1,
        verifier: PrincipalId::new("reviewer").expect("valid verifier ID"),
        protected_oracle_digest: oracle_digest.clone(),
        required_negative_checks: vec![RequiredNegativeCheck {
            check: CheckSpec {
                id: "negative-check".into(),
                program: "fixture-negative".into(),
                args: vec![],
                cwd: ".".into(),
                timeout_secs: 5,
            },
            expected_exit_code: 1,
            fixture_digest: format!("sha256:{}", "9".repeat(64)),
        }],
    };
    let inputs = VerificationInputs {
        project_id: contract.project_id.clone(),
        root_id: contract.root_id.clone(),
        task_id: contract.task_id.clone(),
        attempt_id: AttemptId::new("round16-attempt").expect("valid attempt ID"),
        candidate_ref: "candidate:round16".into(),
        contract_revision: contract.revision,
        contract_digest: content_digest(&contract).expect("contract digest"),
        verification_policy_digest: content_digest(&policy).expect("policy digest"),
        source_commit_sha: "1".repeat(40),
        source_tree_sha: "2".repeat(40),
        source_artifact_digest: format!("sha256:{}", "c".repeat(64)),
        protected_oracle_digest: oracle_digest,
    };
    let receipt = VerificationReceipt {
        schema_version: 1,
        receipt_id: "round16-independent-checks".into(),
        producer: ReceiptProducer::IndependentVerifier,
        verifier: PrincipalId::new("reviewer").expect("valid verifier ID"),
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
        .expect("P15 receipt exactly matches contract and policy");
    assert!(validated.all_checks_passed());
    assert!(!validated.acceptance_granted());
    assert!(!validated.execution_qualified());
    let mut provider_claim = receipt;
    provider_claim.producer = ReceiptProducer::ProviderClaim;
    assert!(validate_receipt(&contract, &inputs, &policy, &provider_claim).is_err());
}
