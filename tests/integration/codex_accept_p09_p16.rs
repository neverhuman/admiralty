#[test]
fn p09_p16_round8_captain_orchestrator_receipt_qualification() {
    use admiralty_core::{
        CheckSpec, MessageId, PrincipalId, ProjectId, RootId, TaskId, content_digest,
    };
    use admiralty_runtime::{
        audits::{
            AuditOutcome, AuditPolicy, AuditRegistry, AuditScope, CheckOutcome, CheckResult,
            DecimalU64, FindingSeverity, NegativeAuditCheck, StagedAuditEvidence,
            StagedAuditIdentity, TreeSource,
        },
        workspace::registry::{Custody, NodeRef, WorkspaceSpec},
    };
    use std::{fs, path::PathBuf};

    let temp = tempfile::tempdir().expect("captain workspace fixture");
    let workspace = fs::canonicalize(temp.path()).expect("canonical workspace");
    let validated = WorkspaceSpec {
        node_id: NodeRef::new("captain-round8").expect("node ID"),
        root_id: RootId::new("root-round8").expect("root ID"),
        project_id: ProjectId::new("project-round8").expect("project ID"),
        workspace,
        owned_paths: vec![PathBuf::from("src")],
        forbidden_paths: vec![PathBuf::from(".git")],
    }
    .validate()
    .expect("P09 registered workspace validates");
    assert_eq!(validated.custody, Custody::RegisteredOnly);
    assert!(validated.owns(std::path::Path::new("src/lib.rs")));
    assert!(!validated.owns(std::path::Path::new(".git/config")));

    let d = |ch: char| format!("sha256:{}", ch.to_string().repeat(64));
    let scope = AuditScope {
        project_id: ProjectId::new("project-round8").expect("project ID"),
        root_id: RootId::new("root-round8").expect("root ID"),
        task_id: TaskId::new("task-round8").expect("task ID"),
        contract_digest: d('a'),
    };
    let check = CheckSpec {
        id: "unit".into(),
        program: "cargo".into(),
        args: vec!["test".into()],
        cwd: ".".into(),
        timeout_secs: 60,
    };
    let negative = CheckSpec {
        id: "negative".into(),
        args: vec!["test".into(), "--negative".into()],
        ..check.clone()
    };
    let mut policy = AuditPolicy {
        required_identity: StagedAuditIdentity {
            base_sha: "1".repeat(40),
            staged_tree_sha: "2".repeat(40),
            profile_digest: d('b'),
            policy_digest: d('c'),
            oracle_digest: d('d'),
            auditor: PrincipalId::new("captain-auditor").expect("auditor"),
            staged_content_digest: d('e'),
            custody_digest: d('f'),
        },
        required_paths: vec!["src/lib.rs".into()],
        hard_findings: vec!["unsafe".into()],
        required_checks: vec![check.clone()],
        required_affected_checks: vec!["unit".into()],
        negative_checks: vec![NegativeAuditCheck {
            check: negative.clone(),
            expected_exit_code: 1,
        }],
        minimum_score: Some(DecimalU64::new(80)),
        prior_audit: None,
    };
    policy.required_identity.policy_digest = content_digest(&(
        2_u32,
        &policy.required_paths,
        &policy.hard_findings,
        &policy.required_checks,
        &policy.required_affected_checks,
        &policy.negative_checks,
        &policy.minimum_score,
    ))
    .expect("canonical policy digest");

    let identity = policy.required_identity.clone();
    let evidence = StagedAuditEvidence {
        audit_id: MessageId::new("audit-round8").expect("audit ID"),
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
        findings: vec!["reviewed".to_string()]
            .into_iter()
            .map(|id| admiralty_runtime::audits::AuditFinding {
                id,
                severity: FindingSeverity::Info,
                detail: "bounded deterministic fixture".into(),
                evidence_ref: Some(d('e')),
            })
            .collect(),
        check_results: vec![
            CheckResult {
                check_id: "unit".into(),
                outcome: CheckOutcome::Passed,
                exit_code: Some(0),
                evidence_ref: Some(d('e')),
                executed_check: check,
                scope: scope.clone(),
                identity: identity.clone(),
            },
            CheckResult {
                check_id: "negative".into(),
                outcome: CheckOutcome::Passed,
                exit_code: Some(1),
                evidence_ref: Some(d('e')),
                executed_check: negative,
                scope: scope.clone(),
                identity,
            },
        ],
        evidence_refs: vec![d('e')],
    };

    let mut captain = AuditRegistry::default();
    let first = captain
        .evaluate(&scope, &policy, &evidence)
        .expect("captain evaluates evidence");
    assert!(matches!(first.outcome, AuditOutcome::PassProposal { .. }));
    assert!(!first.acceptance_granted());
    assert!(!first.execution_granted());
    assert!(!first.publication_granted());
    let replay = captain
        .evaluate(&scope, &policy, &evidence)
        .expect("exact receipt replay");
    assert!(replay.replayed);
    assert_eq!(replay.evidence_digest, first.evidence_digest);
    assert_eq!(captain.retained_audits(), 1);

    let mut changed = evidence;
    changed.score = Some(DecimalU64::new(91));
    assert!(captain.evaluate(&scope, &policy, &changed).is_err());
}
