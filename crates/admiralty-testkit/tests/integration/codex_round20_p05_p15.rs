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

fn id(value: &str) -> MessageId {
    MessageId::new(value).expect("fixed test message ID is valid")
}

fn release_contract() -> TaskContract {
    TaskContract {
        schema_version: 1,
        project_id: ProjectId::new("round20-project").expect("valid project ID"),
        root_id: RootId::new("round20-root").expect("valid root ID"),
        task_id: TaskId::new("round20-task").expect("valid task ID"),
        revision: 20,
        title: "Post PR 57 release qualification".into(),
        objective: "Verify release evidence against the exact task scope".into(),
        non_goals: vec!["granting release authority".into()],
        owned_paths: vec!["crates/admiralty-testkit".into()],
        forbidden_paths: vec!["secrets".into()],
        read_first: vec![],
        dependencies: vec![],
        criteria: vec![AcceptanceCriterion {
            id: "qualification".into(),
            text: "Question, escalation, and receipt scopes match".into(),
            check_ids: vec!["strict-audit".into()],
        }],
        checks: vec![CheckSpec {
            id: "strict-audit".into(),
            program: "cargo".into(),
            args: vec!["clippy".into(), "--workspace".into()],
            cwd: ".".into(),
            timeout_secs: 120,
        }],
        authority_ref: "authority:round20-release".into(),
        max_attempts: 1,
        simulated: false,
    }
}

#[test]
fn codex_round20_p05_p15_post_pr57_release_qualification() {
    let contract = release_contract();
    contract.validate().expect("P02 contract is valid");
    let contract_digest = content_digest(&contract).expect("stable contract digest");
    let scope = QuestionScope {
        question_id: id("round20-question"),
        project_id: contract.project_id.clone(),
        task_id: contract.task_id.clone(),
    };
    let journal_scope = JournalScope {
        root_id: contract.root_id.clone(),
        question: scope.clone(),
        authority_ref: contract.authority_ref.clone(),
    };
    let directory = tempfile::tempdir().expect("isolated qualification journal");
    let database = directory.path().join("round20.sqlite3");
    let mut connection = Connection::open(&database).expect("open SQLite journal");
    initialize(&connection).expect("initialize P05 durable schemas");

    let worker = PrincipalId::new("worker").expect("valid worker");
    let reviewer = PrincipalId::new("reviewer").expect("valid reviewer");
    let opening = QuestionEvent::Open {
        event_id: id("round20-open-event"),
        scope: scope.clone(),
        authority_ref: journal_scope.authority_ref.clone(),
        asker: PrincipalId::new("captain").expect("valid asker"),
        recipient: worker.clone(),
        allowed_recipients: vec![worker.clone(), reviewer.clone()],
        requirement: format!("Check contract digest {contract_digest}"),
        question: contract.objective.clone(),
        options: vec!["hold".into(), "escalate".into()],
        deadline_utc: Some(2_000),
    };
    let opened = append(
        &mut connection,
        &journal_scope,
        &id("round20-open-command"),
        &opening,
    )
    .expect("persist immutable question event");
    let open_replay = append(
        &mut connection,
        &journal_scope,
        &id("round20-open-retry"),
        &opening,
    )
    .expect("exact retry recovers the original receipt");
    assert!(open_replay.duplicate);
    assert_eq!(open_replay.revision, opened.revision);

    let route = QuestionEvent::Route {
        event_id: id("round20-route-event"),
        scope: scope.clone(),
        expected_revision: 1,
        authority_ref: journal_scope.authority_ref.clone(),
        actor: worker,
        recipient: reviewer.clone(),
    };
    let routed = append(
        &mut connection,
        &journal_scope,
        &id("round20-route-command"),
        &route,
    )
    .expect("authorized P05 routing advances exact scope");
    let expiry = QuestionEvent::EvaluateDeadline {
        event_id: id("round20-expiry-event"),
        scope: scope.clone(),
        expected_revision: 2,
        authority_ref: journal_scope.authority_ref.clone(),
        actor: reviewer,
        observed_utc: Some(2_001),
    };
    let expired = append(
        &mut connection,
        &journal_scope,
        &id("round20-expiry-command"),
        &expiry,
    )
    .expect("expired review creates an escalation inbox item");
    let escalation_ref = expired
        .escalation_ref
        .clone()
        .expect("escalation identity exists");
    let expiry_replay = append(
        &mut connection,
        &journal_scope,
        &id("round20-expiry-retry"),
        &expiry,
    )
    .expect("exact expiry replay is idempotent");
    assert!(expiry_replay.duplicate);
    assert_eq!(
        expiry_replay.escalation_ref.as_deref(),
        Some(escalation_ref.as_str())
    );
    assert_eq!(routed.revision, "2");

    let proposal: String = connection
        .query_row(
            "SELECT proposal_json FROM durable_question_escalations WHERE question_id=?1",
            [scope.question_id.as_str()],
            |row| row.get(0),
        )
        .expect("release audit can read durable inbox bundle");
    let proposal: serde_json::Value = serde_json::from_str(&proposal).expect("valid proposal JSON");
    for (key, expected) in [
        ("project_id", contract.project_id.as_str()),
        ("root_id", contract.root_id.as_str()),
        ("task_id", contract.task_id.as_str()),
        ("question_id", scope.question_id.as_str()),
        ("authority_ref", contract.authority_ref.as_str()),
    ] {
        assert_eq!(proposal[key], expected);
    }
    assert!(proposal.get("release_granted").is_none());
    assert!(proposal.get("acceptance_granted").is_none());

    let policy = VerificationPolicy {
        schema_version: 1,
        verifier: PrincipalId::new("independent-reviewer").expect("valid verifier"),
        protected_oracle_digest: format!("sha256:{}", "a".repeat(64)),
        required_negative_checks: vec![RequiredNegativeCheck {
            check: CheckSpec {
                id: "round20-negative-check".into(),
                program: "fixture".into(),
                args: vec![],
                cwd: ".".into(),
                timeout_secs: 10,
            },
            expected_exit_code: 1,
            fixture_digest: format!("sha256:{}", "e".repeat(64)),
        }],
    };
    let inputs = VerificationInputs {
        project_id: contract.project_id.clone(),
        root_id: contract.root_id.clone(),
        task_id: contract.task_id.clone(),
        attempt_id: AttemptId::new("round20-attempt").expect("valid attempt"),
        candidate_ref: "candidate:round20".into(),
        contract_revision: contract.revision,
        contract_digest,
        verification_policy_digest: content_digest(&policy).expect("policy digest"),
        source_commit_sha: "1".repeat(40),
        source_tree_sha: "2".repeat(40),
        source_artifact_digest: format!("sha256:{}", "b".repeat(64)),
        protected_oracle_digest: policy.protected_oracle_digest.clone(),
    };
    let receipt = VerificationReceipt {
        schema_version: 1,
        receipt_id: "round20-post-pr57".into(),
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
                stdout_digest: format!("sha256:{}", "c".repeat(64)),
                stderr_digest: format!("sha256:{}", "d".repeat(64)),
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
    let bytes = serde_json::to_vec(&receipt).expect("encode portable P15 receipt");
    let parsed = parse_receipt(&bytes).expect("strict receipt schema roundtrips");
    let validated = validate_receipt(&contract, &inputs, &policy, &parsed)
        .expect("P15 evidence agrees with exact P02 contract and policy");
    assert!(validated.all_checks_passed());
    assert!(!validated.acceptance_granted());
    assert!(!validated.verifier_authenticated());
    assert!(!validated.execution_qualified());
    assert!(!validated.evidence_custody_verified());

    let mut mismatched_inputs = inputs;
    mismatched_inputs.task_id = TaskId::new("other-task").expect("valid changed task ID");
    assert!(validate_receipt(&contract, &mismatched_inputs, &policy, &parsed).is_err());

    drop(connection);
    let reopened = Connection::open(&database).expect("reopen qualification journal");
    let recovered = reconstruct(&reopened, &journal_scope)
        .expect("audit replay validates digest chain")
        .expect("released question remains retained");
    assert_eq!(
        recovered.state.status,
        QuestionStatus::Expired {
            observed_utc: 2_001
        }
    );
    assert_eq!(recovered.event_count, "3");
    assert_eq!(
        recovered.escalation_ref.as_deref(),
        Some(escalation_ref.as_str())
    );
}
