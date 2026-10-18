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

#[derive(Debug, PartialEq, Eq)]
struct BundleFingerprint {
    proposal_json: String,
    escalation_ref: String,
    receipt_json: Vec<u8>,
    final_revision: u64,
    event_count: String,
}

fn message_id(value: &str) -> MessageId {
    MessageId::new(value).expect("fixed message identity is valid")
}

fn run_hermetic_release_flow() -> BundleFingerprint {
    let contract = TaskContract {
        schema_version: 1,
        project_id: ProjectId::new("round21-project").expect("valid project ID"),
        root_id: RootId::new("round21-root").expect("valid root ID"),
        task_id: TaskId::new("round21-task").expect("valid task ID"),
        revision: 21,
        title: "Hermetic release qualification".into(),
        objective: "Bind question routing and verification to this exact contract".into(),
        non_goals: vec!["automatic release approval".into()],
        owned_paths: vec!["crates/admiralty-testkit".into()],
        forbidden_paths: vec!["secrets".into()],
        read_first: vec![],
        dependencies: vec![],
        criteria: vec![AcceptanceCriterion {
            id: "cross-packet-scope".into(),
            text: "P02, P05, and P15 retain matching identities".into(),
            check_ids: vec!["round21-audit".into()],
        }],
        checks: vec![CheckSpec {
            id: "round21-audit".into(),
            program: "cargo".into(),
            args: vec!["test".into()],
            cwd: ".".into(),
            timeout_secs: 60,
        }],
        authority_ref: "authority:round21".into(),
        max_attempts: 1,
        simulated: false,
    };
    contract.validate().expect("P02 contract validates");
    let contract_digest = content_digest(&contract).expect("stable contract digest");
    let question = QuestionScope {
        question_id: message_id("round21-question"),
        project_id: contract.project_id.clone(),
        task_id: contract.task_id.clone(),
    };
    let journal_scope = JournalScope {
        root_id: contract.root_id.clone(),
        question: question.clone(),
        authority_ref: contract.authority_ref.clone(),
    };

    // Each run gets an independent temporary directory; no host path enters the bundle.
    let directory = tempfile::tempdir().expect("private hermetic fixture directory");
    let database = directory.path().join("release.sqlite3");
    let mut connection = Connection::open(&database).expect("open isolated SQLite journal");
    initialize(&connection).expect("initialize durable question and escalation schemas");
    let worker = PrincipalId::new("worker").expect("valid worker ID");
    let reviewer = PrincipalId::new("reviewer").expect("valid reviewer ID");
    let opening = QuestionEvent::Open {
        event_id: message_id("round21-open-event"),
        scope: question.clone(),
        authority_ref: journal_scope.authority_ref.clone(),
        asker: PrincipalId::new("captain").expect("valid asker ID"),
        recipient: worker.clone(),
        allowed_recipients: vec![worker.clone(), reviewer.clone()],
        requirement: format!("Audit contract digest {contract_digest}"),
        question: contract.objective.clone(),
        options: vec!["hold".into(), "escalate".into()],
        deadline_utc: Some(3_000),
    };
    let open = append(
        &mut connection,
        &journal_scope,
        &message_id("round21-open-command"),
        &opening,
    )
    .expect("persist scoped question");
    let route = QuestionEvent::Route {
        event_id: message_id("round21-route-event"),
        scope: question.clone(),
        expected_revision: 1,
        authority_ref: journal_scope.authority_ref.clone(),
        actor: worker,
        recipient: reviewer.clone(),
    };
    let routed = append(
        &mut connection,
        &journal_scope,
        &message_id("round21-route-command"),
        &route,
    )
    .expect("route to allowlisted reviewer");
    assert_eq!(open.revision, "1");
    assert_eq!(routed.revision, "2");
    let expiry = QuestionEvent::EvaluateDeadline {
        event_id: message_id("round21-expiry-event"),
        scope: question.clone(),
        expected_revision: 2,
        authority_ref: journal_scope.authority_ref.clone(),
        actor: reviewer,
        observed_utc: Some(3_001),
    };
    let expired = append(
        &mut connection,
        &journal_scope,
        &message_id("round21-expiry-command"),
        &expiry,
    )
    .expect("deadline creates a durable escalation");
    let escalation_ref = expired
        .escalation_ref
        .expect("escalation has a stable identity");
    let proposal_json: String = connection
        .query_row(
            "SELECT proposal_json FROM durable_question_escalations WHERE question_id=?1",
            [question.question_id.as_str()],
            |row| row.get(0),
        )
        .expect("read durable inbox proposal");
    let proposal: serde_json::Value =
        serde_json::from_str(&proposal_json).expect("proposal schema is valid JSON");
    for (key, expected) in [
        ("project_id", contract.project_id.as_str()),
        ("root_id", contract.root_id.as_str()),
        ("task_id", contract.task_id.as_str()),
        ("question_id", question.question_id.as_str()),
        ("authority_ref", contract.authority_ref.as_str()),
    ] {
        assert_eq!(proposal[key], expected);
    }
    assert!(proposal.get("acceptance_granted").is_none());
    assert!(proposal.get("release_granted").is_none());

    let policy = VerificationPolicy {
        schema_version: 1,
        verifier: PrincipalId::new("independent-reviewer").expect("valid verifier ID"),
        protected_oracle_digest: format!("sha256:{}", "a".repeat(64)),
        required_negative_checks: vec![RequiredNegativeCheck {
            check: CheckSpec {
                id: "round21-negative-check".into(),
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
        attempt_id: AttemptId::new("round21-attempt").expect("valid attempt ID"),
        candidate_ref: "candidate:round21".into(),
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
        receipt_id: "round21-hermetic-receipt".into(),
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
    let receipt_json = serde_json::to_vec(&receipt).expect("serialize portable receipt");
    let parsed = parse_receipt(&receipt_json).expect("strict receipt schema accepts bundle");
    let validated = validate_receipt(&contract, &inputs, &policy, &parsed)
        .expect("P15 evidence matches the exact P02 contract");
    assert!(validated.all_checks_passed());
    assert!(!validated.acceptance_granted());
    assert!(!validated.verifier_authenticated());
    assert!(!validated.execution_qualified());
    assert!(!validated.evidence_custody_verified());

    drop(connection);
    let reopened = Connection::open(database).expect("reopen durable bundle");
    let recovered = reconstruct(&reopened, &journal_scope)
        .expect("replay validates journal digest chain")
        .expect("question remains in escalation inbox history");
    assert_eq!(
        recovered.state.status,
        QuestionStatus::Expired {
            observed_utc: 3_001
        }
    );
    assert_eq!(recovered.state.revision, 3);
    assert_eq!(recovered.event_count, "3");
    assert_eq!(
        recovered.escalation_ref.as_deref(),
        Some(escalation_ref.as_str())
    );

    BundleFingerprint {
        proposal_json,
        escalation_ref,
        receipt_json,
        final_revision: recovered.state.revision,
        event_count: recovered.event_count,
    }
}

#[test]
fn codex_round21_p05_p15_hermetic_ci_cross_packet_release_qualification() {
    let first = run_hermetic_release_flow();
    let replay = run_hermetic_release_flow();
    assert_eq!(
        first, replay,
        "independent hermetic runs produce identical bundles"
    );
}
