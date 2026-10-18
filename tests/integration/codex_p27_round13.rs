//! Round 13 P27 work-order provenance and release-evidence invariants.

use admiralty_core::{
    AcceptanceCriterion, AttemptId, CheckSpec, PrincipalId, ProjectId, RootId, TaskContract,
    TaskId, content_digest,
    excursions::{
        EvidenceRef, ExcursionScope, IncidentEvent, ReproductionContract, ScopedIncident,
        build_local_proposal, reduce_excursions,
    },
    verification::{
        CheckResult as ReceiptCheck, ReceiptProducer, ReceiptRegistry, RequiredNegativeCheck,
        VerificationInputs, VerificationPolicy, VerificationReceipt, parse_receipt,
    },
};
use std::collections::BTreeMap;

fn digest(byte: char) -> String {
    format!("sha256:{}", byte.to_string().repeat(64))
}

fn release_fixture() -> (
    TaskContract,
    VerificationInputs,
    VerificationPolicy,
    VerificationReceipt,
) {
    let project_id = ProjectId::new("round13-project").expect("project fixture");
    let root_id = RootId::new("round13-root").expect("root fixture");
    let task_id = TaskId::new("round13-task").expect("task fixture");
    let check = CheckSpec {
        id: "unit".into(),
        program: "cargo".into(),
        args: vec!["test".into(), "-p".into(), "admiralty-testkit".into()],
        cwd: ".".into(),
        timeout_secs: 60,
    };
    let contract = TaskContract {
        schema_version: 1,
        project_id: project_id.clone(),
        root_id: root_id.clone(),
        task_id: task_id.clone(),
        revision: 4,
        title: "review P27 proposal evidence".into(),
        objective: "validate exact work-order and release evidence".into(),
        non_goals: vec!["do not activate execution".into()],
        owned_paths: vec!["crates/admiralty-core/src/excursions.rs".into()],
        forbidden_paths: vec!["secrets".into()],
        read_first: vec!["docs/plans/admiralty-v2/WORK_ORDER_REVIEW.md".into()],
        dependencies: vec![],
        criteria: vec![AcceptanceCriterion {
            id: "C1".into(),
            text: "evidence binds to this exact contract and candidate".into(),
            check_ids: vec!["unit".into()],
        }],
        checks: vec![check.clone()],
        authority_ref: "grant:round13-verification".into(),
        max_attempts: 1,
        simulated: false,
    };
    let verifier = PrincipalId::new("round13-verifier").expect("verifier fixture");
    let policy = VerificationPolicy {
        schema_version: 1,
        verifier: verifier.clone(),
        protected_oracle_digest: digest('b'),
        required_negative_checks: vec![RequiredNegativeCheck {
            check: CheckSpec {
                id: "negative-scope".into(),
                program: "fixture-verifier".into(),
                args: vec!["--reject-foreign-scope".into()],
                cwd: ".".into(),
                timeout_secs: 5,
            },
            expected_exit_code: 1,
            fixture_digest: digest('c'),
        }],
    };
    let proposal_artifact_digest = digest('d');
    let inputs = VerificationInputs {
        project_id,
        root_id,
        task_id,
        attempt_id: AttemptId::new("round13-attempt").expect("attempt fixture"),
        candidate_ref: "candidate:p27-local-proposal-r4".into(),
        contract_revision: contract.revision,
        contract_digest: content_digest(&contract).expect("contract digest"),
        verification_policy_digest: content_digest(&policy).expect("policy digest"),
        source_commit_sha: "1".repeat(40),
        source_tree_sha: "2".repeat(40),
        source_artifact_digest: proposal_artifact_digest,
        protected_oracle_digest: policy.protected_oracle_digest.clone(),
    };
    let receipt = VerificationReceipt {
        schema_version: 1,
        receipt_id: "round13-receipt-1".into(),
        producer: ReceiptProducer::IndependentVerifier,
        verifier,
        inputs: inputs.clone(),
        checks: vec![ReceiptCheck {
            check,
            exit_code: Some(0),
            stdout_digest: digest('e'),
            stderr_digest: digest('f'),
        }],
        negative_checks: policy
            .required_negative_checks
            .iter()
            .map(|required| ReceiptCheck {
                check: required.check.clone(),
                exit_code: Some(required.expected_exit_code),
                stdout_digest: digest('e'),
                stderr_digest: digest('f'),
            })
            .collect(),
    };
    (contract, inputs, policy, receipt)
}

#[test]
fn p27_round13_wo_export_provenance_audit() {
    let (contract, mut expected, policy, mut receipt) = release_fixture();
    let scope = ExcursionScope {
        project: ProjectId::new("round13-project").expect("project fixture"),
        root: RootId::new("round13-root").expect("root fixture"),
        task: TaskId::new("round13-task").expect("task fixture"),
        contract_revision: contract.revision,
        contract_digest: content_digest(&contract).expect("contract digest"),
        protected_oracle_digest: policy.protected_oracle_digest.clone(),
    };
    let incident = ScopedIncident {
        scope: scope.clone(),
        event: IncidentEvent {
            event_id: "round13-incident".into(),
            family_signature: "work-order-release-check".into(),
            cause_parent: None,
            kind: "verification-needed".into(),
            evidence_digests: vec![digest('c')],
            observed_tick: Some("13".into()),
        },
    };
    let state = reduce_excursions(&None, &scope, &[incident]).expect("scoped P27 event");
    let proposal = build_local_proposal(
        &state.episodes[0],
        &ReproductionContract {
            fields: BTreeMap::from([
                ("scope".into(), "round13-task; revision 4".into()),
                (
                    "steps".into(),
                    "inspect the pinned work-order evidence".into(),
                ),
                (
                    "expected".into(),
                    "proposal retains exact task scope".into(),
                ),
                (
                    "actual".into(),
                    "independent release check is pending".into(),
                ),
                (
                    "negation".into(),
                    "do not equate proposal with acceptance".into(),
                ),
            ]),
        },
        &[EvidenceRef {
            digest: digest('c'),
            media_type: "application/json".into(),
        }],
    )
    .expect("local proposal from canonical episode");
    assert_eq!(proposal.scope, scope);
    assert!(!proposal.execution_granted);
    assert!(!proposal.external_publication_allowed);
    expected.source_artifact_digest =
        content_digest(&proposal).expect("P27 proposal artifact digest");
    receipt.inputs = expected.clone();

    // Release evidence is bound to exact work-order revision, candidate, tree,
    // verifier policy, and proposal artifact; passing checks alone confer no
    // authenticated verifier, custody, execution, or acceptance authority.
    let serialized = serde_json::to_vec(&receipt).expect("receipt serializes");
    let parsed = parse_receipt(&serialized).expect("bounded release receipt parses");
    let mut registry = ReceiptRegistry::new(2).expect("bounded receipt registry");
    let first = registry
        .validate_and_record(&contract, &expected, &policy, &parsed)
        .expect("exact independent receipt validates");
    assert!(first.result.all_checks_passed());
    assert!(!first.result.acceptance_granted());
    assert!(!first.result.verifier_authenticated());
    assert!(!first.result.evidence_custody_verified());
    assert!(!first.result.execution_qualified());
    let replay = registry
        .validate_and_record(&contract, &expected, &policy, &parsed)
        .expect("exact receipt replay is idempotent");
    assert!(replay.replayed);
    assert_eq!(
        replay.result.receipt_digest(),
        first.result.receipt_digest()
    );

    let mut provider_claim = parsed.clone();
    provider_claim.producer = ReceiptProducer::ProviderClaim;
    assert!(
        registry
            .validate_and_record(&contract, &expected, &policy, &provider_claim)
            .is_err()
    );

    let mut stale_candidate = expected.clone();
    stale_candidate.source_commit_sha = "3".repeat(40);
    assert!(
        registry
            .validate_and_record(&contract, &stale_candidate, &policy, &parsed)
            .is_err()
    );

    let mut injected = serde_json::to_value(&parsed).expect("receipt value");
    injected["acceptance_granted"] = serde_json::Value::Bool(true);
    assert!(parse_receipt(&serde_json::to_vec(&injected).expect("injected JSON")).is_err());

    let mut collision = parsed;
    collision.checks[0].stdout_digest = digest('9');
    assert!(
        registry
            .validate_and_record(&contract, &expected, &policy, &collision)
            .is_err()
    );
}
