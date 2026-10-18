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

#[test]
fn p09_p16_round9_item3_workspace_doctor_qualification() {
    use admiralty_core::{
        ProjectId, RootId, TaskId,
        conflicts::{CandidateChange, ConflictRisk, FileChange, analyze_conflicts},
        economics::DecimalU128,
        placement::{
            BudgetRequirement, DecimalU64 as PlacementCounter, PlacementError, PlacementPolicy,
            PlacementRequest,
        },
    };
    use admiralty_runtime::{
        audits::decode_evidence,
        recovery::DecimalU64 as RecoveryCounter,
        session::{LaunchSpec, validate_launch_envelope},
        workspace::registry::{NodeRef, WorkspaceSpec},
    };
    use admiralty_store::artifacts::ArtifactStore;
    use std::{fs, path::PathBuf};

    let tmp = tempfile::tempdir().expect("qualification root");
    let workspace = fs::canonicalize(tmp.path()).expect("canonical fixture");

    // P09: root registration is observational and cannot adopt traversal scopes.
    let invalid_workspace = WorkspaceSpec {
        node_id: NodeRef::new("doctor-node").expect("node ID"),
        root_id: RootId::new("doctor-root").expect("root ID"),
        project_id: ProjectId::new("doctor-project").expect("project ID"),
        workspace: workspace.clone(),
        owned_paths: vec![PathBuf::from("../outside")],
        forbidden_paths: vec![],
    };
    assert!(invalid_workspace.validate().is_err());

    // P16: worker JSON cannot smuggle receipt flags or unknown evidence fields.
    assert!(decode_evidence(br#"{"unknown":true}"#).is_err());

    // P04: immutable content-addressed evidence refuses to bless modified bytes.
    let object_root = workspace.join("objects");
    fs::create_dir(&object_root).expect("object root");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&object_root, fs::Permissions::from_mode(0o700))
            .expect("private object root");
    }
    let objects = ArtifactStore::open(&object_root, 1024).expect("artifact store");
    let reference = objects
        .publish_bytes(b"verified receipt bytes")
        .expect("publish object");
    let hex = reference
        .sha256
        .strip_prefix("sha256:")
        .expect("digest prefix");
    fs::write(object_root.join(hex), b"modified bytes").expect("tamper fixture object");
    assert!(objects.publish_bytes(b"verified receipt bytes").is_err());

    // P07: a symlinked spool root cannot be adopted as a daemon custody root.
    #[cfg(unix)]
    {
        use admiralty_runtime::session::daemon::{DaemonConfig, SessionDaemon};
        use std::os::unix::fs::symlink;
        let safe_target = workspace.join("safe-spool");
        fs::create_dir(&safe_target).expect("safe spool target");
        let linked = workspace.join("linked-spool");
        symlink(&safe_target, &linked).expect("spool symlink");
        assert!(
            SessionDaemon::bind(DaemonConfig {
                socket: workspace.join("doctor.sock"),
                spool_root: linked,
            })
            .is_err()
        );
    }

    // P18: missing ancestry remains an explicit conflict risk, never a clear graph.
    let candidate = |id: &str, path: &str| CandidateChange {
        project_id: ProjectId::new("doctor-project").expect("project ID"),
        root_id: RootId::new("doctor-root").expect("root ID"),
        task_id: TaskId::new(format!("task-{id}")).expect("task ID"),
        candidate_id: id.into(),
        base_sha: "1".repeat(40),
        head_sha: "2".repeat(40),
        tree_sha: "3".repeat(40),
        ancestry_digest: None,
        files: vec![FileChange {
            path: path.into(),
            hunks: Some(vec![]),
        }],
        constraints: None,
    };
    let graph = analyze_conflicts(
        &[
            candidate("candidate-a", "src/lib.rs"),
            candidate("candidate-b", "src/lib.rs"),
        ],
        "doctor-policy",
    )
    .expect("bounded conflict graph");
    assert!(graph.edges.iter().any(|edge| {
        edge.risks.contains(&ConflictRisk::UnknownAncestry)
            && edge.risks.contains(&ConflictRisk::SamePathUnknownHunks)
    }));
    assert!(graph.generation.combined_verification_required);

    // P21: literal argv rejects NUL before any process can be launched.
    let launch = LaunchSpec {
        run_id: "doctor-run".into(),
        program: PathBuf::from("/bin/true"),
        args: vec!["bad\0arg".into()],
        cwd: workspace.clone(),
        spool_dir: workspace.clone(),
    };
    assert!(validate_launch_envelope(&launch).is_err());

    // P24: missing clock input fails closed; no placement is inferred from unknown time.
    let placement_request = PlacementRequest {
        project: ProjectId::new("doctor-project").expect("project ID"),
        root: RootId::new("doctor-root").expect("root ID"),
        task: TaskId::new("doctor-task").expect("task ID"),
        contract_digest: format!("sha256:{}", "a".repeat(64)),
        authority_ref: "fixture-authority".into(),
        required_capabilities: vec![],
        locality_refs: vec![],
        budget_requirement: BudgetRequirement {
            account_pool: "fixture-pool".into(),
            amount: PlacementCounter::new(1),
        },
    };
    let placement_policy = PlacementPolicy {
        max_observation_age: PlacementCounter::new(10),
        maximum_pressure: PlacementCounter::new(10),
        minimum_spool_reserve: PlacementCounter::new(1),
        disconnected_continuation_only: true,
        ownership_revision: None,
    };
    let placement: Result<_, PlacementError> =
        admiralty_core::placement::propose_placement_recorded(
            &placement_request,
            &[],
            &placement_policy,
            None,
            None,
        );
    assert!(matches!(placement, Err(PlacementError::Clock)));

    // P25: recovery counters reject noncanonical decimals (e.g. leading zero).
    assert!(serde_json::from_str::<RecoveryCounter>("\"01\"").is_err());

    // P26: large usage totals remain exact across JSON and never round through f64.
    let large_usage = DecimalU128(9_007_199_254_740_993);
    assert_eq!(
        serde_json::to_string(&large_usage).expect("exact usage JSON"),
        "\"9007199254740993\""
    );
    assert_eq!(
        serde_json::from_str::<DecimalU128>("\"9007199254740993\"")
            .expect("exact usage parse")
            .as_u128(),
        9_007_199_254_740_993
    );
}
