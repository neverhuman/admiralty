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

fn message(value: &str) -> MessageId {
    MessageId::new(value).expect("static message identity is valid")
}

fn smoke_package() -> (Vec<u8>, String) {
    let contract = TaskContract {
        schema_version: 1,
        project_id: ProjectId::new("round22-project").expect("valid project ID"),
        root_id: RootId::new("round22-root").expect("valid root ID"),
        task_id: TaskId::new("round22-task").expect("valid task ID"),
        revision: 22,
        title: "Canary release package qualification".into(),
        objective: "Bind canary and release evidence to one immutable task scope".into(),
        non_goals: vec!["automatic release authorization".into()],
        owned_paths: vec!["crates/admiralty-testkit".into()],
        forbidden_paths: vec!["secrets".into()],
        read_first: vec![],
        dependencies: vec![],
        criteria: vec![AcceptanceCriterion {
            id: "canary-bundle".into(),
            text: "Release package binds the exact question and receipt".into(),
            check_ids: vec!["canary-smoke".into()],
        }],
        checks: vec![CheckSpec {
            id: "canary-smoke".into(),
            program: "canary-smoke".into(),
            args: vec!["--self-test".into()],
            cwd: ".".into(),
            timeout_secs: 45,
        }],
        authority_ref: "authority:round22".into(),
        max_attempts: 1,
        simulated: false,
    };
    contract.validate().expect("P02 contract validates");
    let contract_digest = content_digest(&contract).expect("canonical P02 digest");
    let question = QuestionScope {
        question_id: message("round22-question"),
        project_id: contract.project_id.clone(),
        task_id: contract.task_id.clone(),
    };
    let journal = JournalScope {
        root_id: contract.root_id.clone(),
        question: question.clone(),
        authority_ref: contract.authority_ref.clone(),
    };
    let mut connection = Connection::open_in_memory().expect("hermetic SQLite journal");
    initialize(&connection).expect("initialize P05 journal and escalation schema");
    let worker = PrincipalId::new("worker").expect("valid worker ID");
    let reviewer = PrincipalId::new("reviewer").expect("valid reviewer ID");
    let opening = QuestionEvent::Open {
        event_id: message("round22-open-event"),
        scope: question.clone(),
        authority_ref: journal.authority_ref.clone(),
        asker: PrincipalId::new("captain").expect("valid asker ID"),
        recipient: worker.clone(),
        allowed_recipients: vec![worker.clone(), reviewer.clone()],
        requirement: format!("Canary release for contract {contract_digest}"),
        question: contract.objective.clone(),
        options: vec!["hold".into(), "review".into()],
        deadline_utc: Some(4_000),
    };
    append(
        &mut connection,
        &journal,
        &message("round22-open-command"),
        &opening,
    )
    .expect("persist P05 question");
    let route = QuestionEvent::Route {
        event_id: message("round22-route-event"),
        scope: question.clone(),
        expected_revision: 1,
        authority_ref: journal.authority_ref.clone(),
        actor: worker,
        recipient: reviewer.clone(),
    };
    let routed = append(
        &mut connection,
        &journal,
        &message("round22-route-command"),
        &route,
    )
    .expect("route to authorized reviewer");
    assert_eq!(routed.revision, "2");
    let expiry = QuestionEvent::EvaluateDeadline {
        event_id: message("round22-expiry-event"),
        scope: question.clone(),
        expected_revision: 2,
        authority_ref: journal.authority_ref.clone(),
        actor: reviewer,
        observed_utc: Some(4_001),
    };
    let expired = append(
        &mut connection,
        &journal,
        &message("round22-expiry-command"),
        &expiry,
    )
    .expect("expired canary review creates Escalation Inbox item");
    let escalation_ref = expired.escalation_ref.expect("escalation ref is retained");
    let proposal_json: String = connection
        .query_row(
            "SELECT proposal_json FROM durable_question_escalations WHERE question_id=?1",
            [question.question_id.as_str()],
            |row| row.get(0),
        )
        .expect("read release escalation proposal");
    let proposal: serde_json::Value =
        serde_json::from_str(&proposal_json).expect("proposal schema is valid");
    assert_eq!(proposal["project_id"], contract.project_id.as_str());
    assert_eq!(proposal["root_id"], contract.root_id.as_str());
    assert_eq!(proposal["task_id"], contract.task_id.as_str());
    assert_eq!(proposal["question_id"], question.question_id.as_str());
    assert!(proposal.get("acceptance_granted").is_none());
    assert!(proposal.get("release_granted").is_none());

    let policy = VerificationPolicy {
        schema_version: 1,
        verifier: PrincipalId::new("independent-reviewer").expect("valid verifier ID"),
        protected_oracle_digest: format!("sha256:{}", "a".repeat(64)),
        required_negative_checks: vec![RequiredNegativeCheck {
            check: CheckSpec {
                id: "reject-invalid-package".into(),
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
        attempt_id: AttemptId::new("round22-attempt").expect("valid attempt ID"),
        candidate_ref: "candidate:round22".into(),
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
        receipt_id: "round22-canary-receipt".into(),
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
    let receipt_bytes = serde_json::to_vec(&receipt).expect("encode portable P15 receipt");
    let parsed = parse_receipt(&receipt_bytes).expect("receipt schema accepts canary evidence");
    let validated = validate_receipt(&contract, &inputs, &policy, &parsed)
        .expect("P15 evidence agrees with P02 contract and policy");
    assert!(validated.all_checks_passed());
    assert!(!validated.acceptance_granted());
    assert!(!validated.verifier_authenticated());
    assert!(!validated.execution_qualified());
    assert!(!validated.evidence_custody_verified());

    let package = serde_json::json!({
        "schema_version": 1,
        "contract_digest": content_digest(&contract).expect("contract digest"),
        "question_id": question.question_id.as_str(),
        "escalation_ref": escalation_ref,
        "proposal_digest": content_digest(&proposal).expect("proposal digest"),
        "receipt_digest": content_digest(&receipt_bytes).expect("receipt digest"),
    });
    let package_bytes = serde_json::to_vec(&package).expect("encode deterministic release package");
    let package_digest = content_digest(&package).expect("release package digest");
    assert_eq!(package["schema_version"], 1);
    assert!(package.get("release_granted").is_none());

    let recovered = reconstruct(&connection, &journal)
        .expect("audit P05 event chain")
        .expect("Escalation Inbox retains expired question");
    assert_eq!(
        recovered.state.status,
        QuestionStatus::Expired {
            observed_utc: 4_001
        }
    );
    assert_eq!(recovered.event_count, "3");
    assert_eq!(
        recovered.escalation_ref.as_deref(),
        Some(escalation_ref.as_str())
    );
    (package_bytes, package_digest)
}

#[test]
fn codex_round22_p05_p15_canary_smoke_release_package_qualification() {
    let first = smoke_package();
    let replay = smoke_package();
    assert_eq!(
        first, replay,
        "independent smoke runs package identical evidence"
    );
}
