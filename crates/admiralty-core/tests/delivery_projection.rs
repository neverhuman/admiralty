use admiralty_core::{
    AcceptanceCriterion, AttemptId, CheckSpec, PrincipalId, ProjectId, RootId, SessionId,
    TaskContract, TaskId, content_digest,
    delivery::*,
    intake::{
        DeliveryRequirement, DeliveryStage, DeliveryTarget, DeliveryTargetKind, TargetRequirement,
    },
    publication::{
        ApprovalEvidence, DecimalU64 as PubDecimalU64, IntentStage, PublicationScope,
        ReconciliationReceipt,
    },
    verification::{
        CheckResult, ReceiptProducer, RequiredNegativeCheck, VerificationInputs,
        VerificationPolicy, VerificationReceipt, validate_receipt,
    },
};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

fn digest(ch: char) -> String {
    format!("sha256:{}", ch.to_string().repeat(64))
}

fn oid(ch: char) -> String {
    ch.to_string().repeat(40)
}

fn task_contract() -> TaskContract {
    TaskContract {
        schema_version: 1,
        project_id: ProjectId::new("admiralty").expect("valid project id"),
        root_id: RootId::new("root-wo5").expect("valid root id"),
        task_id: TaskId::new("task-wo5").expect("valid task id"),
        revision: 2,
        title: "Pure delivery and attention projection".into(),
        objective: "Join exact delivery evidence without conflating stages".into(),
        non_goals: vec!["No live publish or daemon execution".into()],
        owned_paths: vec!["crates/admiralty-core/src/delivery.rs".into()],
        forbidden_paths: vec!["credentials".into()],
        read_first: vec!["docs/plans/admiralty-v2/WORK_ORDER_REVIEW.md".into()],
        dependencies: vec![],
        criteria: vec![AcceptanceCriterion {
            id: "delivery-check".into(),
            text: "All delivery projection invariants hold".into(),
            check_ids: vec!["cargo-test".into()],
        }],
        checks: vec![CheckSpec {
            id: "cargo-test".into(),
            program: "cargo".into(),
            args: vec![
                "test".into(),
                "-p".into(),
                "admiralty-core".into(),
                "--test".into(),
                "delivery_projection".into(),
            ],
            cwd: ".".into(),
            timeout_secs: 120,
        }],
        authority_ref: "grant:wo5-delivery".into(),
        max_attempts: 2,
        simulated: false,
    }
}

fn join_for(
    contract: &TaskContract,
    requirement: &DeliveryRequirement,
    target_id: &str,
) -> DeliveryJoinIdentity {
    DeliveryJoinIdentity {
        project_id: contract.project_id.clone(),
        root_id: contract.root_id.clone(),
        task_id: contract.task_id.clone(),
        contract_revision: contract.revision,
        contract_digest: requirement.contract_digest.clone(),
        attempt_id: AttemptId::new("attempt-1").expect("valid attempt"),
        candidate_ref: "candidate:wo5-1".into(),
        target_id: target_id.into(),
        requirement_generation: requirement.generation,
    }
}

fn claimed_trust(id: &str, ch: char) -> ObservationTrust {
    ObservationTrust::ClaimedEvidence {
        producer_id: PrincipalId::new("verifier-service").expect("valid principal"),
        provenance_ref: format!("evidence:{id}"),
        evidence_digest: digest(ch),
    }
}

fn bundle(id: &str, ver: &str, ch: char, commit_ch: char) -> BundleIdentity {
    BundleIdentity {
        bundle_id: id.into(),
        bundle_digest: digest(ch),
        version: ver.into(),
        source_commit_sha: oid(commit_ch),
    }
}

/// Collect all provenance references claimed inside an observation slice for tests
/// where the caller explicitly attests those provenance references.
fn attest_all_provenance(observations: &[DeliveryObservation]) -> BTreeSet<String> {
    let mut set = BTreeSet::new();
    for obs in observations {
        if let ObservationTrust::ClaimedEvidence { provenance_ref, .. } = &obs.trust {
            set.insert(provenance_ref.clone());
        }
        match &obs.detail {
            StageObservationDetail::Review { approvals, .. } => {
                for a in approvals {
                    set.insert(a.provenance_ref.clone());
                }
            }
            StageObservationDetail::Merge {
                effect_receipt: Some(receipt),
                ..
            } => {
                set.insert(receipt.provenance_ref.clone());
            }
            StageObservationDetail::Running { sessions, .. } => {
                for s in sessions {
                    if let Some(prov) = &s.provenance_ref {
                        set.insert(prov.clone());
                    }
                }
            }
            _ => {}
        }
    }
    set
}

fn context_for(
    contract: TaskContract,
    requirement: DeliveryRequirement,
    default_owner: ResponsibleActor,
    now_tick: u64,
    observations: &[DeliveryObservation],
) -> TrustedDeliveryContext {
    TrustedDeliveryContext {
        contract,
        requirement,
        attempt_id: AttemptId::new("attempt-1").expect("valid attempt"),
        candidate_ref: "candidate:wo5-1".into(),
        default_owner,
        now_tick: DecimalU64::new(now_tick),
        attested_provenance_refs: attest_all_provenance(observations),
        validated_receipts: BTreeMap::new(),
    }
}

fn batch_for(observations: Vec<DeliveryObservation>) -> DeliveryObservationBatch {
    DeliveryObservationBatch {
        schema_version: DELIVERY_PROJECTION_VERSION,
        observations,
        operational_blockers: vec![],
        historical_receipts: vec![],
        prior_attention: vec![],
    }
}

fn full_deployment_observations(
    contract: &TaskContract,
    requirement: &DeliveryRequirement,
    target_id: &str,
    prefix: &str,
    include_release: bool,
) -> Vec<DeliveryObservation> {
    let join = join_for(contract, requirement, target_id);
    let b = bundle("admiralty-bundle", "0.2.0", 'b', '1');
    let mut obs = vec![
        DeliveryObservation {
            observation_id: format!("{prefix}-pkg"),
            join: join.clone(),
            target_kind: DeliveryTargetKind::Deployment,
            trust: claimed_trust(&format!("{prefix}-pkg"), '1'),
            observed_tick: DecimalU64::new(100),
            valid_through_tick: DecimalU64::new(200),
            detail: StageObservationDetail::Package {
                source_commit_sha: oid('1'),
                package_manifest_digest: digest('2'),
                version: "0.2.0".into(),
            },
        },
        DeliveryObservation {
            observation_id: format!("{prefix}-stage"),
            join: join.clone(),
            target_kind: DeliveryTargetKind::Deployment,
            trust: claimed_trust(&format!("{prefix}-stage"), '2'),
            observed_tick: DecimalU64::new(101),
            valid_through_tick: DecimalU64::new(200),
            detail: StageObservationDetail::Stage {
                staged_bundle: b.clone(),
            },
        },
        DeliveryObservation {
            observation_id: format!("{prefix}-install"),
            join: join.clone(),
            target_kind: DeliveryTargetKind::Deployment,
            trust: claimed_trust(&format!("{prefix}-install"), '3'),
            observed_tick: DecimalU64::new(102),
            valid_through_tick: DecimalU64::new(200),
            detail: StageObservationDetail::Install {
                installed_bundle: b.clone(),
                target_install_ref: format!("install:{target_id}:v2"),
                install_receipt_digest: digest('3'),
            },
        },
        DeliveryObservation {
            observation_id: format!("{prefix}-running"),
            join: join.clone(),
            target_kind: DeliveryTargetKind::Deployment,
            trust: claimed_trust(&format!("{prefix}-running"), '4'),
            observed_tick: DecimalU64::new(103),
            valid_through_tick: DecimalU64::new(200),
            detail: StageObservationDetail::Running {
                desired_bundle: b.clone(),
                installed_bundle: Some(b.clone()),
                sessions: vec![SessionExecutionObservation {
                    session_id: SessionId::new(format!("sess-{prefix}")).expect("valid session"),
                    attempt_id: AttemptId::new("attempt-1").expect("valid attempt"),
                    requested_model: "gpt-5.4".into(),
                    requested_bundle_digest: b.bundle_digest.clone(),
                    observed_running_model: Some("gpt-5.4".into()),
                    observed_running_bundle_digest: Some(b.bundle_digest.clone()),
                    provenance_ref: Some(format!("proc:{prefix}:1")),
                    active_child_work: false,
                    unlanded_candidate_ref: None,
                }],
                drain_disposition: DrainDisposition::Idle {},
            },
        },
    ];
    if include_release {
        obs.push(DeliveryObservation {
            observation_id: format!("{prefix}-release"),
            join,
            target_kind: DeliveryTargetKind::Deployment,
            trust: claimed_trust(&format!("{prefix}-release"), '5'),
            observed_tick: DecimalU64::new(104),
            valid_through_tick: DecimalU64::new(200),
            detail: StageObservationDetail::Release {
                released_bundle: b,
                release_ref: format!("release:{target_id}:v2"),
                release_receipt_digest: digest('5'),
            },
        });
    }
    obs
}

fn full_repo_observations(
    contract: &TaskContract,
    requirement: &DeliveryRequirement,
    target_id: &str,
    prefix: &str,
    intent_stage: IntentStage,
) -> Vec<DeliveryObservation> {
    let join = join_for(contract, requirement, target_id);
    let head = oid('1');
    let tree = oid('2');
    let base = oid('3');
    let merged_head = oid('4');
    let effect_receipt =
        (intent_stage == IntentStage::ReconciledSuccess).then(|| ReconciliationReceipt {
            intent_key: format!("intent:{prefix}"),
            request_digest: digest('9'),
            scope: PublicationScope {
                project_id: contract.project_id.clone(),
                root_id: contract.root_id.clone(),
                task_id: contract.task_id.clone(),
                target_ref: "refs/heads/main".into(),
                authority_generation: PubDecimalU64::new(1),
            },
            expected_head: head.clone(),
            expected_base: base.clone(),
            authority_generation: PubDecimalU64::new(1),
            observed_head: merged_head.clone(),
            effect_occurred: true,
            provenance_ref: format!("forge:merge:{prefix}"),
        });
    let rescue_binding =
        (intent_stage != IntentStage::ReconciledSuccess).then(|| RescueCandidateBinding {
            task_id: contract.task_id.clone(),
            attempt_id: AttemptId::new("attempt-1").expect("valid attempt"),
            candidate_ref: "candidate:wo5-1".into(),
            source_commit_sha: head.clone(),
            source_tree_sha: tree.clone(),
            rescue_artifact_digest: digest('8'),
            rescue_ref: format!("refs/heads/rescue/{target_id}"),
            force_push_permitted: false,
        });

    vec![
        DeliveryObservation {
            observation_id: format!("{prefix}-impl"),
            join: join.clone(),
            target_kind: DeliveryTargetKind::Repository,
            trust: claimed_trust(&format!("{prefix}-impl"), 'a'),
            observed_tick: DecimalU64::new(100),
            valid_through_tick: DecimalU64::new(200),
            detail: StageObservationDetail::Implementation {
                source_commit_sha: head.clone(),
                source_tree_sha: tree.clone(),
                source_artifact_digest: digest('a'),
                tracking_trailers: vec!["Task-Ref: task-wo5".into()],
            },
        },
        DeliveryObservation {
            observation_id: format!("{prefix}-check"),
            join: join.clone(),
            target_kind: DeliveryTargetKind::Repository,
            trust: claimed_trust(&format!("{prefix}-check"), 'b'),
            observed_tick: DecimalU64::new(101),
            valid_through_tick: DecimalU64::new(200),
            detail: StageObservationDetail::Check {
                source_commit_sha: head.clone(),
                source_tree_sha: tree,
                producer: ReceiptProducer::IndependentVerifier,
                all_checks_passed: true,
                negative_checks_passed: true,
                receipt_digest: digest('b'),
            },
        },
        DeliveryObservation {
            observation_id: format!("{prefix}-review"),
            join: join.clone(),
            target_kind: DeliveryTargetKind::Repository,
            trust: claimed_trust(&format!("{prefix}-review"), 'c'),
            observed_tick: DecimalU64::new(102),
            valid_through_tick: DecimalU64::new(200),
            detail: StageObservationDetail::Review {
                head_commit_sha: head.clone(),
                required_reviewers: vec!["neverhumanbot".into()],
                approvals: vec![ApprovalEvidence {
                    reviewer: "neverhumanbot".into(),
                    head: head.clone(),
                    dismissed: false,
                    provenance_ref: format!("forge:review:{prefix}"),
                }],
            },
        },
        DeliveryObservation {
            observation_id: format!("{prefix}-merge"),
            join,
            target_kind: DeliveryTargetKind::Repository,
            trust: claimed_trust(&format!("{prefix}-merge"), 'd'),
            observed_tick: DecimalU64::new(103),
            valid_through_tick: DecimalU64::new(200),
            detail: StageObservationDetail::Merge {
                target_ref: "refs/heads/main".into(),
                expected_head_sha: head,
                expected_base_sha: base,
                observed_head_sha: (intent_stage == IntentStage::ReconciledSuccess)
                    .then_some(merged_head),
                intent_key: format!("intent:{prefix}"),
                intent_stage,
                effect_receipt,
                rescue_binding,
            },
        },
    ]
}

#[test]
fn two_targets_only_one_released_keeps_delivery_incomplete() {
    let contract = task_contract();
    let requirement = DeliveryRequirement {
        schema_version: 1,
        task_id: contract.task_id.clone(),
        contract_revision: contract.revision,
        contract_digest: content_digest(&contract).expect("contract digest"),
        generation: 1,
        targets: vec![
            DeliveryTarget {
                id: "deploy-canary".into(),
                target_kind: DeliveryTargetKind::Deployment,
                requirement: TargetRequirement::Required {
                    stage: DeliveryStage::Released,
                },
            },
            DeliveryTarget {
                id: "deploy-prod".into(),
                target_kind: DeliveryTargetKind::Deployment,
                requirement: TargetRequirement::Required {
                    stage: DeliveryStage::Released,
                },
            },
        ],
    };

    let mut observations =
        full_deployment_observations(&contract, &requirement, "deploy-canary", "canary", true);
    observations.extend(full_deployment_observations(
        &contract,
        &requirement,
        "deploy-prod",
        "prod",
        false,
    ));

    let ctx = context_for(
        contract,
        requirement,
        ResponsibleActor::RoleQueue {
            role: ResponsibleRole::Commodore,
        },
        110,
        &observations,
    );
    let batch = batch_for(observations);
    let raw = serde_json::to_vec(&batch).expect("serialize batch");
    let projected = parse_and_project_delivery(&ctx, &raw).expect("project delivery");

    assert!(!projected.overall_satisfied);
    assert_eq!(
        projected.completion_state,
        DeliveryCompletionState::Incomplete
    );
    assert!(!projected.acceptance_granted());
    assert!(!projected.release_authorized());
    assert!(!projected.dispatch_authorized());
    assert!(!projected.manager_wake_issued());
    assert!(!projected.source_authenticated_by_projection());
    assert!(!projected.attention_delta.manager_wake_issued());
    assert!(!projected.attention_delta.model_escalation_permitted());
    assert_eq!(
        projected.targets[0].completion,
        TargetCompletionState::Satisfied {
            required_stage: DeliveryStage::Released
        }
    );
    assert_eq!(
        projected.targets[0].highest_verified_stage,
        Some(DeliveryStage::Released)
    );
    assert_eq!(
        projected.targets[1].highest_verified_stage,
        Some(DeliveryStage::Running)
    );
    assert!(matches!(
        projected.targets[1].completion,
        TargetCompletionState::Unsatisfied {
            required_stage: DeliveryStage::Released,
            ..
        }
    ));
    assert!(
        projected
            .attention_items
            .iter()
            .any(|item| item.subject.target_id.as_deref() == Some("deploy-prod"))
    );
}

#[test]
fn missing_reviews_checks_and_unattested_provenance_never_grant_delivery() {
    let contract = task_contract();
    let requirement = DeliveryRequirement {
        schema_version: 1,
        task_id: contract.task_id.clone(),
        contract_revision: contract.revision,
        contract_digest: content_digest(&contract).expect("contract digest"),
        generation: 1,
        targets: vec![DeliveryTarget {
            id: "repo-main".into(),
            target_kind: DeliveryTargetKind::Repository,
            requirement: TargetRequirement::Required {
                stage: DeliveryStage::Merged,
            },
        }],
    };
    let join = join_for(&contract, &requirement, "repo-main");
    let head = oid('1');
    let tree = oid('2');

    // Case A: Passing gate check claim with ProviderClaim, dismissed review, and tracking trailer hint.
    let observations = vec![
        DeliveryObservation {
            observation_id: "obs-trailer-only".into(),
            join: join.clone(),
            target_kind: DeliveryTargetKind::Repository,
            trust: ObservationTrust::TrackingTrailerHint {
                trailer: "Released-By: worker-claim".into(),
            },
            observed_tick: DecimalU64::new(100),
            valid_through_tick: DecimalU64::new(200),
            detail: StageObservationDetail::Implementation {
                source_commit_sha: head.clone(),
                source_tree_sha: tree.clone(),
                source_artifact_digest: digest('a'),
                tracking_trailers: vec!["Accepted-On: main".into()],
            },
        },
        DeliveryObservation {
            observation_id: "obs-provider-claim-check".into(),
            join: join.clone(),
            target_kind: DeliveryTargetKind::Repository,
            trust: claimed_trust("obs-provider-claim-check", 'b'),
            observed_tick: DecimalU64::new(101),
            valid_through_tick: DecimalU64::new(200),
            detail: StageObservationDetail::Check {
                source_commit_sha: head.clone(),
                source_tree_sha: tree.clone(),
                producer: ReceiptProducer::ProviderClaim,
                all_checks_passed: true,
                negative_checks_passed: true,
                receipt_digest: digest('b'),
            },
        },
        DeliveryObservation {
            observation_id: "obs-dismissed-review".into(),
            join,
            target_kind: DeliveryTargetKind::Repository,
            trust: claimed_trust("obs-dismissed-review", 'c'),
            observed_tick: DecimalU64::new(102),
            valid_through_tick: DecimalU64::new(200),
            detail: StageObservationDetail::Review {
                head_commit_sha: head.clone(),
                required_reviewers: vec!["neverhumanbot".into()],
                approvals: vec![ApprovalEvidence {
                    reviewer: "neverhumanbot".into(),
                    head,
                    dismissed: true,
                    provenance_ref: "forge:review:dismissed".into(),
                }],
            },
        },
    ];

    let ctx = context_for(
        contract.clone(),
        requirement.clone(),
        ResponsibleActor::Unassigned {},
        105,
        &observations,
    );
    let projected = project_delivery(&ctx, &batch_for(observations)).expect("project delivery");
    assert!(!projected.overall_satisfied);
    let t = &projected.targets[0];
    assert_eq!(t.stages.implementation, StageEvaluation::UntrustedClaim);
    assert_eq!(t.stages.check, StageEvaluation::UntrustedClaim);
    assert_eq!(t.stages.review, StageEvaluation::Failed);
    assert_eq!(t.stages.merge, StageEvaluation::Unknown);
    assert_eq!(t.highest_verified_stage, None);
    assert_eq!(
        t.lookup_hints,
        vec![
            "Released-By: worker-claim".to_owned(),
            "Accepted-On: main".to_owned()
        ]
    );
    assert!(
        projected
            .attention_items
            .iter()
            .any(|i| i.reason == AttentionReason::FailedVerificationCheck)
    );
    assert!(
        projected
            .attention_items
            .iter()
            .any(|i| i.reason == AttentionReason::MissingReviewApproval)
    );
    assert!(
        projected
            .attention_items
            .iter()
            .all(|i| i.owner == ResponsibleActor::Unassigned {})
    );

    // Case B: JSON claims `IndependentVerifier`, `ApprovalEvidence`, and `ReconciliationReceipt`,
    // but the caller's non-Deserialize `TrustedDeliveryContext` has NOT attested their provenance_refs!
    let unattested_obs = full_repo_observations(
        &contract,
        &requirement,
        "repo-main",
        "unattested",
        IntentStage::ReconciledSuccess,
    );
    let mut unattested_ctx = context_for(
        contract.clone(),
        requirement.clone(),
        ResponsibleActor::Unassigned {},
        105,
        &[],
    );
    let unattested_proj = project_delivery(&unattested_ctx, &batch_for(unattested_obs.clone()))
        .expect("project unattested");
    assert!(!unattested_proj.overall_satisfied);
    let ut = &unattested_proj.targets[0];
    assert_eq!(
        ut.stages.implementation,
        StageEvaluation::UnattestedProvenance
    );
    assert_eq!(ut.stages.check, StageEvaluation::UnattestedProvenance);
    assert_eq!(ut.stages.review, StageEvaluation::UnattestedProvenance);
    assert_eq!(ut.stages.merge, StageEvaluation::UnattestedProvenance);
    assert_eq!(ut.highest_verified_stage, None);

    // Case C: Observation wrapper provenance is attested, but inner `ApprovalEvidence.provenance_ref`
    // and `ReconciliationReceipt.provenance_ref` are NOT attested, and `ValidatedReceipt` binding is
    // required by context for `Check` but missing for `unattested-check`!
    unattested_ctx.attested_provenance_refs = BTreeSet::from([
        "evidence:unattested-impl".to_owned(),
        "evidence:unattested-check".to_owned(),
        "evidence:unattested-review".to_owned(),
        "evidence:unattested-merge".to_owned(),
    ]);
    // Insert a ValidatedReceipt for a different receipt digest so `validated_receipts` is non-empty,
    // proving `IndependentVerifier` in JSON cannot bypass a missing ValidatedReceipt binding.
    let mut neg_check = contract.checks[0].clone();
    neg_check.id = "neg-cargo-test".into();
    let policy = VerificationPolicy {
        schema_version: 1,
        verifier: PrincipalId::new("verifier-service").expect("valid principal"),
        protected_oracle_digest: digest('9'),
        required_negative_checks: vec![RequiredNegativeCheck {
            check: neg_check.clone(),
            expected_exit_code: 0,
            fixture_digest: digest('8'),
        }],
    };
    let policy_digest = content_digest(&policy).expect("policy digest");
    let v_receipt = VerificationReceipt {
        schema_version: 1,
        receipt_id: "vr-other".into(),
        producer: ReceiptProducer::IndependentVerifier,
        verifier: PrincipalId::new("verifier-service").expect("valid principal"),
        inputs: VerificationInputs {
            project_id: contract.project_id.clone(),
            root_id: contract.root_id.clone(),
            task_id: contract.task_id.clone(),
            attempt_id: AttemptId::new("attempt-1").expect("valid attempt"),
            candidate_ref: "candidate:wo5-1".into(),
            contract_revision: contract.revision,
            contract_digest: requirement.contract_digest.clone(),
            verification_policy_digest: policy_digest,
            source_commit_sha: oid('1'),
            source_tree_sha: oid('2'),
            source_artifact_digest: digest('a'),
            protected_oracle_digest: digest('9'),
        },
        checks: vec![CheckResult {
            check: contract.checks[0].clone(),
            exit_code: Some(0),
            stdout_digest: digest('6'),
            stderr_digest: digest('7'),
        }],
        negative_checks: vec![CheckResult {
            check: neg_check,
            exit_code: Some(0),
            stdout_digest: digest('6'),
            stderr_digest: digest('7'),
        }],
    };
    let observed_vr =
        validate_receipt(&contract, &v_receipt.inputs, &policy, &v_receipt).expect("validate vr");
    unattested_ctx
        .validated_receipts
        .insert(observed_vr.receipt_digest().to_owned(), observed_vr);

    let inner_unattested = project_delivery(&unattested_ctx, &batch_for(unattested_obs))
        .expect("project inner unattested");
    assert!(!inner_unattested.overall_satisfied);
    let it = &inner_unattested.targets[0];
    assert_eq!(it.stages.implementation, StageEvaluation::Verified);
    assert_eq!(it.stages.check, StageEvaluation::UnattestedProvenance);
    assert_eq!(it.stages.review, StageEvaluation::UnattestedProvenance);
    assert_eq!(it.stages.merge, StageEvaluation::UnattestedProvenance);
    assert_eq!(it.highest_verified_stage, Some(DeliveryStage::Candidate));
}

#[test]
fn wrong_contract_attempt_candidate_target_and_generation_rejected_while_retaining_old_receipts() {
    let contract = task_contract();
    let requirement = DeliveryRequirement {
        schema_version: 1,
        task_id: contract.task_id.clone(),
        contract_revision: contract.revision,
        contract_digest: content_digest(&contract).expect("contract digest"),
        generation: 2,
        targets: vec![DeliveryTarget {
            id: "repo-main".into(),
            target_kind: DeliveryTargetKind::Repository,
            requirement: TargetRequirement::Required {
                stage: DeliveryStage::Candidate,
            },
        }],
    };

    let base_join = join_for(&contract, &requirement, "repo-main");
    let mut wrong_task = base_join.clone();
    wrong_task.task_id = TaskId::new("other-task").expect("valid task");
    let mut wrong_rev = base_join.clone();
    wrong_rev.contract_revision = 99;
    let mut wrong_digest = base_join.clone();
    wrong_digest.contract_digest = digest('f');
    let mut wrong_attempt = base_join.clone();
    wrong_attempt.attempt_id = AttemptId::new("attempt-2").expect("valid attempt");
    let mut wrong_candidate = base_join.clone();
    wrong_candidate.candidate_ref = "candidate:other".into();
    let mut wrong_target = base_join.clone();
    wrong_target.target_id = "undeclared-target".into();
    let mut superseded_gen = base_join.clone();
    superseded_gen.requirement_generation = 1;

    let make_obs = |id: &str, join: DeliveryJoinIdentity| DeliveryObservation {
        observation_id: id.into(),
        join,
        target_kind: DeliveryTargetKind::Repository,
        trust: claimed_trust(id, 'a'),
        observed_tick: DecimalU64::new(100),
        valid_through_tick: DecimalU64::new(200),
        detail: StageObservationDetail::Implementation {
            source_commit_sha: oid('1'),
            source_tree_sha: oid('2'),
            source_artifact_digest: digest('a'),
            tracking_trailers: vec![],
        },
    };

    let historical = HistoricalDeliveryReceipt {
        receipt_id: "hist-gen1-receipt".into(),
        join: superseded_gen.clone(),
        satisfied_stage: DeliveryStage::Merged,
        receipt_digest: digest('7'),
        recorded_tick: DecimalU64::new(50),
    };

    let observations = vec![
        make_obs("bad-task", wrong_task),
        make_obs("bad-rev", wrong_rev),
        make_obs("bad-digest", wrong_digest),
        make_obs("bad-attempt", wrong_attempt),
        make_obs("bad-candidate", wrong_candidate),
        make_obs("bad-target", wrong_target),
        make_obs("superseded-gen1", superseded_gen),
    ];
    let ctx = context_for(
        contract,
        requirement,
        ResponsibleActor::RoleQueue {
            role: ResponsibleRole::Commodore,
        },
        110,
        &observations,
    );
    let batch = DeliveryObservationBatch {
        schema_version: DELIVERY_PROJECTION_VERSION,
        observations,
        operational_blockers: vec![],
        historical_receipts: vec![historical.clone()],
        prior_attention: vec![],
    };

    let projected = project_delivery(&ctx, &batch).expect("project delivery");
    assert!(!projected.overall_satisfied);
    assert_eq!(projected.targets[0].highest_verified_stage, None);
    assert_eq!(projected.rejected_observations.len(), 7);
    let reasons: Vec<JoinMismatchReason> = projected
        .rejected_observations
        .iter()
        .map(|r| r.reason)
        .collect();
    assert_eq!(
        reasons,
        vec![
            JoinMismatchReason::TaskId,
            JoinMismatchReason::ContractRevision,
            JoinMismatchReason::ContractDigest,
            JoinMismatchReason::AttemptId,
            JoinMismatchReason::CandidateRef,
            JoinMismatchReason::UnknownTargetId,
            JoinMismatchReason::SupersededGeneration,
        ]
    );
    assert_eq!(projected.retained_historical_receipts, vec![historical]);
    assert!(
        projected
            .attention_items
            .iter()
            .any(|i| i.reason == AttentionReason::JoinIdentityMismatch)
    );
}

#[test]
fn unknown_vs_not_applicable_and_optional_targets_and_stages() {
    let contract = task_contract();
    let mut requirement = DeliveryRequirement {
        schema_version: 1,
        task_id: contract.task_id.clone(),
        contract_revision: contract.revision,
        contract_digest: content_digest(&contract).expect("contract digest"),
        generation: 1,
        targets: vec![
            DeliveryTarget {
                id: "repo-main".into(),
                target_kind: DeliveryTargetKind::Repository,
                requirement: TargetRequirement::Required {
                    stage: DeliveryStage::Merged,
                },
            },
            DeliveryTarget {
                id: "deploy-na".into(),
                target_kind: DeliveryTargetKind::Deployment,
                requirement: TargetRequirement::NotApplicable {
                    reason: "source-only library change".into(),
                },
            },
            DeliveryTarget {
                id: "deploy-optional".into(),
                target_kind: DeliveryTargetKind::Deployment,
                requirement: TargetRequirement::Optional {
                    stage: DeliveryStage::Staged,
                },
            },
            DeliveryTarget {
                id: "deploy-unknown".into(),
                target_kind: DeliveryTargetKind::Deployment,
                requirement: TargetRequirement::Unknown {},
            },
        ],
    };

    let observations = full_repo_observations(
        &contract,
        &requirement,
        "repo-main",
        "repo",
        IntentStage::ReconciledSuccess,
    );

    let mut ctx = context_for(
        contract,
        requirement.clone(),
        ResponsibleActor::RoleQueue {
            role: ResponsibleRole::Commodore,
        },
        110,
        &observations,
    );
    let batch = batch_for(observations);

    let blocked = project_delivery(&ctx, &batch).expect("project with unknown target");
    assert!(!blocked.overall_satisfied);
    assert_eq!(
        blocked.completion_state,
        DeliveryCompletionState::BlockedByUnknownRequirement
    );
    assert_eq!(
        blocked.targets[1].completion,
        TargetCompletionState::NotApplicable {
            reason: "source-only library change".into()
        }
    );
    assert_eq!(
        blocked.targets[1].stages.release,
        StageEvaluation::NotApplicable
    );
    assert_eq!(
        blocked.targets[2].completion,
        TargetCompletionState::OptionalPending {
            stage: DeliveryStage::Staged
        }
    );
    assert_eq!(blocked.targets[2].stages.stage, StageEvaluation::Unknown);
    assert_eq!(
        blocked.targets[3].completion,
        TargetCompletionState::RequirementUnknown {}
    );

    requirement.targets[3].requirement = TargetRequirement::NotApplicable {
        reason: "no secondary cluster enrolled".into(),
    };
    ctx.requirement = requirement;
    let satisfied = project_delivery(&ctx, &batch).expect("project after resolving unknown target");
    assert!(satisfied.overall_satisfied);
    assert_eq!(
        satisfied.completion_state,
        DeliveryCompletionState::Satisfied
    );
}

#[test]
fn stale_observations_and_backward_logical_clock_rejected_for_satisfaction() {
    let contract = task_contract();
    let requirement = DeliveryRequirement {
        schema_version: 1,
        task_id: contract.task_id.clone(),
        contract_revision: contract.revision,
        contract_digest: content_digest(&contract).expect("contract digest"),
        generation: 1,
        targets: vec![DeliveryTarget {
            id: "repo-main".into(),
            target_kind: DeliveryTargetKind::Repository,
            requirement: TargetRequirement::Required {
                stage: DeliveryStage::Verified,
            },
        }],
    };
    let join = join_for(&contract, &requirement, "repo-main");

    let observations = vec![
        DeliveryObservation {
            observation_id: "obs-stale-impl".into(),
            join: join.clone(),
            target_kind: DeliveryTargetKind::Repository,
            trust: claimed_trust("obs-stale-impl", 'a'),
            observed_tick: DecimalU64::new(50),
            valid_through_tick: DecimalU64::new(90),
            detail: StageObservationDetail::Implementation {
                source_commit_sha: oid('1'),
                source_tree_sha: oid('2'),
                source_artifact_digest: digest('a'),
                tracking_trailers: vec![],
            },
        },
        DeliveryObservation {
            observation_id: "obs-future-check".into(),
            join,
            target_kind: DeliveryTargetKind::Repository,
            trust: claimed_trust("obs-future-check", 'b'),
            observed_tick: DecimalU64::new(120),
            valid_through_tick: DecimalU64::new(200),
            detail: StageObservationDetail::Check {
                source_commit_sha: oid('1'),
                source_tree_sha: oid('2'),
                producer: ReceiptProducer::IndependentVerifier,
                all_checks_passed: true,
                negative_checks_passed: true,
                receipt_digest: digest('b'),
            },
        },
    ];

    let ctx = context_for(
        contract,
        requirement,
        ResponsibleActor::RoleQueue {
            role: ResponsibleRole::Commodore,
        },
        100,
        &observations,
    );
    let projected = project_delivery(&ctx, &batch_for(observations)).expect("project delivery");
    assert!(!projected.overall_satisfied);
    assert_eq!(
        projected.targets[0].stages.implementation,
        StageEvaluation::Stale
    );
    assert_eq!(
        projected.targets[0].stages.check,
        StageEvaluation::ClockSkewBackward
    );
    assert_eq!(projected.targets[0].highest_verified_stage, None);
    let stale_item = projected
        .attention_items
        .iter()
        .find(|i| i.reason == AttentionReason::StaleOrBackwardObservation)
        .expect("stale/backward attention item");
    assert_eq!(stale_item.freshness, FreshnessState::ClockSkewBackward);
    assert_eq!(
        stale_item.next_action,
        NextAuthorizedAction::RefreshObservation
    );
}

#[test]
fn partial_multi_repo_landing_and_uncertain_external_outcome_with_rescue_binding() {
    let contract = task_contract();
    let requirement = DeliveryRequirement {
        schema_version: 1,
        task_id: contract.task_id.clone(),
        contract_revision: contract.revision,
        contract_digest: content_digest(&contract).expect("contract digest"),
        generation: 1,
        targets: vec![
            DeliveryTarget {
                id: "repo-core".into(),
                target_kind: DeliveryTargetKind::Repository,
                requirement: TargetRequirement::Required {
                    stage: DeliveryStage::Merged,
                },
            },
            DeliveryTarget {
                id: "repo-plugin".into(),
                target_kind: DeliveryTargetKind::Repository,
                requirement: TargetRequirement::Required {
                    stage: DeliveryStage::Merged,
                },
            },
        ],
    };

    let mut observations = full_repo_observations(
        &contract,
        &requirement,
        "repo-core",
        "core",
        IntentStage::ReconciledSuccess,
    );
    observations.extend(full_repo_observations(
        &contract,
        &requirement,
        "repo-plugin",
        "plugin",
        IntentStage::Proposed,
    ));

    let ctx = context_for(
        contract.clone(),
        requirement.clone(),
        ResponsibleActor::RoleQueue {
            role: ResponsibleRole::Commodore,
        },
        110,
        &observations,
    );
    let partial =
        project_delivery(&ctx, &batch_for(observations)).expect("project partial landing");
    assert!(!partial.overall_satisfied);
    assert_eq!(
        partial.completion_state,
        DeliveryCompletionState::BlockedByPartialLanding
    );
    assert_eq!(
        partial.multi_repo_landing,
        MultiRepoLandingState::PartialLanding {
            landed_targets: vec!["repo-core".into()],
            unlanded_targets: vec!["repo-plugin".into()],
        }
    );
    assert_eq!(
        partial.targets[1].external_outcome,
        ExternalOutcomeState::Unlanded {
            target_ref: "refs/heads/main".into(),
            rescue_preserved: true,
            rescue_ref: Some("refs/heads/rescue/repo-plugin".into()),
        }
    );
    assert_eq!(
        partial.headline_attention_reason,
        Some(AttentionReason::PartialMultiRepoLanding)
    );

    let mut uncertain_obs = full_repo_observations(
        &contract,
        &requirement,
        "repo-core",
        "core",
        IntentStage::ReconciledSuccess,
    );
    let mut plugin_ambiguous = full_repo_observations(
        &contract,
        &requirement,
        "repo-plugin",
        "plugin",
        IntentStage::Ambiguous,
    );
    if let StageObservationDetail::Merge {
        rescue_binding: Some(rescue),
        ..
    } = &mut plugin_ambiguous[3].detail
    {
        rescue.force_push_permitted = true;
    }
    uncertain_obs.extend(plugin_ambiguous);
    let uncertain_ctx = context_for(
        contract,
        requirement,
        ResponsibleActor::RoleQueue {
            role: ResponsibleRole::Commodore,
        },
        110,
        &uncertain_obs,
    );

    let uncertain =
        project_delivery(&uncertain_ctx, &batch_for(uncertain_obs)).expect("project uncertain");
    assert!(!uncertain.overall_satisfied);
    assert_eq!(
        uncertain.completion_state,
        DeliveryCompletionState::BlockedByUncertainOutcome
    );
    assert_eq!(
        uncertain.multi_repo_landing,
        MultiRepoLandingState::UncertainOutcome {
            uncertain_targets: vec!["repo-plugin".into()],
            landed_targets: vec!["repo-core".into()],
            unlanded_targets: vec![],
        }
    );
    assert_eq!(
        uncertain.headline_attention_reason,
        Some(AttentionReason::UncertainExternalOutcome)
    );
    assert!(
        uncertain
            .attention_items
            .iter()
            .any(|i| i.reason == AttentionReason::UnpreservedRescueWork)
    );
}

#[test]
fn stale_installed_bundle_and_draining_sessions_remain_distinct_from_desired_bundle() {
    let contract = task_contract();
    let requirement = DeliveryRequirement {
        schema_version: 1,
        task_id: contract.task_id.clone(),
        contract_revision: contract.revision,
        contract_digest: content_digest(&contract).expect("contract digest"),
        generation: 1,
        targets: vec![DeliveryTarget {
            id: "deploy-prod".into(),
            target_kind: DeliveryTargetKind::Deployment,
            requirement: TargetRequirement::Required {
                stage: DeliveryStage::Running,
            },
        }],
    };
    let join = join_for(&contract, &requirement, "deploy-prod");
    let desired_v2 = bundle("admiralty-bundle", "0.2.0", 'b', '2');
    let installed_v1 = bundle("admiralty-bundle", "0.1.0", 'a', '1');

    let observations = vec![
        DeliveryObservation {
            observation_id: "prod-pkg".into(),
            join: join.clone(),
            target_kind: DeliveryTargetKind::Deployment,
            trust: claimed_trust("prod-pkg", '1'),
            observed_tick: DecimalU64::new(100),
            valid_through_tick: DecimalU64::new(200),
            detail: StageObservationDetail::Package {
                source_commit_sha: oid('2'),
                package_manifest_digest: digest('2'),
                version: "0.2.0".into(),
            },
        },
        DeliveryObservation {
            observation_id: "prod-stage".into(),
            join: join.clone(),
            target_kind: DeliveryTargetKind::Deployment,
            trust: claimed_trust("prod-stage", '2'),
            observed_tick: DecimalU64::new(101),
            valid_through_tick: DecimalU64::new(200),
            detail: StageObservationDetail::Stage {
                staged_bundle: desired_v2.clone(),
            },
        },
        DeliveryObservation {
            observation_id: "prod-install-stale".into(),
            join: join.clone(),
            target_kind: DeliveryTargetKind::Deployment,
            trust: claimed_trust("prod-install-stale", '3'),
            observed_tick: DecimalU64::new(102),
            valid_through_tick: DecimalU64::new(200),
            detail: StageObservationDetail::Install {
                installed_bundle: installed_v1.clone(),
                target_install_ref: "install:deploy-prod:v1".into(),
                install_receipt_digest: digest('3'),
            },
        },
        DeliveryObservation {
            observation_id: "prod-running-stale".into(),
            join,
            target_kind: DeliveryTargetKind::Deployment,
            trust: claimed_trust("prod-running-stale", '4'),
            observed_tick: DecimalU64::new(103),
            valid_through_tick: DecimalU64::new(200),
            detail: StageObservationDetail::Running {
                desired_bundle: desired_v2.clone(),
                installed_bundle: Some(installed_v1.clone()),
                sessions: vec![SessionExecutionObservation {
                    session_id: SessionId::new("sess-1").expect("valid session"),
                    attempt_id: AttemptId::new("attempt-1").expect("valid attempt"),
                    requested_model: "gpt-5.4".into(),
                    requested_bundle_digest: desired_v2.bundle_digest.clone(),
                    observed_running_model: Some("gpt-5.4".into()),
                    observed_running_bundle_digest: Some(installed_v1.bundle_digest.clone()),
                    provenance_ref: Some("proc:sess-1".into()),
                    active_child_work: true,
                    unlanded_candidate_ref: Some("candidate:active-child".into()),
                }],
                drain_disposition: DrainDisposition::Draining {
                    active_sessions: 1,
                    preserved_candidate_refs: vec!["candidate:active-child".into()],
                },
            },
        },
    ];

    let ctx = context_for(
        contract,
        requirement,
        ResponsibleActor::RoleQueue {
            role: ResponsibleRole::Commodore,
        },
        110,
        &observations,
    );
    let projected =
        project_delivery(&ctx, &batch_for(observations)).expect("project stale installed bundle");
    assert!(!projected.overall_satisfied);
    let t = &projected.targets[0];
    assert_eq!(t.highest_verified_stage, Some(DeliveryStage::Staged));
    assert_eq!(t.stages.install, StageEvaluation::Failed);
    assert_eq!(t.stages.running, StageEvaluation::Failed);
    let rt = t
        .bundle_runtime
        .as_ref()
        .expect("bundle runtime projection");
    assert_eq!(rt.desired_bundle, Some(desired_v2));
    assert_eq!(rt.installed_bundle, Some(installed_v1));
    assert_eq!(rt.alignment, BundleAlignmentState::StaleInstalledBundle);
    assert!(!rt.sessions[0].matches_desired_bundle);
    assert!(
        projected
            .attention_items
            .iter()
            .any(|i| i.reason == AttentionReason::StaleInstalledBundle)
    );
}

#[test]
fn actual_running_identity_unknown_never_accepts_requested_model_or_bundle_substitute() {
    let contract = task_contract();
    let requirement = DeliveryRequirement {
        schema_version: 1,
        task_id: contract.task_id.clone(),
        contract_revision: contract.revision,
        contract_digest: content_digest(&contract).expect("contract digest"),
        generation: 1,
        targets: vec![DeliveryTarget {
            id: "deploy-prod".into(),
            target_kind: DeliveryTargetKind::Deployment,
            requirement: TargetRequirement::Required {
                stage: DeliveryStage::Running,
            },
        }],
    };
    let mut observations =
        full_deployment_observations(&contract, &requirement, "deploy-prod", "prod", false);
    if let StageObservationDetail::Running { sessions, .. } = &mut observations[3].detail {
        sessions[0].observed_running_model = None;
    }

    let ctx = context_for(
        contract,
        requirement,
        ResponsibleActor::RoleQueue {
            role: ResponsibleRole::Commodore,
        },
        110,
        &observations,
    );
    let projected =
        project_delivery(&ctx, &batch_for(observations)).expect("project unknown running identity");
    assert!(!projected.overall_satisfied);
    let t = &projected.targets[0];
    assert_eq!(t.highest_verified_stage, Some(DeliveryStage::Installed));
    assert_eq!(t.stages.running, StageEvaluation::Unknown);
    let rt = t
        .bundle_runtime
        .as_ref()
        .expect("bundle runtime projection");
    assert_eq!(rt.alignment, BundleAlignmentState::ActualRunningUnknown);
    assert_eq!(rt.sessions[0].requested_model, "gpt-5.4");
    assert_eq!(rt.sessions[0].observed_running_model, None);
    assert!(!rt.sessions[0].running_identity_known);
    assert!(!rt.sessions[0].matches_desired_bundle);
    assert_eq!(
        projected.headline_attention_reason,
        Some(AttentionReason::ActualRunningIdentityUnknown)
    );
}

#[test]
fn immutable_first_observed_wait_age_and_no_manager_wake_for_unchanged_observation() {
    let contract = task_contract();
    let requirement = DeliveryRequirement {
        schema_version: 1,
        task_id: contract.task_id.clone(),
        contract_revision: contract.revision,
        contract_digest: content_digest(&contract).expect("contract digest"),
        generation: 1,
        targets: vec![DeliveryTarget {
            id: "repo-main".into(),
            target_kind: DeliveryTargetKind::Repository,
            requirement: TargetRequirement::Required {
                stage: DeliveryStage::Merged,
            },
        }],
    };
    let mut observations = full_repo_observations(
        &contract,
        &requirement,
        "repo-main",
        "repo",
        IntentStage::ReconciledSuccess,
    );
    observations.truncate(2);

    let mut first_ctx = context_for(
        contract,
        requirement,
        ResponsibleActor::Assigned {
            role: ResponsibleRole::Commodore,
            principal_id: PrincipalId::new("commodore-1").expect("valid principal"),
        },
        110,
        &observations,
    );
    let first_batch = DeliveryObservationBatch {
        schema_version: DELIVERY_PROJECTION_VERSION,
        observations: observations.clone(),
        operational_blockers: vec![OperationalBlockerObservation {
            blocker_id: "blk-pressure".into(),
            target_id: Some("repo-main".into()),
            owner: ResponsibleActor::Assigned {
                role: ResponsibleRole::Commodore,
                principal_id: PrincipalId::new("commodore-1").expect("valid principal"),
            },
            provenance_ref: "node:xbabe2:pressure:1".into(),
            observed_tick: DecimalU64::new(110),
            valid_through_tick: DecimalU64::new(300),
            blocker: OperationalBlockerKind::ResourcePressureWait {
                resource_dimension: "cpu_busy_pct".into(),
                detail: "host CPU > 85%".into(),
            },
        }],
        historical_receipts: vec![],
        prior_attention: vec![],
    };

    let first_proj = project_delivery(&first_ctx, &first_batch).expect("first projection");
    assert!(first_proj.attention_delta.actionable_facts_changed);
    assert!(!first_proj.attention_delta.manager_wake_issued());
    assert!(!first_proj.attention_delta.model_escalation_permitted());
    assert!(!first_proj.manager_wake_issued());
    assert_eq!(
        first_proj.attention_delta.changed_items.len(),
        first_proj.attention_items.len()
    );
    for item in &first_proj.attention_items {
        assert_eq!(item.first_observed_tick, DecimalU64::new(110));
        assert_eq!(item.wait_age_ticks, DecimalU64::new(0));
    }

    // Re-evaluate identical observations at tick 155 with prior_attention from tick 110.
    first_ctx.now_tick = DecimalU64::new(155);
    let second_batch = DeliveryObservationBatch {
        prior_attention: first_proj.attention_items.clone(),
        ..first_batch
    };
    let second_proj = project_delivery(&first_ctx, &second_batch).expect("second projection");
    assert!(!second_proj.attention_delta.actionable_facts_changed);
    assert!(!second_proj.attention_delta.manager_wake_issued());
    assert!(second_proj.attention_delta.changed_items.is_empty());
    assert_eq!(
        second_proj.attention_delta.unchanged_item_ids.len(),
        second_proj.attention_items.len()
    );
    for (before, after) in first_proj
        .attention_items
        .iter()
        .zip(&second_proj.attention_items)
    {
        assert_eq!(after.first_observed_tick, DecimalU64::new(110));
        assert_eq!(after.first_observed_ref, before.first_observed_ref);
        assert_eq!(after.last_observed_tick, DecimalU64::new(155));
        assert_eq!(after.wait_age_ticks, DecimalU64::new(45));
        assert_eq!(after.facts_digest, before.facts_digest);
    }
}

#[test]
fn critical_action_reason_and_unresolved_stage_facts_never_lost_under_additional_observations() {
    let contract = task_contract();
    let requirement = DeliveryRequirement {
        schema_version: 1,
        task_id: contract.task_id.clone(),
        contract_revision: contract.revision,
        contract_digest: content_digest(&contract).expect("contract digest"),
        generation: 1,
        targets: vec![DeliveryTarget {
            id: "repo-main".into(),
            target_kind: DeliveryTargetKind::Repository,
            requirement: TargetRequirement::Required {
                stage: DeliveryStage::Merged,
            },
        }],
    };
    let join = join_for(&contract, &requirement, "repo-main");

    // Supply an ambiguous merge and a failed review FIRST, and then append passing review + reconciled merge
    // observations for the same target/stage. Additional observations MUST NOT mask or overwrite earlier
    // unresolved `UncertainOutcome` or `Failed` facts!
    let mut observations = vec![
        DeliveryObservation {
            observation_id: "obs-review-failed-first".into(),
            join: join.clone(),
            target_kind: DeliveryTargetKind::Repository,
            trust: claimed_trust("obs-review-failed-first", 'a'),
            observed_tick: DecimalU64::new(100),
            valid_through_tick: DecimalU64::new(200),
            detail: StageObservationDetail::Review {
                head_commit_sha: oid('1'),
                required_reviewers: vec!["neverhumanbot".into()],
                approvals: vec![],
            },
        },
        DeliveryObservation {
            observation_id: "obs-ambiguous-merge-first".into(),
            join,
            target_kind: DeliveryTargetKind::Repository,
            trust: claimed_trust("obs-ambiguous-merge-first", 'b'),
            observed_tick: DecimalU64::new(101),
            valid_through_tick: DecimalU64::new(200),
            detail: StageObservationDetail::Merge {
                target_ref: "refs/heads/main".into(),
                expected_head_sha: oid('1'),
                expected_base_sha: oid('3'),
                observed_head_sha: None,
                intent_key: "intent:ambiguous-1".into(),
                intent_stage: IntentStage::Ambiguous,
                effect_receipt: None,
                rescue_binding: Some(RescueCandidateBinding {
                    task_id: contract.task_id.clone(),
                    attempt_id: AttemptId::new("attempt-1").expect("valid attempt"),
                    candidate_ref: "candidate:wo5-1".into(),
                    source_commit_sha: oid('1'),
                    source_tree_sha: oid('2'),
                    rescue_artifact_digest: digest('8'),
                    rescue_ref: "refs/heads/rescue/repo-main".into(),
                    force_push_permitted: false,
                }),
            },
        },
    ];
    // Append a full set of "passing" observations after the unresolved facts:
    observations.extend(full_repo_observations(
        &contract,
        &requirement,
        "repo-main",
        "later-pass",
        IntentStage::ReconciledSuccess,
    ));

    let ctx = context_for(
        contract,
        requirement,
        ResponsibleActor::RoleQueue {
            role: ResponsibleRole::Commodore,
        },
        110,
        &observations,
    );
    let batch = DeliveryObservationBatch {
        schema_version: DELIVERY_PROJECTION_VERSION,
        observations,
        operational_blockers: vec![
            OperationalBlockerObservation {
                blocker_id: "blk-pressure".into(),
                target_id: Some("repo-main".into()),
                owner: ResponsibleActor::RoleQueue {
                    role: ResponsibleRole::Commodore,
                },
                provenance_ref: "node:pressure:1".into(),
                observed_tick: DecimalU64::new(100),
                valid_through_tick: DecimalU64::new(200),
                blocker: OperationalBlockerKind::ResourcePressureWait {
                    resource_dimension: "memory".into(),
                    detail: "waiting for >= 20 GiB free".into(),
                },
            },
            OperationalBlockerObservation {
                blocker_id: "blk-decision".into(),
                target_id: Some("repo-main".into()),
                owner: ResponsibleActor::RoleQueue {
                    role: ResponsibleRole::Admiral,
                },
                provenance_ref: "decision:q1".into(),
                observed_tick: DecimalU64::new(100),
                valid_through_tick: DecimalU64::new(200),
                blocker: OperationalBlockerKind::DecisionBlocker {
                    question_id: "q-scope-1".into(),
                    detail: "awaiting cross-project conflict resolution".into(),
                },
            },
        ],
        historical_receipts: vec![],
        prior_attention: vec![],
    };

    let projected = project_delivery(&ctx, &batch).expect("project multiple findings");
    assert!(!projected.overall_satisfied);
    assert_eq!(
        projected.targets[0].stages.review,
        StageEvaluation::Failed,
        "later passing review observation must not mask earlier Failed review observation"
    );
    assert_eq!(
        projected.targets[0].stages.merge,
        StageEvaluation::UncertainOutcome,
        "later ReconciledSuccess observation must not mask earlier UncertainOutcome merge observation"
    );
    assert!(matches!(
        projected.targets[0].external_outcome,
        ExternalOutcomeState::Uncertain { .. }
    ));
    assert_eq!(
        projected.headline_attention_reason,
        Some(AttentionReason::UncertainExternalOutcome)
    );
    assert_eq!(
        projected.targets[0].primary_attention_reason,
        Some(AttentionReason::UncertainExternalOutcome)
    );
    assert_eq!(
        projected.attention_items[0].reason,
        AttentionReason::UncertainExternalOutcome
    );
    assert_eq!(
        projected.attention_items[0].severity,
        AttentionSeverity::Critical
    );
    assert_eq!(
        projected.attention_items[0].next_action,
        NextAuthorizedAction::ReconcileExternalEffect
    );
    assert!(
        projected.attention_items[0]
            .supporting_reasons
            .contains(&AttentionReason::DecisionBlocker)
    );
    assert!(
        projected.attention_items[0]
            .supporting_reasons
            .contains(&AttentionReason::ResourcePressureWait)
    );
    assert!(
        projected.attention_items[0]
            .supporting_reasons
            .contains(&AttentionReason::MissingReviewApproval)
    );
}

#[test]
fn size_depth_count_duplicate_key_and_reference_boundaries_rejected() {
    // 1. Oversize raw input (> 1 MiB)
    let huge = vec![b' '; MAX_DELIVERY_JSON_BYTES + 1];
    assert_eq!(
        parse_observation_batch(&huge),
        Err(DeliveryError::InputTooLarge {
            limit: MAX_DELIVERY_JSON_BYTES
        })
    );

    // 2. Excessive JSON nesting depth (> 32)
    let deep = format!("{}{}{}", "[".repeat(40), "1", "]".repeat(40));
    assert_eq!(
        parse_observation_batch(deep.as_bytes()),
        Err(DeliveryError::Bound)
    );

    // 3. Duplicate JSON object key
    let dup_key = br#"{"schema_version":1,"schema_version":1}"#;
    assert_eq!(
        parse_observation_batch(dup_key),
        Err(DeliveryError::DuplicateKey)
    );

    // 4. Unknown field rejected by deny_unknown_fields
    let contract = task_contract();
    let requirement = DeliveryRequirement {
        schema_version: 1,
        task_id: contract.task_id.clone(),
        contract_revision: contract.revision,
        contract_digest: content_digest(&contract).expect("contract digest"),
        generation: 1,
        targets: vec![DeliveryTarget {
            id: "repo-main".into(),
            target_kind: DeliveryTargetKind::Repository,
            requirement: TargetRequirement::Required {
                stage: DeliveryStage::Candidate,
            },
        }],
    };
    let valid_ctx = context_for(
        contract,
        requirement,
        ResponsibleActor::Unassigned {},
        100,
        &[],
    );
    let valid_batch = batch_for(vec![]);
    let mut val = serde_json::to_value(&valid_batch).expect("to_value");
    val["unexpected_field"] = json!(true);
    assert_eq!(
        parse_observation_batch(&serde_json::to_vec(&val).expect("to_vec")),
        Err(DeliveryError::Json)
    );

    // 5. Non-canonical decimal u64 tick or numeric JSON tick rejected
    let sample_obs = DeliveryObservation {
        observation_id: "obs-1".into(),
        join: join_for(&valid_ctx.contract, &valid_ctx.requirement, "repo-main"),
        target_kind: DeliveryTargetKind::Repository,
        trust: claimed_trust("obs-1", 'a'),
        observed_tick: DecimalU64::new(10),
        valid_through_tick: DecimalU64::new(200),
        detail: StageObservationDetail::Implementation {
            source_commit_sha: oid('1'),
            source_tree_sha: oid('2'),
            source_artifact_digest: digest('a'),
            tracking_trailers: vec![],
        },
    };
    let mut bad_tick = serde_json::to_value(batch_for(vec![sample_obs.clone()])).expect("to_value");
    bad_tick["observations"][0]["observed_tick"] = json!(100);
    assert!(parse_observation_batch(&serde_json::to_vec(&bad_tick).expect("to_vec")).is_err());
    bad_tick["observations"][0]["observed_tick"] = json!("0100");
    assert!(parse_observation_batch(&serde_json::to_vec(&bad_tick).expect("to_vec")).is_err());

    // 6. Hostile / template / path-traversal candidate_ref rejected
    for hostile in [
        "../escape",
        "http://example.com/x",
        "file:///etc/passwd",
        "candidate:${inject}",
        "candidate:{{inject}}",
    ] {
        let mut bad_ctx = valid_ctx.clone();
        bad_ctx.candidate_ref = hostile.into();
        assert!(
            project_delivery(&bad_ctx, &valid_batch).is_err(),
            "must reject {hostile}"
        );
    }

    // 7. Count bound (> MAX_STAGE_OBSERVATIONS) rejected
    let mut too_many = valid_batch;
    too_many.observations = (0..=MAX_STAGE_OBSERVATIONS)
        .map(|i| {
            let mut o = sample_obs.clone();
            o.observation_id = format!("obs-{i}");
            o
        })
        .collect();
    assert_eq!(
        project_delivery(&valid_ctx, &too_many),
        Err(DeliveryError::Bound)
    );
}

#[test]
fn existing_task_contract_v1_bytes_and_digest_unchanged() {
    let contract = task_contract();
    let encoded = serde_json::to_string(&contract).expect("serialize TaskContract v1");
    let expected_json = r#"{"schema_version":1,"project_id":"admiralty","root_id":"root-wo5","task_id":"task-wo5","revision":2,"title":"Pure delivery and attention projection","objective":"Join exact delivery evidence without conflating stages","non_goals":["No live publish or daemon execution"],"owned_paths":["crates/admiralty-core/src/delivery.rs"],"forbidden_paths":["credentials"],"read_first":["docs/plans/admiralty-v2/WORK_ORDER_REVIEW.md"],"dependencies":[],"criteria":[{"id":"delivery-check","text":"All delivery projection invariants hold","check_ids":["cargo-test"]}],"checks":[{"id":"cargo-test","program":"cargo","args":["test","-p","admiralty-core","--test","delivery_projection"],"cwd":".","timeout_secs":120}],"authority_ref":"grant:wo5-delivery","max_attempts":2,"simulated":false}"#;
    assert_eq!(encoded, expected_json);
    let roundtrip: TaskContract =
        serde_json::from_str(expected_json).expect("deserialize TaskContract v1");
    assert_eq!(roundtrip, contract);
    assert_eq!(
        content_digest(&contract).expect("digest"),
        content_digest(&roundtrip).expect("roundtrip digest")
    );
}
