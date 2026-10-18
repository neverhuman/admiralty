#![cfg(unix)]
//! Offline publication regressions using retained provider bytes and a fake forge.
//! Local Git fixtures never launch a provider or touch a real forge. These
//! component checks confer no P08/P19/P21 native qualification or acceptance.

use admiralty_core::{
    ProjectId, RootId, TaskId,
    conflicts::{CandidateChange, ConflictRisk, FileChange, HunkRange, analyze_conflicts},
    economics::{
        self, BoundedId, CohortSpec, DecimalU64 as EconomicsU64, ObservedAmount, ParentInclusion,
        RootBinding, RootOutcome, RootStatus, UsageRecord,
    },
    placement::{
        BudgetRequirement, Known, NodeId, NodeOffer, Observation, PlacementPolicy,
        PlacementRequest, propose_placement,
    },
    publication::{
        ApprovalEvidence, CheckEvidence, CheckState, DecimalU64, IntentStage, PublicationBlock,
        PublicationIntent, PublicationScope, PublicationSnapshot, ReconciliationReceipt,
        propose_publication, publication_request_digest, reduce_publication_intent,
    },
};
use admiralty_runtime::{
    providers::claude::{ClaudeLaunch, Effort, TurnOutcome, UsageSource, parse_stream},
    recovery::{Blocker, Reconciliation, SessionCustody, plan_recovery},
    session::MAX_INPUT_BYTES,
};
use admiralty_store::artifacts::ArtifactStore;
use admiralty_testkit::forge::{
    Fault as ForgeFault, ForgeKey, ForgeOutcome, ForgeReply, ForgeRequest, ForgeSimulator,
};
#[path = "support/git_fixture.rs"]
mod git_fixture;
use git_fixture::{commit, git, private_directory, private_fixture, text};

#[path = "support/recovery.rs"]
mod recovery_fixture;

fn eligible_intent(
    scope: &PublicationScope,
    snapshot: &PublicationSnapshot,
    intent_key: &str,
) -> PublicationIntent {
    let generation = DecimalU64::new(8);
    let digest = publication_request_digest(scope, snapshot, intent_key, &generation)
        .expect("fixture request digest");
    let result = propose_publication(scope, snapshot, intent_key, &digest, &generation);
    assert!(result.eligible, "proposal rejected: {:?}", result.reasons);
    assert!(!result.execution_granted());
    result.intent.expect("eligible proposal returns an intent")
}

#[test]
fn immutable_worktree_candidate_drives_only_a_simulated_publication() {
    let fixture = private_fixture();
    let repository = fixture.path().join("repository");
    let worktree = fixture.path().join("codex-worktree");
    private_directory(&repository);
    git(&repository, &["init", "--quiet", "--initial-branch=main"]);
    std::fs::create_dir(repository.join("src")).expect("initial source directory");
    std::fs::write(
        repository.join("src/lib.rs"),
        b"pub fn value() -> u8 { 1 }\n",
    )
    .expect("baseline source");
    let base = commit(&repository, "baseline", "2000-01-01T00:00:00Z");

    git(
        &repository,
        &[
            "worktree",
            "add",
            "--quiet",
            "-b",
            "codex/round8-item2",
            worktree.to_str().expect("worktree path is utf-8"),
            &base,
        ],
    );
    std::fs::write(worktree.join("src/lib.rs"), b"pub fn value() -> u8 { 2 }\n")
        .expect("Codex candidate source");
    let candidate = commit(&worktree, "candidate", "2000-01-01T00:00:01Z");

    assert_eq!(text(git(&repository, &["rev-parse", "main"])), base);
    assert_eq!(text(git(&worktree, &["rev-parse", "HEAD"])), candidate);
    assert_eq!(
        text(git(&worktree, &["rev-parse", "HEAD^"])),
        base,
        "candidate is a linear child of the observed baseline"
    );

    let generation = DecimalU64::new(8);
    let scope = PublicationScope {
        project_id: ProjectId::new("round8-p08-p19").expect("project id"),
        root_id: RootId::new("temporary-git-root").expect("root id"),
        task_id: TaskId::new("worktree-publication").expect("task id"),
        target_ref: "refs/heads/main".into(),
        authority_generation: generation.clone(),
    };
    let snapshot = PublicationSnapshot {
        current_head: Some(candidate.clone()),
        current_base: Some(base.clone()),
        expected_head: Some(candidate.clone()),
        expected_base: Some(base.clone()),
        required_reviewers: vec!["maintainer".into()],
        approvals: vec![ApprovalEvidence {
            reviewer: "maintainer".into(),
            head: candidate.clone(),
            dismissed: false,
            provenance_ref: "fixture:round8:approval".into(),
        }],
        required_checks: vec!["worktree-integrity".into()],
        checks: vec![CheckEvidence {
            check_id: "worktree-integrity".into(),
            head: candidate.clone(),
            state: CheckState::Passed,
            provenance_ref: "fixture:round8:check".into(),
        }],
        owner_only: false,
        caller_is_owner: false,
        unresolved_intent: false,
    };
    let intent = eligible_intent(&scope, &snapshot, "offline-worktree-publication");
    assert_eq!(intent.expected_head, candidate);
    assert_eq!(intent.expected_base, base);

    // A new, valid Git candidate cannot inherit the original candidate's
    // approval/check evidence, even with an otherwise refreshed snapshot.
    std::fs::write(worktree.join("src/lib.rs"), b"pub fn value() -> u8 { 3 }\n")
        .expect("changed synthetic candidate source");
    let changed_head = commit(&worktree, "changed candidate", "2000-01-01T00:00:02Z");
    let mut changed = snapshot.clone();
    changed.current_head = Some(changed_head.clone());
    changed.expected_head = Some(changed_head);
    let changed_digest = publication_request_digest(
        &scope,
        &changed,
        "changed-worktree-publication",
        &generation,
    )
    .expect("changed candidate request digest");
    let rejected = propose_publication(
        &scope,
        &changed,
        "changed-worktree-publication",
        &changed_digest,
        &generation,
    );
    assert!(!rejected.eligible);
    assert!(rejected.intent.is_none());
    assert!(!rejected.execution_granted());
    assert!(rejected.reasons.contains(&PublicationBlock::WrongReviewer));
    assert!(
        rejected
            .reasons
            .contains(&PublicationBlock::MissingOrUnknownCheck)
    );

    let key = ForgeKey {
        repository: "fixture/round8".into(),
        target: scope.target_ref,
    };
    let request = ForgeRequest {
        id: intent.intent_key,
        key: key.clone(),
        expected_head: intent.expected_base,
        new_head: intent.expected_head,
    };
    let mut forge = ForgeSimulator::new(1, 1).expect("bounded forge simulator");
    forge
        .seed_ref(key.clone(), base.clone())
        .expect("seed target");
    assert_eq!(
        forge.submit(request, ForgeFault::None),
        Ok(ForgeReply::Observed(ForgeOutcome::Applied {
            previous_head: base.clone(),
            new_head: candidate.clone(),
        }))
    );
    assert_eq!(forge.head(&key), Some(candidate.as_str()));
    assert_eq!(forge.applied_mutations(), 1);
    assert_eq!(text(git(&repository, &["rev-parse", "main"])), base);
}

#[test]
fn provider_observation_and_cross_component_uncertainty_do_not_grant_publication() {
    // P04: preserve immutable candidate evidence and reject an over-limit object.
    let artifact_fixture = private_fixture();
    let artifact_root = artifact_fixture.path().join("objects");
    private_directory(&artifact_root);
    let artifacts = ArtifactStore::open(&artifact_root, 64).expect("bounded artifact store");
    let candidate_bytes = b"Claude candidate source fixture";
    let reference = artifacts
        .publish_bytes(candidate_bytes)
        .expect("candidate evidence stored by content address");
    assert_eq!(
        artifacts
            .read_verified(&reference)
            .expect("verify evidence"),
        candidate_bytes
    );
    assert!(artifacts.publish_bytes(&[b'x'; 65]).is_err());
    assert_eq!(
        artifacts
            .read_verified(&reference)
            .expect("oversize rejection preserves object"),
        candidate_bytes
    );

    // P07: the launch remains unadmitted and an input frame over the shared host
    // bound is rejected before any provider process can be started.
    let launch = ClaudeLaunch {
        program: "/usr/local/bin/claude".into(),
        cwd: "/workspace/project".into(),
        model: "sonnet".into(),
        effort: Effort::Low,
        max_budget_usd: "0.1".into(),
        permission_mode: "dontAsk".into(),
        session_id: Some("fixture-session".into()),
        resume: true,
    };
    assert!(launch.args().is_ok());
    assert!(!launch.is_admitted());
    assert!(
        ClaudeLaunch::input_frame(&"x".repeat(MAX_INPUT_BYTES + 1)).is_err(),
        "oversized prompt is rejected rather than truncated or retried"
    );

    // P21: provider success is only an observation. Session mismatch, absent
    // capture authority, and malformed terminal usage remain non-accepting.
    let turn = include_bytes!("../fixtures/providers/claude/continuity-turn.jsonl");
    let observed = parse_stream(turn, Some("fixture-session")).expect("valid synthetic turn");
    assert_eq!(observed.outcome, TurnOutcome::Succeeded);
    assert_eq!(observed.usage_source, UsageSource::ParentTerminal);
    assert!(!observed.native_execution_qualified());
    assert!(!observed.trusted_acceptance());
    assert!(parse_stream(turn, Some("foreign-session")).is_err());
    let mut truncated = turn.to_vec();
    truncated.pop();
    assert!(parse_stream(&truncated, Some("fixture-session")).is_err());
    assert!(parse_stream(&vec![b' '; 1_048_577], None).is_err());

    // P18: incomplete hunk/ancestry facts remain explicit conflict risks.
    let project = ProjectId::new("round9-project").expect("project");
    let root = RootId::new("round9-root").expect("root");
    let candidate =
        |candidate_id: &str, head: char, hunks: Option<Vec<HunkRange>>| CandidateChange {
            project_id: project.clone(),
            root_id: root.clone(),
            task_id: TaskId::new(format!("task-{candidate_id}")).expect("task"),
            candidate_id: candidate_id.into(),
            base_sha: "a".repeat(40),
            head_sha: head.to_string().repeat(40),
            tree_sha: "d".repeat(40),
            ancestry_digest: None,
            files: vec![FileChange {
                path: "src/lib.rs".into(),
                hunks,
            }],
            constraints: None,
        };
    let graph = analyze_conflicts(
        &[
            candidate("candidate-a", 'b', None),
            candidate("candidate-b", 'c', Some(vec![])),
        ],
        "round9-policy",
    )
    .expect("conservative graph is produced");
    assert!(graph.edges.iter().any(|edge| {
        edge.risks.contains(&ConflictRisk::UnknownAncestry)
            && edge.risks.contains(&ConflictRisk::UnknownWriteScope)
            && edge.risks.contains(&ConflictRisk::SamePathUnknownHunks)
    }));
    assert!(graph.generation.combined_verification_required);

    // P24: unknown node capacity cannot yield an execution candidate.
    let placement_request = PlacementRequest {
        project: project.clone(),
        root: root.clone(),
        task: TaskId::new("round9-placement").expect("placement task"),
        contract_digest: format!("sha256:{}", "e".repeat(64)),
        authority_ref: "fixture:authority".into(),
        required_capabilities: vec!["claude".into()],
        locality_refs: vec!["src".into()],
        budget_requirement: BudgetRequirement {
            account_pool: "fixture-pool".into(),
            amount: admiralty_core::placement::DecimalU64::new(1),
        },
    };
    let decimal = admiralty_core::placement::DecimalU64::new;
    let offer = NodeOffer {
        node_id: NodeId::new("node-round9").expect("node"),
        boot_id: "boot-round9".into(),
        authority_generation: decimal(1),
        visible_scopes: Observation {
            value: vec!["round9-project".into()],
            observed_at: decimal(10),
        },
        capabilities: Observation {
            value: vec!["claude".into()],
            observed_at: decimal(10),
        },
        account_pool: Observation {
            value: "fixture-pool".into(),
            observed_at: decimal(10),
        },
        capacity: Observation {
            value: Known::Unknown {
                reason: "capacity probe unavailable".into(),
            },
            observed_at: decimal(10),
        },
        current_pressure: Observation {
            value: Known::Known { value: decimal(0) },
            observed_at: decimal(10),
        },
        spool_free: Observation {
            value: Known::Known { value: decimal(10) },
            observed_at: decimal(10),
        },
        existing_active_roots: vec![],
        controller_connected: true,
        locality_refs: vec!["src".into()],
        continuation_permit: None,
        reconciliation_required: false,
        ownership_observed_at: Some(decimal(10)),
        ownership_revision: Some(decimal(1)),
    };
    let placement = propose_placement(
        &placement_request,
        &[offer],
        &PlacementPolicy {
            max_observation_age: decimal(10),
            maximum_pressure: decimal(5),
            minimum_spool_reserve: decimal(1),
            disconnected_continuation_only: true,
            ownership_revision: Some(decimal(1)),
        },
        10,
    )
    .expect("unknown capacity becomes a safe rejection");
    assert!(placement.candidates.is_empty());
    assert!(!placement.execution_granted);
    assert!(!placement.publication_granted);

    // P25: unresolved remote effects and unknown session custody prevent resume.
    let (manifest, mut recovery_observation, fence) = recovery_fixture::valid_recovery_evidence();
    recovery_observation.unresolved_effects = Reconciliation::Unresolved;
    recovery_observation.session_custody = SessionCustody::Unknown;
    let recovery = plan_recovery(&manifest, &recovery_observation, Some(&fence))
        .expect("uncertainty remains an explicit proposal blocker");
    assert!(
        recovery
            .blockers
            .contains(&Blocker::UnresolvedRemoteMutation)
    );
    assert!(recovery.blockers.contains(&Blocker::UnknownSessionCustody));
    assert!(!recovery.resume_proposed);
    assert!(!recovery.restore_authorized());

    // P26: absent native usage is unknown cost, not zero cost or a savings claim.
    let task = TaskId::new("round9-economics").expect("economics task");
    let binding = RootBinding {
        root_id: root.clone(),
        task_id: task,
        contract_revision: 1,
        contract_digest: format!("sha256:{}", "a".repeat(64)),
        protected_oracle_digest: format!("sha256:{}", "b".repeat(64)),
    };
    let cohort = CohortSpec {
        cohort_id: BoundedId::new("round9-cohort").expect("cohort"),
        project_id: project.clone(),
        fixed_assignment_roots: vec![root.clone()],
        required_delivery_stage: BoundedId::new("accepted").expect("stage"),
        valuation_digest: format!("sha256:{}", "c".repeat(64)),
        assigned_at: EconomicsU64(1),
        bindings: vec![binding.clone()],
    };
    let summary = economics::summarize_cohort(
        &cohort,
        &[UsageRecord {
            project_id: project.clone(),
            binding: binding.clone(),
            observation_id: BoundedId::new("claude-usage").expect("usage id"),
            revision: EconomicsU64(1),
            root_scope: root.clone(),
            role: BoundedId::new("worker").expect("role"),
            purpose: BoundedId::new("implementation").expect("purpose"),
            provider: BoundedId::new("claude").expect("provider"),
            unit: BoundedId::new("usd_micros").expect("unit"),
            amount: ObservedAmount::Unknown {
                reason: "provider interval is not independently reconciled".into(),
            },
            parent_inclusion: ParentInclusion::Standalone,
        }],
        &[RootOutcome {
            project_id: project.clone(),
            binding,
            acceptance_receipt_digest: None,
            root_id: root.clone(),
            status: RootStatus::Failed,
            accepted_stage: None,
            maturity: EconomicsU64(10),
        }],
        10,
    )
    .expect("unknown usage summarizes without inventing a cost");
    assert_eq!(summary.units[0].amount, None);
    assert_eq!(summary.units[0].coverage, economics::Coverage::Unknown);
    assert_eq!(summary.accepted_roots, Some(EconomicsU64(0)));
    // P19: a successful provider result cannot satisfy unresolved independent
    // checks. Admission produces no intent and grants no publication effect.
    let generation = DecimalU64::new(9);
    let scope = PublicationScope {
        project_id: project,
        root_id: root,
        task_id: TaskId::new("round9-publication").expect("publication task"),
        target_ref: "refs/heads/main".into(),
        authority_generation: generation.clone(),
    };
    let base = "a".repeat(40);
    let proposal_snapshot = PublicationSnapshot {
        current_head: Some(base.clone()),
        current_base: Some(base.clone()),
        expected_head: Some(base.clone()),
        expected_base: Some(base),
        required_reviewers: vec!["maintainer".into()],
        approvals: vec![ApprovalEvidence {
            reviewer: "maintainer".into(),
            head: "a".repeat(40),
            dismissed: false,
            provenance_ref: "fixture:round9:approval".into(),
        }],
        required_checks: vec!["cross-packet-edge-review".into()],
        checks: vec![CheckEvidence {
            check_id: "cross-packet-edge-review".into(),
            head: "a".repeat(40),
            state: CheckState::Pending,
            provenance_ref: "fixture:round9:unresolved-evidence".into(),
        }],
        owner_only: false,
        caller_is_owner: false,
        unresolved_intent: false,
    };
    let digest =
        publication_request_digest(&scope, &proposal_snapshot, "round9-candidate", &generation)
            .expect("bounded publication request");
    let publication = propose_publication(
        &scope,
        &proposal_snapshot,
        "round9-candidate",
        &digest,
        &generation,
    );
    assert!(!publication.eligible);
    assert!(!publication.execution_granted());
    assert!(publication.intent.is_none());
    assert!(
        publication
            .reasons
            .contains(&PublicationBlock::CheckNotPassed)
    );
}

#[test]
fn synthetic_workspace_gate_requires_current_checks_and_authority() {
    // A provider-reported successful turn is only an observation. Bind it to
    // the exact retained session before evaluating independent workspace gates.
    let bytes = include_bytes!("../fixtures/providers/claude/continuity-turn.jsonl");
    let claude = parse_stream(bytes, Some("fixture-session")).expect("complete Claude turn");
    assert_eq!(claude.outcome, TurnOutcome::Succeeded);
    assert!(!claude.trusted_acceptance());
    assert!(!claude.native_execution_qualified());
    assert_eq!(
        parse_stream(bytes, Some("different-session")),
        Err(admiralty_runtime::providers::claude::ClaudeError::ResumeMismatch)
    );

    let generation = DecimalU64::new(10);
    let scope = PublicationScope {
        project_id: ProjectId::new("round10-workspace-audit").expect("project"),
        root_id: RootId::new("round10-root").expect("root"),
        task_id: TaskId::new("round10-claude-turn").expect("task"),
        target_ref: "refs/heads/main".into(),
        authority_generation: generation.clone(),
    };
    let base = "a".repeat(40);
    let mut snapshot = PublicationSnapshot {
        current_head: Some(base.clone()),
        current_base: Some(base.clone()),
        expected_head: Some(base.clone()),
        expected_base: Some(base.clone()),
        required_reviewers: vec!["workspace-owner".into()],
        approvals: vec![ApprovalEvidence {
            reviewer: "workspace-owner".into(),
            head: base.clone(),
            dismissed: false,
            provenance_ref: "fixture:round10:review".into(),
        }],
        required_checks: vec!["workspace-state-audit".into()],
        checks: vec![CheckEvidence {
            check_id: "workspace-state-audit".into(),
            head: base.clone(),
            state: CheckState::Pending,
            provenance_ref: "fixture:round10:pending-audit".into(),
        }],
        owner_only: false,
        caller_is_owner: false,
        unresolved_intent: false,
    };

    let proposal_for = |snapshot: &PublicationSnapshot, authority: &DecimalU64| {
        let digest = publication_request_digest(&scope, snapshot, "claude-candidate", authority)
            .expect("bounded exact workspace audit request");
        propose_publication(&scope, snapshot, "claude-candidate", &digest, authority)
    };

    let pending = proposal_for(&snapshot, &generation);
    assert!(!pending.eligible);
    assert!(pending.intent.is_none());
    assert!(pending.reasons.contains(&PublicationBlock::CheckNotPassed));

    // A moved workspace head invalidates the earlier approval/check evidence
    // even though the provider stream remains a well-formed successful turn.
    snapshot.checks[0].state = CheckState::Passed;
    snapshot.current_head = Some("b".repeat(40));
    let moved_head = proposal_for(&snapshot, &generation);
    assert!(!moved_head.eligible);
    assert!(moved_head.reasons.contains(&PublicationBlock::ChangedHead));

    snapshot.current_head = Some(base);
    let stale_authority = proposal_for(&snapshot, &DecimalU64::new(9));
    assert!(!stale_authority.eligible);
    assert!(
        stale_authority
            .reasons
            .contains(&PublicationBlock::StaleAuthorityGeneration)
    );

    let audited = proposal_for(&snapshot, &generation);
    assert!(
        audited.eligible,
        "audited snapshot should be proposal eligible"
    );
    assert!(!audited.execution_granted());
    assert!(audited.intent.is_some());
}

