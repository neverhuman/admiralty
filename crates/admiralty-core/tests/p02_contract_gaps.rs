use admiralty_core::{
    AcceptanceCriterion, AttemptId, CandidateDecisionEvidence, CheckSpec, DomainEvent,
    LifecycleAction, LifecycleCommand, PrincipalId, ProjectId, RootId, SchedulingState,
    TaskContract, TaskId,
    attempt_outcome::{AttemptOutcomeProjection, AttemptOutcomeStatus, TaskSchedulingDimension},
    content_digest,
    coordination::envelope::{Sha256Digest, TaskRevisionRef},
    execution_requirements::{
        EXECUTION_REQUIREMENTS_SCHEMA_VERSION, ExecutionRequirementsV2, ExecutionRunnerMode,
        ExecutionRunnerRequirements, RequiredDeliveryBoundary, ResponsibleRole,
        ShellBoundaryPolicy,
    },
    interface_compatibility::{
        ConsumerCompatibilityInput, ConsumerCompatibilityPolicy, ConsumerCompatibilityVerdict,
        ConsumerEvaluationStage, INTERFACE_COMPAT_SCHEMA_VERSION, InterfaceBindingStatus,
        TaskInterfaceContractV2,
    },
    interfaces::{DecimalU64 as InterfaceDecimalU64, InterfaceId},
    placement::{BudgetRequirement, DecimalU64 as PlacementDecimalU64},
};

fn sample_contract(task_id: &str, max_attempts: u16) -> TaskContract {
    TaskContract {
        schema_version: 1,
        project_id: ProjectId::new("admiralty").expect("project"),
        root_id: RootId::new("root-p02").expect("root"),
        task_id: TaskId::new(task_id).expect("task"),
        revision: 1,
        title: "Implement P02 contract closure".into(),
        objective: "Freeze independent attempt outcome and execution requirements".into(),
        non_goals: vec!["No live provider launch".into()],
        owned_paths: vec!["crates/admiralty-core/src/attempt_outcome.rs".into()],
        forbidden_paths: vec!["secrets/prod.env".into()],
        read_first: vec!["docs/plans/admiralty-v2/MASTER_PLAN.md".into()],
        dependencies: vec![],
        criteria: vec![AcceptanceCriterion {
            id: "C1".into(),
            text: "Unit and integration checks pass".into(),
            check_ids: vec!["T1".into()],
        }],
        checks: vec![CheckSpec {
            id: "T1".into(),
            program: "cargo".into(),
            args: vec!["test".into(), "-p".into(), "admiralty-core".into()],
            cwd: ".".into(),
            timeout_secs: 120,
        }],
        authority_ref: "policy:p02".into(),
        max_attempts,
        simulated: true,
    }
}

fn digest_of(label: &str) -> String {
    Sha256Digest::of_bytes(label.as_bytes()).into()
}

#[test]
fn attempt_outcome_preserves_prior_closure_evidence_across_retry_and_keeps_scheduling_separate() {
    let contract = sample_contract("task-outcome-1", 3);
    let task_id = contract.task_id.clone();
    let contract_digest = content_digest(&contract).expect("contract digest");
    let state = AttemptOutcomeProjection::default();

    let state = state
        .apply(&DomainEvent::TaskSubmitted {
            contract: contract.clone(),
        })
        .expect("submit");
    let ledger = &state.task_ledgers[&task_id];
    assert_eq!(ledger.scheduling, TaskSchedulingDimension::Ready);
    assert!(ledger.outcomes.is_empty());

    let attempt1 = AttemptId::new("attempt-1").expect("attempt-1");
    let worker1 = PrincipalId::new("worker-a").expect("worker-a");
    let state = state
        .apply(&DomainEvent::TaskClaimed {
            task_id: task_id.clone(),
            revision: 1,
            attempt_id: attempt1.clone(),
            owner: worker1.clone(),
        })
        .expect("claim attempt-1");
    assert_eq!(
        state.task_ledgers[&task_id].scheduling,
        TaskSchedulingDimension::Claimed
    );
    assert_eq!(
        state.task_ledgers[&task_id].outcomes[&attempt1].status,
        AttemptOutcomeStatus::Unknown
    );

    let state = state
        .apply(&DomainEvent::TaskStarted {
            task_id: task_id.clone(),
            attempt_id: attempt1.clone(),
        })
        .expect("start attempt-1");
    assert_eq!(
        state.task_ledgers[&task_id].scheduling,
        TaskSchedulingDimension::Running
    );
    assert_eq!(
        state.task_ledgers[&task_id].outcomes[&attempt1].status,
        AttemptOutcomeStatus::Unknown
    );

    // Close attempt-1 as failed with explicit closure evidence and reason.
    let closure1 = digest_of("closure-attempt-1");
    let expected_task_digest = content_digest(&state.projection.tasks[&task_id]).expect("task cas");
    let state = state
        .apply(&DomainEvent::TaskLifecycle {
            command: LifecycleCommand {
                protocol_version: 2,
                task_id: task_id.clone(),
                expected_task_digest,
                actor: PrincipalId::new("supervisor-1").expect("supervisor"),
                action: LifecycleAction::CloseAttempt {
                    attempt_id: attempt1.clone(),
                    closure_digest: closure1.clone(),
                    reason: "child process exited 101 during compilation".into(),
                    cancelled: false,
                },
            },
        })
        .expect("close attempt-1");
    assert_eq!(
        state.task_ledgers[&task_id].scheduling,
        TaskSchedulingDimension::Closed
    );
    assert_eq!(
        state.task_ledgers[&task_id].outcomes[&attempt1].status,
        AttemptOutcomeStatus::Failed
    );
    assert_eq!(
        state.task_ledgers[&task_id].outcomes[&attempt1].closure_digest,
        Some(closure1.clone())
    );

    // Retry attempt-1: v1 TaskView clears its active closure_digest/reason,
    // but TaskAttemptOutcomeLedger preserves attempt-1's full closure record!
    let expected_task_digest =
        content_digest(&state.projection.tasks[&task_id]).expect("task cas before retry");
    let state = state
        .apply(&DomainEvent::TaskLifecycle {
            command: LifecycleCommand {
                protocol_version: 2,
                task_id: task_id.clone(),
                expected_task_digest,
                actor: PrincipalId::new("supervisor-1").expect("supervisor"),
                action: LifecycleAction::Retry {
                    attempt_id: attempt1.clone(),
                    closure_digest: closure1.clone(),
                },
            },
        })
        .expect("retry after attempt-1");
    assert_eq!(
        state.projection.tasks[&task_id].state,
        SchedulingState::Ready
    );
    assert!(
        state.projection.tasks[&task_id]
            .lifecycle
            .as_ref()
            .expect("v2 lifecycle")
            .closure_digest
            .is_none(),
        "closed v1 TaskLifecycleView clears current closure_digest on retry"
    );
    let retained_attempt1 = &state.task_ledgers[&task_id].outcomes[&attempt1];
    assert_eq!(retained_attempt1.status, AttemptOutcomeStatus::Failed);
    assert_eq!(retained_attempt1.closure_digest, Some(closure1));
    assert_eq!(
        retained_attempt1.reason.as_deref(),
        Some("child process exited 101 during compilation")
    );
    assert!(retained_attempt1.superseded_by_retry);
    assert_eq!(
        state.task_ledgers[&task_id].scheduling,
        TaskSchedulingDimension::Ready
    );

    // Claim, start, submit candidate, and accept attempt-2.
    let attempt2 = AttemptId::new("attempt-2").expect("attempt-2");
    let worker2 = PrincipalId::new("worker-b").expect("worker-b");
    let state = state
        .apply(&DomainEvent::TaskClaimed {
            task_id: task_id.clone(),
            revision: 1,
            attempt_id: attempt2.clone(),
            owner: worker2,
        })
        .expect("claim attempt-2")
        .apply(&DomainEvent::TaskStarted {
            task_id: task_id.clone(),
            attempt_id: attempt2.clone(),
        })
        .expect("start attempt-2")
        .apply(&DomainEvent::CandidateSubmitted {
            task_id: task_id.clone(),
            attempt_id: attempt2.clone(),
            candidate_ref: "refs/heads/codex/candidate-2".into(),
        })
        .expect("submit candidate-2");

    assert_eq!(
        state.task_ledgers[&task_id].scheduling,
        TaskSchedulingDimension::AwaitingDecision
    );
    assert_eq!(
        state.task_ledgers[&task_id].outcomes[&attempt2].status,
        AttemptOutcomeStatus::Unknown
    );

    let verifier = PrincipalId::new("verifier-independent").expect("verifier");
    let closure2 = digest_of("closure-attempt-2");
    let evidence2 = CandidateDecisionEvidence {
        contract_digest,
        verification_policy_digest: digest_of("policy"),
        receipt_digest: digest_of("receipt"),
        custody_digest: digest_of("custody"),
        closure_digest: closure2.clone(),
        source_commit_sha: "a".repeat(40),
        source_tree_sha: "b".repeat(40),
        verifier: verifier.clone(),
    };
    let expected_task_digest =
        content_digest(&state.projection.tasks[&task_id]).expect("task cas before decide");
    let state = state
        .apply(&DomainEvent::TaskLifecycle {
            command: LifecycleCommand {
                protocol_version: 2,
                task_id: task_id.clone(),
                expected_task_digest,
                actor: verifier,
                action: LifecycleAction::DecideCandidate {
                    attempt_id: attempt2.clone(),
                    candidate_ref: "refs/heads/codex/candidate-2".into(),
                    evidence: evidence2.clone(),
                    accepted: true,
                },
            },
        })
        .expect("accept attempt-2");

    let final_ledger = &state.task_ledgers[&task_id];
    assert_eq!(final_ledger.scheduling, TaskSchedulingDimension::Closed);
    assert_eq!(
        final_ledger.attempt_order,
        vec![attempt1.clone(), attempt2.clone()]
    );
    assert_eq!(
        final_ledger.outcomes[&attempt1].status,
        AttemptOutcomeStatus::Failed
    );
    assert!(final_ledger.outcomes[&attempt1].superseded_by_retry);
    assert_eq!(
        final_ledger.outcomes[&attempt2].status,
        AttemptOutcomeStatus::Succeeded
    );
    assert!(!final_ledger.outcomes[&attempt2].superseded_by_retry);
    assert_eq!(
        final_ledger.outcomes[&attempt2].decision_evidence,
        Some(evidence2)
    );
    assert!(final_ledger.canonical_digest().is_ok());
}

#[test]
fn execution_requirements_v2_validates_contract_binding_shell_boundary_and_env_allowlist() {
    let contract = sample_contract("task-exec-req-1", 2);
    let contract_digest = content_digest(&contract).expect("contract digest");

    let valid_reqs = ExecutionRequirementsV2 {
        schema_version: EXECUTION_REQUIREMENTS_SCHEMA_VERSION,
        project_id: contract.project_id.clone(),
        root_id: contract.root_id.clone(),
        task_id: contract.task_id.clone(),
        contract_revision: contract.revision,
        contract_digest: contract_digest.clone(),
        base_commit_sha: "c58a67c095248ab3a3c7dd5ba9568422575ad751".into(),
        runner: ExecutionRunnerRequirements {
            runner_mode: ExecutionRunnerMode::PrivateCloneRootlessOci,
            required_capabilities: vec!["linux-x86_64".into(), "rust-1.85".into()],
            locality_refs: vec!["repo:admiralty".into()],
            budget_requirement: BudgetRequirement {
                account_pool: "pool-main".into(),
                amount: PlacementDecimalU64::new(500),
            },
        },
        environment_allowlist: vec!["CARGO_BUILD_JOBS".into(), "RUST_TEST_THREADS".into()],
        shell_boundary: ShellBoundaryPolicy::ForbidShell,
        deadline_utc: "2026-10-19T18:00:00Z".into(),
        max_wall_clock_secs: 600,
        responsible_role: ResponsibleRole::Worker,
        required_delivery_stage: RequiredDeliveryBoundary::VerifiedCandidate,
    };

    let capsule = valid_reqs
        .compile_capsule_binding(&contract)
        .expect("valid execution requirements capsule binding");
    assert_eq!(capsule.contract_digest, contract_digest);
    assert!(capsule.requirements_digest.starts_with("sha256:"));
    assert!(capsule.placement_request_digest.starts_with("sha256:"));
    assert!(
        capsule
            .rendered_summary
            .contains("CARGO_BUILD_JOBS,RUST_TEST_THREADS")
    );

    for deadline in [
        "".to_owned(),
        "Z".to_owned(),
        "not-a-timeZ".to_owned(),
        "2026-13-20T12:34:56Z".to_owned(),
        "2026-10-00T12:34:56Z".to_owned(),
        "2026-02-30T12:34:56Z".to_owned(),
        "2025-02-29T12:34:56Z".to_owned(),
        "2026-10-20T25:34:56Z".to_owned(),
        "2026-10-20T12:60:00Z".to_owned(),
        "2026-10-20T12:34:56".to_owned(),
        "2026-10-20T12:34:56+00:00".to_owned(),
        "2026-10-20T12:34:56+01:00Z".to_owned(),
        "2026-10-20\nT12:34:56Z".to_owned(),
        "\u{0085}2026-10-20T12:34:56Z".to_owned(),
        format!("{}Z", "0".repeat(64)),
    ] {
        let mut invalid_deadline = valid_reqs.clone();
        invalid_deadline.deadline_utc = deadline.clone();
        assert!(
            invalid_deadline.validate().is_err(),
            "invalid UTC deadline must fail: {deadline:?}"
        );
        assert!(
            invalid_deadline
                .validate_against_contract(&contract)
                .is_err(),
            "invalid UTC deadline cannot bind a contract: {deadline:?}"
        );
        assert!(
            invalid_deadline.compile_capsule_binding(&contract).is_err(),
            "invalid UTC deadline cannot produce a capsule: {deadline:?}"
        );
    }

    for deadline in [
        "2026-10-19T18:00:00Z",
        "2026-10-20T12:34:56Z",
        "2024-02-29T12:34:56Z",
        "2026-10-20T12:34:56.123456789Z",
    ] {
        let mut valid_deadline = valid_reqs.clone();
        valid_deadline.deadline_utc = deadline.to_owned();
        let wire = serde_json::to_vec(&valid_deadline).expect("canonical requirements wire");
        let expected_digest = content_digest(&valid_deadline).expect("requirements digest");
        let binding = valid_deadline
            .compile_capsule_binding(&contract)
            .expect("valid UTC deadline");
        assert_eq!(binding.requirements_digest, expected_digest);
        assert_eq!(
            binding.placement_request_digest, capsule.placement_request_digest,
            "deadline parsing must not change placement identity"
        );
        let decoded: ExecutionRequirementsV2 =
            serde_json::from_slice(&wire).expect("roundtrip unchanged requirements");
        assert_eq!(decoded.deadline_utc, deadline);
        assert_eq!(serde_json::to_vec(&decoded).expect("unchanged wire"), wire);
        assert_eq!(
            decoded
                .compile_capsule_binding(&contract)
                .expect("valid roundtrip capsule"),
            binding
        );
        if deadline == valid_reqs.deadline_utc {
            assert_eq!(binding, capsule, "original valid capsule stays unchanged");
        }
    }

    // Secret-bearing env allowlist entry must be rejected.
    let mut secret_env = valid_reqs.clone();
    secret_env
        .environment_allowlist
        .push("GITHUB_API_TOKEN".into());
    assert!(secret_env.validate_against_contract(&contract).is_err());

    // Shell check program under ForbidShell must be rejected, and permitted only when
    // DeclaredInterpreter matches the exact shell program.
    let mut shell_contract = contract.clone();
    shell_contract.checks[0].program = "/bin/bash".into();
    let shell_contract_digest = content_digest(&shell_contract).expect("shell contract digest");
    let mut shell_reqs = valid_reqs.clone();
    shell_reqs.contract_digest = shell_contract_digest;
    assert!(
        shell_reqs
            .validate_against_contract(&shell_contract)
            .is_err(),
        "shell program must fail under ForbidShell"
    );

    shell_reqs.shell_boundary = ShellBoundaryPolicy::DeclaredInterpreter {
        interpreter_program: "/bin/bash".into(),
        trust_boundary_ref: "sandbox:rootless-oci-seccomp".into(),
        justification: "Controlled wrapper script execution".into(),
    };
    assert!(
        shell_reqs
            .validate_against_contract(&shell_contract)
            .is_ok(),
        "matching DeclaredInterpreter must pass"
    );
}

#[test]
fn interface_compatibility_allows_provisional_dev_enforces_verified_final_and_revokes_without_erasing_work()
 {
    let producer_ref_v1 = TaskRevisionRef {
        task_id: TaskId::new("api-producer").expect("producer"),
        revision: 1,
        contract_digest: Sha256Digest::of_bytes(b"producer-v1"),
    };
    let producer_ref_v2 = TaskRevisionRef {
        task_id: TaskId::new("api-producer").expect("producer"),
        revision: 2,
        contract_digest: Sha256Digest::of_bytes(b"producer-v2"),
    };
    let consumer_ref = TaskRevisionRef {
        task_id: TaskId::new("ui-consumer").expect("consumer"),
        revision: 1,
        contract_digest: Sha256Digest::of_bytes(b"consumer-v1"),
    };
    let artifact_digest = Sha256Digest::of_bytes(b"schema-artifact-v2");
    let schema_digest = Sha256Digest::of_bytes(b"schema-digest-v2");

    let mut binding = TaskInterfaceContractV2 {
        schema_version: INTERFACE_COMPAT_SCHEMA_VERSION,
        interface_id: InterfaceId::new("fleet-feed-v2").expect("interface id"),
        project_id: ProjectId::new("admiralty").expect("project"),
        root_id: RootId::new("root-p02").expect("root"),
        authority_ref: "policy:interface-v2".into(),
        interface_revision: InterfaceDecimalU64::new(1),
        producer_task: producer_ref_v2.clone(),
        consumer_tasks: vec![consumer_ref.clone()],
        artifact_digest: artifact_digest.clone(),
        schema_digest: schema_digest.clone(),
        compatibility_policy: ConsumerCompatibilityPolicy::ProvisionalDevelopmentAllowed {
            min_provisional_producer_revision: 1,
            required_final_producer_revision: 2,
        },
        acceptance_check_ids: vec!["schema-roundtrip".into(), "ui-contract-check".into()],
        status: InterfaceBindingStatus::Provisional,
    };

    let prior_work = vec![
        "attempt:ui-1".to_string(),
        "candidate:refs/heads/ui-draft".to_string(),
    ];

    // 1. Provisional development against producer revision 1 succeeds while binding is Provisional.
    let provisional_input = ConsumerCompatibilityInput {
        consumer_task: consumer_ref.clone(),
        observed_producer_task: producer_ref_v1,
        observed_artifact_digest: Sha256Digest::of_bytes(b"provisional-artifact-v1"),
        observed_schema_digest: schema_digest.clone(),
        passed_check_ids: vec![],
        stage: ConsumerEvaluationStage::ProvisionalDevelopment,
        prior_work_refs: prior_work.clone(),
    };
    assert_eq!(
        binding
            .evaluate_consumer(&provisional_input)
            .expect("provisional eval"),
        ConsumerCompatibilityVerdict::CompatibleProvisional {
            retained_work_refs: prior_work.clone()
        }
    );

    // 2. Final acceptance fails while binding is still Provisional, even if producer revision is 2.
    let final_input = ConsumerCompatibilityInput {
        consumer_task: consumer_ref.clone(),
        observed_producer_task: producer_ref_v2.clone(),
        observed_artifact_digest: artifact_digest.clone(),
        observed_schema_digest: schema_digest.clone(),
        passed_check_ids: vec!["schema-roundtrip".into(), "ui-contract-check".into()],
        stage: ConsumerEvaluationStage::FinalAcceptance,
        prior_work_refs: prior_work.clone(),
    };
    assert!(matches!(
        binding
            .evaluate_consumer(&final_input)
            .expect("final eval while provisional"),
        ConsumerCompatibilityVerdict::IncompatibleRevision { .. }
    ));

    // 3. Once binding is Verified, final acceptance requires all acceptance_check_ids and exact producer v2.
    binding.status = InterfaceBindingStatus::Verified;
    let mut missing_check_input = final_input.clone();
    missing_check_input.passed_check_ids = vec!["schema-roundtrip".into()];
    assert_eq!(
        binding
            .evaluate_consumer(&missing_check_input)
            .expect("missing check eval"),
        ConsumerCompatibilityVerdict::MissingAcceptanceChecks {
            missing_check_ids: vec!["ui-contract-check".into()],
            retained_work_refs: prior_work.clone(),
        }
    );

    let accepted = binding
        .evaluate_consumer(&final_input)
        .expect("verified final eval");
    assert!(matches!(
        accepted,
        ConsumerCompatibilityVerdict::CompatibleFinalAcceptance {
            ref verified_binding_digest,
            ref retained_work_refs
        } if verified_binding_digest.starts_with("sha256:") && retained_work_refs == &prior_work
    ));

    // 4. Revoking the producer interface invalidates consumer readiness and verification receipts
    // without erasing prior consumer work references.
    let (revoked_binding, impact) = binding
        .revoke("producer revision 2 regression found in schema")
        .expect("revoke binding");
    assert!(impact.readiness_invalidated);
    assert!(impact.verification_receipts_invalidated);
    assert!(!impact.prior_work_erased);
    assert_eq!(
        impact.affected_consumer_tasks,
        vec![consumer_ref.task_id.clone()]
    );

    let post_revoke = revoked_binding
        .evaluate_consumer(&final_input)
        .expect("eval after revocation");
    assert_eq!(
        post_revoke,
        ConsumerCompatibilityVerdict::InvalidatedByRevocation {
            reason: "producer revision 2 regression found in schema".into(),
            readiness_invalidated: true,
            receipts_invalidated: true,
            retained_work_refs: prior_work,
        }
    );
}
