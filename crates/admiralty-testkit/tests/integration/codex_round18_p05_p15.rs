#![allow(clippy::unwrap_used, clippy::expect_used)]
use admiralty_core::coordination::questions::{QuestionEvent, QuestionScope, QuestionStatus};
use admiralty_core::verification::{
    CheckResult, ReceiptProducer, RequiredNegativeCheck, VerificationInputs, VerificationPolicy,
    VerificationReceipt, parse_receipt, validate_receipt,
};
use admiralty_core::{
    AcceptanceCriterion, AttemptId, CheckSpec, MessageId, PrincipalId, ProjectId, RootId,
    TaskContract, TaskId, content_digest,
};
use admiralty_store::questions::{JournalScope, append, initialize, reconstruct};
use rusqlite::Connection;

fn contract() -> TaskContract {
    TaskContract {
        schema_version: 1,
        project_id: ProjectId::new("round18-project").expect("valid project ID"),
        root_id: RootId::new("round18-root").expect("valid root ID"),
        task_id: TaskId::new("round18-task").expect("valid task ID"),
        revision: 4,
        title: "Release bundle schema parity".into(),
        objective: "Bind routing, escalation, and verification to one task contract".into(),
        non_goals: vec!["automatic release approval".into()],
        owned_paths: vec!["crates/admiralty-testkit".into()],
        forbidden_paths: vec!["secrets".into()],
        read_first: vec![],
        dependencies: vec![],
        criteria: vec![AcceptanceCriterion {
            id: "bundle-integrity".into(),
            text: "Bundle identities and schemas agree".into(),
            check_ids: vec!["round18-check".into()],
        }],
        checks: vec![CheckSpec {
            id: "round18-check".into(),
            program: "cargo".into(),
            args: vec!["test".into()],
            cwd: ".".into(),
            timeout_secs: 30,
        }],
        authority_ref: "authority:round18".into(),
        max_attempts: 1,
        simulated: false,
    }
}

#[test]
fn codex_round18_p05_p15_release_bundle_schema_parity() {
    let contract = contract();
    contract.validate().expect("P02 contract validates");
    let contract_digest = content_digest(&contract).expect("canonical contract digest");
    let scope = QuestionScope {
        question_id: MessageId::new("round18-question").expect("valid question ID"),
        project_id: contract.project_id.clone(),
        task_id: contract.task_id.clone(),
    };
    let journal_scope = JournalScope {
        root_id: contract.root_id.clone(),
        question: scope.clone(),
        authority_ref: contract.authority_ref.clone(),
    };
    let directory = tempfile::tempdir().expect("isolated durable release bundle");
    let database = directory.path().join("round18.sqlite3");
    let mut connection = Connection::open(&database).expect("open durable journal");
    initialize(&connection).expect("initialize P05 event and escalation schemas");

    let worker = PrincipalId::new("worker").expect("valid worker ID");
    let reviewer = PrincipalId::new("reviewer").expect("valid reviewer ID");
    let opening = QuestionEvent::Open {
        event_id: MessageId::new("round18-open-event").expect("valid event ID"),
        scope: scope.clone(),
        authority_ref: journal_scope.authority_ref.clone(),
        asker: PrincipalId::new("captain").expect("valid asker ID"),
        recipient: worker.clone(),
        allowed_recipients: vec![worker.clone(), reviewer.clone()],
        requirement: format!("Verify P02 contract {contract_digest}"),
        question: contract.objective.clone(),
        options: vec!["hold".into(), "review".into()],
        deadline_utc: Some(500),
    };
    append(
        &mut connection,
        &journal_scope,
        &MessageId::new("round18-open-command").expect("valid command ID"),
        &opening,
    )
    .expect("persist question under exact contract scope");
    let route = QuestionEvent::Route {
        event_id: MessageId::new("round18-route-event").expect("valid route event ID"),
        scope: scope.clone(),
        expected_revision: 1,
        authority_ref: journal_scope.authority_ref.clone(),
        actor: worker,
        recipient: reviewer.clone(),
    };
    let routed = append(
        &mut connection,
        &journal_scope,
        &MessageId::new("round18-route-command").expect("valid route command ID"),
        &route,
    )
    .expect("P05 routes to an allowlisted reviewer");
    assert_eq!(routed.revision, "2");

    let expired = QuestionEvent::EvaluateDeadline {
        event_id: MessageId::new("round18-expire-event").expect("valid expiry event ID"),
        scope: scope.clone(),
        expected_revision: 2,
        authority_ref: journal_scope.authority_ref.clone(),
        actor: reviewer,
        observed_utc: Some(501),
    };
    let expiration = append(
        &mut connection,
        &journal_scope,
        &MessageId::new("round18-expire-command").expect("valid expiry command ID"),
        &expired,
    )
    .expect("deadline creates a durable escalation proposal");
    let escalation_ref = expiration
        .escalation_ref
        .clone()
        .expect("release bundle retains escalation identity");
    let stored_event: String = connection
        .query_row(
            "SELECT event_json FROM durable_question_events WHERE question_id=?1 AND sequence=2",
            [scope.question_id.as_str()],
            |row| row.get(0),
        )
        .expect("read stored route schema");
    assert_eq!(
        serde_json::from_str::<QuestionEvent>(&stored_event).expect("route schema roundtrips"),
        route
    );
    let proposal_json: String = connection
        .query_row(
            "SELECT proposal_json FROM durable_question_escalations WHERE question_id=?1",
            [scope.question_id.as_str()],
            |row| row.get(0),
        )
        .expect("read escalation bundle payload");
    let proposal: serde_json::Value =
        serde_json::from_str(&proposal_json).expect("escalation payload is valid JSON");
    assert_eq!(proposal["project_id"], contract.project_id.as_str());
    assert_eq!(proposal["root_id"], contract.root_id.as_str());
    assert_eq!(proposal["task_id"], contract.task_id.as_str());
    assert_eq!(proposal["question_id"], scope.question_id.as_str());
    assert_eq!(proposal["authority_ref"], contract.authority_ref);
    assert!(proposal.get("acceptance_granted").is_none());
    assert!(proposal.get("release_granted").is_none());

    let policy = VerificationPolicy {
        schema_version: 1,
        verifier: PrincipalId::new("independent-reviewer").expect("valid verifier ID"),
        protected_oracle_digest: format!("sha256:{}", "a".repeat(64)),
        required_negative_checks: vec![RequiredNegativeCheck {
            check: CheckSpec {
                id: "reject-forgery".into(),
                program: "fixture".into(),
                args: vec![],
                cwd: ".".into(),
                timeout_secs: 10,
            },
            expected_exit_code: 1,
            fixture_digest: format!("sha256:{}", "b".repeat(64)),
        }],
    };
    let inputs = VerificationInputs {
        project_id: contract.project_id.clone(),
        root_id: contract.root_id.clone(),
        task_id: contract.task_id.clone(),
        attempt_id: AttemptId::new("round18-attempt").expect("valid attempt ID"),
        candidate_ref: "candidate:round18".into(),
        contract_revision: contract.revision,
        contract_digest,
        verification_policy_digest: content_digest(&policy).expect("policy digest"),
        source_commit_sha: "1".repeat(40),
        source_tree_sha: "2".repeat(40),
        source_artifact_digest: format!("sha256:{}", "c".repeat(64)),
        protected_oracle_digest: policy.protected_oracle_digest.clone(),
    };
    let receipt = VerificationReceipt {
        schema_version: 1,
        receipt_id: "round18-release-bundle".into(),
        producer: ReceiptProducer::IndependentVerifier,
        verifier: policy.verifier.clone(),
        inputs: inputs.clone(),
        checks: contract
            .checks
            .iter()
            .cloned()
            .map(|check| CheckResult {
                check,
                exit_code: Some(0),
                stdout_digest: format!("sha256:{}", "d".repeat(64)),
                stderr_digest: format!("sha256:{}", "e".repeat(64)),
            })
            .collect(),
        negative_checks: policy
            .required_negative_checks
            .iter()
            .map(|negative| CheckResult {
                check: negative.check.clone(),
                exit_code: Some(1),
                stdout_digest: format!("sha256:{}", "f".repeat(64)),
                stderr_digest: format!("sha256:{}", "0".repeat(64)),
            })
            .collect(),
    };
    let encoded_receipt = serde_json::to_vec(&receipt).expect("serialize P15 receipt");
    let parsed_receipt = parse_receipt(&encoded_receipt).expect("P15 schema parses its own bundle");
    assert_eq!(parsed_receipt, receipt);
    let validated = validate_receipt(&contract, &inputs, &policy, &parsed_receipt)
        .expect("receipt identities and pinned checks match P02");
    assert!(validated.all_checks_passed());
    assert!(!validated.acceptance_granted());
    assert!(!validated.verifier_authenticated());
    assert!(!validated.execution_qualified());
    assert!(!validated.evidence_custody_verified());

    let mut future_event = serde_json::to_value(&route).expect("serialize route event");
    future_event["unrecognized_grant"] = serde_json::json!(true);
    assert!(serde_json::from_value::<QuestionEvent>(future_event).is_err());
    let mut forged_receipt = serde_json::to_value(&receipt).expect("serialize receipt bundle");
    forged_receipt["release_granted"] = serde_json::json!(true);
    assert!(
        parse_receipt(&serde_json::to_vec(&forged_receipt).expect("encode forged receipt"))
            .is_err()
    );

    drop(connection);
    let reopened = Connection::open(&database).expect("reopen durable release bundle");
    let recovered = reconstruct(&reopened, &journal_scope)
        .expect("replay checks the durable event chain")
        .expect("question is present after reopen");
    assert_eq!(
        recovered.state.status,
        QuestionStatus::Expired { observed_utc: 501 }
    );
    assert_eq!(recovered.event_count, "3");
    assert_eq!(
        recovered.escalation_ref.as_deref(),
        Some(escalation_ref.as_str())
    );
}
