use admiralty_core::{
    CheckSpec, MessageId, PrincipalId, ProjectId, RootId, TaskId, content_digest,
};
use admiralty_runtime::audits::{
    AuditOutcome, AuditPolicy, AuditReplayReceipt, AuditScope, CheckOutcome, CheckResult,
    DecimalU64, FindingSeverity, NegativeAuditCheck, StagedAuditEvidence, StagedAuditIdentity,
    TreeSource, evaluate_staged_audit,
};

fn digest(ch: char) -> String {
    format!("sha256:{}", ch.to_string().repeat(64))
}

fn scope() -> AuditScope {
    AuditScope {
        project_id: ProjectId::new("project").expect("project ID"),
        root_id: RootId::new("root").expect("root ID"),
        task_id: TaskId::new("task").expect("task ID"),
        contract_digest: digest('a'),
    }
}

fn check(id: &str, args: Vec<String>) -> CheckSpec {
    CheckSpec {
        id: id.into(),
        program: "cargo".into(),
        args,
        cwd: ".".into(),
        timeout_secs: 60,
    }
}

fn policy() -> AuditPolicy {
    let mut policy = AuditPolicy {
        required_identity: StagedAuditIdentity {
            base_sha: "1".repeat(40),
            staged_tree_sha: "2".repeat(40),
            profile_digest: digest('b'),
            policy_digest: digest('c'),
            oracle_digest: digest('d'),
            auditor: PrincipalId::new("auditor").expect("auditor ID"),
            staged_content_digest: digest('e'),
            custody_digest: digest('f'),
        },
        required_paths: vec!["src/lib.rs".into()],
        hard_findings: vec!["secret".into()],
        required_checks: vec![check("unit", vec!["test".into(), "--locked".into()])],
        required_affected_checks: vec!["unit".into()],
        negative_checks: vec![NegativeAuditCheck {
            check: check("negative", vec!["test".into(), "--negative".into()]),
            expected_exit_code: 1,
        }],
        minimum_score: Some(DecimalU64::new(80)),
        prior_audit: None,
    };
    policy.required_identity.policy_digest = content_digest(&(
        admiralty_runtime::audits::AUDIT_POLICY_VERSION,
        &policy.required_paths,
        &policy.hard_findings,
        &policy.required_checks,
        &policy.required_affected_checks,
        &policy.negative_checks,
        &policy.minimum_score,
    ))
    .expect("policy digest");
    policy
}

fn evidence(policy: &AuditPolicy) -> StagedAuditEvidence {
    let scope = scope();
    let identity = policy.required_identity.clone();
    StagedAuditEvidence {
        audit_id: MessageId::new("audit-one").expect("audit ID"),
        scope: scope.clone(),
        identity: identity.clone(),
        staged_content_digest: identity.staged_content_digest.clone(),
        scope_coverage: vec!["src/lib.rs".into()],
        coverage_complete: true,
        full_context_available: true,
        profile_supported: true,
        tree_source: TreeSource::StagedIndex,
        index_matches_staged_tree: true,
        score: Some(DecimalU64::new(90)),
        findings: Vec::<admiralty_runtime::audits::AuditFinding>::new(),
        check_results: vec![
            CheckResult {
                check_id: "unit".into(),
                outcome: CheckOutcome::Passed,
                exit_code: Some(0),
                evidence_ref: Some(digest('e')),
                executed_check: policy.required_checks[0].clone(),
                scope: scope.clone(),
                identity: identity.clone(),
            },
            CheckResult {
                check_id: "negative".into(),
                outcome: CheckOutcome::Passed,
                exit_code: Some(1),
                evidence_ref: Some(digest('e')),
                executed_check: policy.negative_checks[0].check.clone(),
                scope,
                identity,
            },
        ],
        evidence_refs: vec![digest('e')],
    }
}

#[test]
fn staged_index_identity_and_oracle_changes_fail_closed_without_acceptance_authority() {
    let policy = policy();
    let mut staged = evidence(&policy);
    let proposal = evaluate_staged_audit(&scope(), &policy, &staged).expect("staged evidence");
    assert!(matches!(
        proposal.outcome,
        AuditOutcome::PassProposal { .. }
    ));
    assert!(!proposal.acceptance_granted());
    assert!(!proposal.execution_granted());
    assert!(!proposal.publication_granted());

    staged.tree_source = TreeSource::WorkingTree;
    let proposal = evaluate_staged_audit(&scope(), &policy, &staged).expect("tree mismatch");
    assert!(matches!(proposal.outcome, AuditOutcome::Invalidated { .. }));

    staged = evidence(&policy);
    staged.index_matches_staged_tree = false;
    let proposal = evaluate_staged_audit(&scope(), &policy, &staged).expect("partial stage");
    assert!(matches!(proposal.outcome, AuditOutcome::Invalidated { .. }));

    staged = evidence(&policy);
    staged.identity.oracle_digest = digest('9');
    for result in &mut staged.check_results {
        result.identity = staged.identity.clone();
    }
    let proposal = evaluate_staged_audit(&scope(), &policy, &staged).expect("oracle change");
    assert!(matches!(proposal.outcome, AuditOutcome::Invalidated { .. }));

    staged = evidence(&policy);
    staged
        .findings
        .push(admiralty_runtime::audits::AuditFinding {
            id: "secret".into(),
            severity: FindingSeverity::Hard,
            detail: "protected finding".into(),
            evidence_ref: Some(digest('e')),
        });
    let proposal = evaluate_staged_audit(&scope(), &policy, &staged).expect("hard finding");
    assert!(matches!(
        proposal.outcome,
        AuditOutcome::HardFindings { .. }
    ));
    assert!(!proposal.acceptance_granted());
}

#[test]
fn negative_check_with_wrong_exit_code_requires_reproduction() {
    let policy = policy();
    let mut staged = evidence(&policy);
    let negative = staged
        .check_results
        .iter_mut()
        .find(|result| result.check_id == "negative")
        .expect("negative check result");
    negative.outcome = CheckOutcome::Failed;
    negative.exit_code = Some(0);

    let proposal = evaluate_staged_audit(&scope(), &policy, &staged).expect("well-formed evidence");
    match proposal.outcome {
        AuditOutcome::Reproduce { ref checks, .. } => {
            assert_eq!(checks.len(), 1);
            assert_eq!(checks[0].id, "negative");
        }
        other => panic!("unexpected outcome for wrong negative exit code: {other:?}"),
    }
    assert!(!proposal.acceptance_granted());
    assert!(!proposal.execution_granted());
    assert!(!proposal.publication_granted());

    let mut false_pass = evidence(&policy);
    false_pass
        .check_results
        .iter_mut()
        .find(|result| result.check_id == "negative")
        .expect("negative check result")
        .exit_code = Some(0);
    assert!(
        evaluate_staged_audit(&scope(), &policy, &false_pass).is_err(),
        "a passing result with the wrong negative-control exit code is rejected"
    );
}

#[test]
fn minimum_score_boundary_is_inclusive() {
    let policy = policy();
    let mut staged = evidence(&policy);
    staged.score = Some(DecimalU64::new(80));

    let proposal = evaluate_staged_audit(&scope(), &policy, &staged).expect("boundary evidence");
    assert!(matches!(
        proposal.outcome,
        AuditOutcome::PassProposal { .. }
    ));
    assert!(!proposal.acceptance_granted());
}

#[test]
fn exact_audit_replay_is_idempotent_and_changed_evidence_collides() {
    let mut policy = policy();
    let staged = evidence(&policy);
    let first = evaluate_staged_audit(&scope(), &policy, &staged).expect("initial evaluation");

    policy.prior_audit = Some(AuditReplayReceipt {
        audit_id: first.audit_id.clone(),
        evidence_digest: first.evidence_digest.clone(),
        outcome: first.outcome.clone(),
    });
    let replay = evaluate_staged_audit(&scope(), &policy, &staged).expect("exact replay");
    assert!(replay.replayed);
    assert_eq!(replay.evidence_digest, first.evidence_digest);
    assert_eq!(replay.outcome, first.outcome);

    let mut changed = staged;
    changed.score = Some(DecimalU64::new(89));
    assert!(
        evaluate_staged_audit(&scope(), &policy, &changed).is_err(),
        "same audit ID cannot replay different evidence"
    );
}
