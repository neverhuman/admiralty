use admiralty_core::evaluation::*;
use admiralty_core::trials::{
    AnalysisReceipt, AnalysisRecommendation, ClusterAssignment, ClusterId, ClusterMember,
    DecimalU64, DecisionPhase, FeatureId, FeatureManifest, Missingness, Outcome, OutcomeLabel,
    TrialDecision, TrialEvidence, TrialId, TrialSpec, VariantId, assign_trial,
    propose_trial_decision, trial_evidence_digest, validate_trial,
};
use admiralty_core::{PrincipalId, ProjectId, RootId, TaskId};
use admiralty_server::services::improvement::experiments::{
    ExperimentService, ReviewAuthority, ReviewRequest,
};

fn digest(fill: char) -> String {
    format!("sha256:{}", fill.to_string().repeat(64))
}

fn cluster(id: &str) -> ClusterId {
    ClusterId::new(id).expect("valid cluster fixture ID")
}

fn variant(id: &str) -> VariantId {
    VariantId::new(id).expect("valid variant fixture ID")
}

fn trial() -> TrialSpec {
    let control = variant("control");
    let candidate = variant("candidate");
    let cluster_a = cluster("manager-a");
    let cluster_b = cluster("manager-b");
    let fixed_cohort = [
        ("root-a1", cluster_a.clone()),
        ("root-a2", cluster_a.clone()),
        ("root-b1", cluster_b.clone()),
        ("root-b2", cluster_b.clone()),
    ]
    .into_iter()
    .map(|(root, cluster_id)| ClusterMember {
        root_id: RootId::new(root).expect("valid root fixture ID"),
        cluster_id,
    })
    .collect();

    TrialSpec {
        trial_id: TrialId::new("experiment-42").expect("valid trial fixture ID"),
        project_id: ProjectId::new("harbor").expect("valid project fixture ID"),
        scope_root_id: RootId::new("scope-root").expect("valid scope root fixture ID"),
        scope_task_id: TaskId::new("scope-task").expect("valid scope task fixture ID"),
        contract_revision: 3,
        contract_digest: digest('c'),
        fixed_cohort,
        immutable_bundle_digests: vec![digest('a'), digest('b')],
        protected_oracle_digest: digest('0'),
        pre_registered_seed: "seed-2026-10".into(),
        decision_tick: DecimalU64::new(50),
        cluster_assignments: vec![
            ClusterAssignment {
                cluster_id: cluster_a,
                variants: vec![control.clone(), candidate.clone()],
            },
            ClusterAssignment {
                cluster_id: cluster_b,
                variants: vec![control, candidate],
            },
        ],
        required_maturity: DecimalU64::new(4),
        planned_looks: DecimalU64::new(1),
    }
}

fn complete_evidence() -> TrialEvidence {
    let trial = trial();
    let assignments = assign_trial(&trial, &trial.fixed_cohort, 1, 2)
        .expect("assign the preregistered fixed cohort");
    let outcomes = assignments
        .iter()
        .map(|assignment| Outcome {
            root_id: assignment.root_id.clone(),
            label: OutcomeLabel::Mature,
            observed_tick: DecimalU64::new(55),
            heldout_contaminated: false,
            rework: false,
            analysis_value_micros: Some(DecimalU64::new(1200)),
        })
        .collect();
    TrialEvidence {
        bundle_digests_at_analysis: trial.immutable_bundle_digests.clone(),
        protected_oracle_digest_at_analysis: trial.protected_oracle_digest.clone(),
        trial,
        features: vec![FeatureManifest {
            feature_id: FeatureId::new("prior-cost-band").expect("valid feature fixture ID"),
            digest: digest('f'),
            available_tick: DecimalU64::new(49),
            decision_phase: DecisionPhase::InitialAssignment,
            missingness: Missingness::Known,
            extraction_cost_micros: Some(DecimalU64::new(25)),
        }],
        assignments,
        outcomes,
        look_count: DecimalU64::new(1),
        analysis_tick: DecimalU64::new(60),
        independent_analysis: None,
    }
}

fn attach_analysis(evidence: &mut TrialEvidence, recommendation: AnalysisRecommendation) {
    let evidence_digest =
        trial_evidence_digest(evidence).expect("digest exact preregistered analysis evidence");
    evidence.independent_analysis = Some(AnalysisReceipt {
        receipt_digest: digest('7'),
        analyst: PrincipalId::new("independent-reviewer").expect("valid analyst fixture ID"),
        analyzed_evidence_digest: evidence_digest,
        recommendation,
    });
}

#[test]
fn seeded_assignment_is_replayable_and_shared_manager_clusters_stay_together() {
    let spec = trial();
    let expected = assign_trial(&spec, &spec.fixed_cohort, 1, 2).expect("assign registered roots");
    let mut reversed_cohort = spec.fixed_cohort.clone();
    reversed_cohort.reverse();
    assert_eq!(
        expected,
        assign_trial(&spec, &reversed_cohort, 1, 2).expect("replay reordered cohort")
    );

    for shared_manager in ["manager-a", "manager-b"] {
        let related_roots: Vec<_> = expected
            .iter()
            .filter(|assignment| assignment.cluster_id.as_str() == shared_manager)
            .collect();
        assert_eq!(related_roots.len(), 2);
        assert_eq!(
            related_roots[0].variant, related_roots[1].variant,
            "roots sharing a manager cluster must not be split across arms"
        );
    }
    assert_eq!(
        expected
            .iter()
            .map(|assignment| &assignment.variant)
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        2,
        "balanced clusters cover both preregistered arms"
    );
}

#[test]
fn future_features_bad_propensity_and_oracle_or_bundle_drift_fail_closed() {
    let mut evidence = complete_evidence();
    evidence.features[0].available_tick = DecimalU64::new(51);
    assert!(
        validate_trial(&evidence).is_err(),
        "future feature must be rejected"
    );

    let mut evidence = complete_evidence();
    evidence.features[0].decision_phase = DecisionPhase::Continuation;
    assert!(
        validate_trial(&evidence).is_err(),
        "continuation feature must be rejected at assignment"
    );

    let spec = trial();
    assert!(
        assign_trial(&spec, &spec.fixed_cohort, 0, 2).is_err(),
        "zero propensity is invalid"
    );
    assert!(
        assign_trial(&spec, &spec.fixed_cohort, 1, 3).is_err(),
        "propensity must match the preregistered uniform arm draw"
    );
    let mut evidence = complete_evidence();
    evidence.assignments[0].propensity_denominator = DecimalU64::new(1);
    assert!(
        validate_trial(&evidence).is_err(),
        "recorded propensity must be exact"
    );

    let mut evidence = complete_evidence();
    evidence.protected_oracle_digest_at_analysis = digest('9');
    assert!(
        validate_trial(&evidence).is_err(),
        "protected oracle drift invalidates the trial"
    );
    let mut evidence = complete_evidence();
    evidence.bundle_digests_at_analysis[0] = digest('9');
    assert!(
        validate_trial(&evidence).is_err(),
        "immutable bundle drift invalidates the trial"
    );
}

#[test]
fn repeated_peeks_heldout_contamination_and_immature_or_unknown_costs_never_promote() {
    let mut evidence = complete_evidence();
    evidence.look_count = DecimalU64::new(2);
    attach_analysis(
        &mut evidence,
        AnalysisRecommendation::RecommendIndependentReview,
    );
    assert!(matches!(
        propose_trial_decision(&evidence, vec![digest('a')]).expect("proposal for repeated look"),
        TrialDecision::InsufficientEvidence {
            promotion_authorized: false,
            ..
        }
    ));

    let mut evidence = complete_evidence();
    evidence.outcomes[0].heldout_contaminated = true;
    attach_analysis(
        &mut evidence,
        AnalysisRecommendation::RecommendIndependentReview,
    );
    assert!(matches!(
        propose_trial_decision(&evidence, vec![digest('a')])
            .expect("proposal for contaminated evidence"),
        TrialDecision::InsufficientEvidence {
            promotion_authorized: false,
            ..
        }
    ));

    let mut evidence = complete_evidence();
    evidence.outcomes[0].label = OutcomeLabel::Immature;
    attach_analysis(
        &mut evidence,
        AnalysisRecommendation::RecommendIndependentReview,
    );
    assert!(matches!(
        propose_trial_decision(&evidence, vec![digest('a')]).expect("proposal for immature label"),
        TrialDecision::InsufficientEvidence {
            promotion_authorized: false,
            ..
        }
    ));

    let mut evidence = complete_evidence();
    evidence.features[0].extraction_cost_micros = None;
    attach_analysis(
        &mut evidence,
        AnalysisRecommendation::RecommendIndependentReview,
    );
    assert!(matches!(
        propose_trial_decision(&evidence, vec![digest('a')]).expect("proposal for unknown cost"),
        TrialDecision::InsufficientEvidence {
            promotion_authorized: false,
            ..
        }
    ));
}

#[test]
fn fixed_cohort_cannot_be_dropped_or_reassigned_and_missing_analysis_is_insufficient() {
    let mut evidence = complete_evidence();
    evidence.assignments.pop();
    evidence.outcomes.pop();
    assert!(
        validate_trial(&evidence).is_err(),
        "fixed cohort members cannot be filtered"
    );

    let mut evidence = complete_evidence();
    evidence.assignments[0].variant = if evidence.assignments[0].variant.as_str() == "control" {
        variant("candidate")
    } else {
        variant("control")
    };
    assert!(
        validate_trial(&evidence).is_err(),
        "seeded cluster assignments cannot be changed"
    );

    let evidence = complete_evidence();
    assert!(matches!(
        propose_trial_decision(&evidence, vec![digest('a')]).expect("missing analysis proposal"),
        TrialDecision::InsufficientEvidence {
            promotion_authorized: false,
            ..
        }
    ));
}

#[test]
fn independent_recommendations_only_propose_review_or_rejection() {
    let mut evidence = complete_evidence();
    attach_analysis(
        &mut evidence,
        AnalysisRecommendation::RecommendIndependentReview,
    );
    assert!(matches!(
        propose_trial_decision(&evidence, vec![digest('a')]).expect("bounded candidate proposal"),
        TrialDecision::PromotableProposal {
            promotion_authorized: false,
            rollback,
            ..
        } if rollback.applies_to_new_work_only && rollback.champion_bundle_digests == vec![digest('a')]
    ));

    let mut evidence = complete_evidence();
    attach_analysis(&mut evidence, AnalysisRecommendation::Reject);
    assert!(matches!(
        propose_trial_decision(&evidence, vec![]).expect("independent rejection proposal"),
        TrialDecision::RejectProposal {
            promotion_authorized: false,
            ..
        }
    ));
}

#[test]
fn tampered_independent_receipt_cannot_authorize_even_a_review_proposal() {
    let mut evidence = complete_evidence();
    attach_analysis(
        &mut evidence,
        AnalysisRecommendation::RecommendIndependentReview,
    );
    evidence
        .independent_analysis
        .as_mut()
        .expect("analysis receipt attached")
        .analyzed_evidence_digest = digest('8');

    assert!(matches!(
        propose_trial_decision(&evidence, vec![digest('a')])
            .expect("tampered receipt fails closed"),
        TrialDecision::InsufficientEvidence {
            promotion_authorized: false,
            reasons,
        } if reasons.iter().any(|reason| reason == "analysis receipt does not bind evidence")
    ));
}

#[test]
fn rollback_cannot_reference_an_unregistered_trial_bundle() {
    let mut evidence = complete_evidence();
    attach_analysis(
        &mut evidence,
        AnalysisRecommendation::RecommendIndependentReview,
    );

    assert!(
        propose_trial_decision(&evidence, vec![digest('9')]).is_err(),
        "rollback must identify one of the trial's immutable bundles"
    );
}

#[test]
fn planned_look_budget_accepts_limit_and_rejects_limit_plus_one() {
    let mut evidence = complete_evidence();
    evidence.trial.planned_looks = DecimalU64::new(16);
    evidence.assignments = assign_trial(&evidence.trial, &evidence.trial.fixed_cohort, 1, 2)
        .expect("maximum look budget remains a valid trial specification");
    assert!(
        validate_trial(&evidence).is_ok(),
        "the maximum preregistered look budget is valid"
    );

    evidence.trial.planned_looks = DecimalU64::new(17);
    assert_eq!(
        validate_trial(&evidence)
            .expect_err("a look budget above the contract limit must be rejected")
            .to_string(),
        "invalid trial input: invalid fixed cohort, revision or preregistered budget"
    );
}

#[test]
fn decision_proposal_is_idempotent_for_identical_evidence() {
    let mut evidence = complete_evidence();
    attach_analysis(
        &mut evidence,
        AnalysisRecommendation::RecommendIndependentReview,
    );
    let champion = vec![digest('a')];

    let first =
        propose_trial_decision(&evidence, champion.clone()).expect("first deterministic proposal");
    let replay =
        propose_trial_decision(&evidence, champion).expect("replayed deterministic proposal");

    assert_eq!(
        first, replay,
        "replaying evidence must not alter its proposal"
    );
    assert!(matches!(
        first,
        TrialDecision::PromotableProposal {
            promotion_authorized: false,
            ..
        }
    ));
}

#[test]
fn truncated_feature_digest_is_rejected_at_the_sha256_boundary() {
    let mut evidence = complete_evidence();
    evidence.features[0].digest = format!("sha256:{}", "a".repeat(63));

    assert_eq!(
        validate_trial(&evidence)
            .expect_err("truncated feature digests must be rejected")
            .to_string(),
        "invalid trial input: full lowercase SHA-256 digest required"
    );
}

#[test]
fn outcome_clock_must_advance_past_trial_assignment() {
    let mut evidence = complete_evidence();
    evidence.outcomes[0].observed_tick = DecimalU64::new(51);
    assert!(
        validate_trial(&evidence).is_ok(),
        "an outcome observed after assignment is valid"
    );

    evidence.outcomes[0].observed_tick = evidence.trial.decision_tick.clone();
    assert_eq!(
        validate_trial(&evidence)
            .expect_err("outcomes cannot be observed at the assignment transition")
            .to_string(),
        "invalid trial input: outcome outside recorded assignment/analysis clock interval"
    );
}

fn bundle(id: &str) -> Bundle {
    Bundle {
        id: TrialId::new(id).expect("bundle ID"),
        role_prompts: [("worker".into(), format!("bounded prompt {id}"))].into(),
        policy_digest: digest('1'),
        detector_digest: digest('2'),
        evaluator_digest: digest('3'),
        authority_digest: digest('4'),
        learned_routing: false,
    }
}

fn registered(
    mode: TrialMode,
) -> (
    ExperimentService,
    TrialRegistration,
    TrialEvidence,
    Vec<ExecutionRecord>,
) {
    let mut service = ExperimentService::default();
    let champion = service
        .register_bundle(bundle("champion-v1"))
        .expect("register champion");
    let candidate = service
        .register_bundle(bundle("candidate-v1"))
        .expect("register candidate");
    let mut evidence = complete_evidence();
    evidence.trial.immutable_bundle_digests = vec![champion.clone(), candidate.clone()];
    evidence.bundle_digests_at_analysis = evidence.trial.immutable_bundle_digests.clone();
    evidence.assignments = assign_trial(&evidence.trial, &evidence.trial.fixed_cohort, 1, 2)
        .expect("exact assignments");
    let registration = TrialRegistration {
        spec: evidence.trial.clone(),
        owner: PrincipalId::new("author").expect("owner"),
        mode,
        champion: champion.clone(),
        candidate: candidate.clone(),
        managers: evidence
            .trial
            .fixed_cohort
            .iter()
            .map(|m| {
                (
                    m.root_id.clone(),
                    PrincipalId::new(m.cluster_id.as_str()).expect("manager"),
                )
            })
            .collect(),
        max_feature_age_ticks: DecimalU64::new(5),
        features: evidence.features.clone(),
    };
    let arms = &evidence.trial.cluster_assignments[0].variants;
    let executions = evidence
        .assignments
        .iter()
        .map(|a| ExecutionRecord {
            root_id: a.root_id.clone(),
            bundle_digest: if a.variant == arms[0] {
                champion.clone()
            } else {
                candidate.clone()
            },
            execution_digest: admiralty_core::content_digest(&a.root_id).expect("execution digest"),
            usage_digest: Some(digest('5')),
            execution_cost_micros: Some(DecimalU64::new(1100)),
            training_cost_micros: Some(DecimalU64::new(0)),
            analysis_cost_micros: Some(DecimalU64::new(100)),
        })
        .collect();
    service
        .register_trial(registration.clone())
        .expect("freeze trial before results");
    attach_analysis(
        &mut evidence,
        AnalysisRecommendation::RecommendIndependentReview,
    );
    (service, registration, evidence, executions)
}

struct ExactReview {
    digest: String,
    sensitive: bool,
}
impl ReviewAuthority for ExactReview {
    fn authorize(&self, request: &ReviewRequest) -> bool {
        request.reviewer.as_str() == "promotion-reviewer"
            && request.dossier_digest == self.digest
            && (self.sensitive || (!request.authority_changed && !request.evaluator_changed))
    }
}
fn reviewer() -> PrincipalId {
    PrincipalId::new("promotion-reviewer").expect("reviewer")
}

#[test]
fn bounded_trial_accepts_and_rolls_back_only_new_admissions_with_exact_independent_review() {
    let (mut service, registration, evidence, executions) = registered(TrialMode::HeldOut);
    let old_pin = service
        .pin_new_work(&registration.spec.project_id)
        .expect("old work pin");
    let dataset = service
        .record(evidence.clone(), executions.clone())
        .expect("record actual result");
    assert!(dataset.ineligibility.is_empty());
    assert_eq!(
        dataset
            .all_in_cost_micros
            .as_ref()
            .expect("all-in cost")
            .get(),
        4825
    );
    let authority = ExactReview {
        digest: dataset.dossier_digest().expect("dossier"),
        sensitive: false,
    };
    let wrong = ExactReview {
        digest: digest('9'),
        sensitive: true,
    };
    assert!(
        service
            .decide(
                &registration.spec.trial_id,
                0,
                reviewer(),
                Verdict::Accept,
                &wrong
            )
            .is_err()
    );
    assert!(
        service
            .decide(
                &registration.spec.trial_id,
                1,
                reviewer(),
                Verdict::Accept,
                &authority
            )
            .is_err()
    );
    assert!(
        service
            .decide(
                &registration.spec.trial_id,
                0,
                registration.owner.clone(),
                Verdict::Accept,
                &authority
            )
            .is_err()
    );
    service
        .decide(
            &registration.spec.trial_id,
            0,
            reviewer(),
            Verdict::Accept,
            &authority,
        )
        .expect("independently accept");
    let candidate_pin = service
        .pin_new_work(&registration.spec.project_id)
        .expect("candidate pin");
    assert_eq!(old_pin.bundle_digest(), registration.champion);
    assert_eq!(candidate_pin.bundle_digest(), registration.candidate);
    assert_eq!(candidate_pin.revision(), 1);
    service
        .record(evidence, executions)
        .expect("exact evidence retry preserves accepted decision");
    assert!(
        service
            .dataset(&registration.spec.trial_id)
            .expect("dataset")
            .decision
            .is_some()
    );
    assert!(
        service
            .rollback(&registration.spec.trial_id, 0, reviewer(), &authority)
            .is_err()
    );
    service
        .rollback(&registration.spec.trial_id, 1, reviewer(), &authority)
        .expect("reviewed rollback");
    assert_eq!(
        candidate_pin.bundle_digest(),
        registration.candidate,
        "running work retains candidate pin"
    );
    assert_eq!(
        service
            .pin_new_work(&registration.spec.project_id)
            .expect("new champion pin")
            .bundle_digest(),
        registration.champion
    );
    let dataset = service
        .dataset(&registration.spec.trial_id)
        .expect("retained decision");
    assert!(dataset.decision.as_ref().expect("acceptance").rolled_back);
    assert_eq!(
        dataset
            .rollback_review
            .as_ref()
            .expect("rollback review retained")
            .alias_revision,
        2
    );
    assert!(
        service
            .rollback(&registration.spec.trial_id, 2, reviewer(), &authority)
            .is_err()
    );
    let (digest, bytes) = service
        .export_dataset(&registration.spec.trial_id)
        .expect("export");
    let value: serde_json::Value = serde_json::from_slice(&bytes).expect("JSON dataset");
    assert_eq!(
        digest,
        admiralty_core::content_digest(
            &service
                .dataset(&registration.spec.trial_id)
                .expect("dataset")
        )
        .expect("same digest")
    );
    assert_eq!(
        value["executions"]
            .as_array()
            .expect("execution rows")
            .len(),
        4
    );
    assert_eq!(
        service
            .export_dataset(&registration.spec.trial_id)
            .expect("reproducible export"),
        (digest, bytes)
    );
}

#[test]
fn registry_preserves_immutable_bundles_trial_inputs_and_single_look_evidence() {
    let (mut service, registration, evidence, executions) = registered(TrialMode::Fresh);
    assert_eq!(
        service
            .register_bundle(bundle("champion-v1"))
            .expect("idempotent bundle"),
        registration.champion
    );
    let mut changed = bundle("champion-v1");
    changed
        .role_prompts
        .insert("worker".into(), "changed prompt".into());
    assert!(service.register_bundle(changed).is_err());
    let mut changed = registration.clone();
    changed.max_feature_age_ticks = DecimalU64::new(100);
    assert!(service.register_trial(changed).is_err());
    let mut changed = registration.clone();
    changed.spec.trial_id = TrialId::new("another-look").expect("trial");
    assert!(
        service.register_trial(changed).is_err(),
        "candidate/cohort cannot be reused to bypass stopping"
    );
    service
        .record(evidence.clone(), executions.clone())
        .expect("seal first look");
    let mut changed = evidence;
    changed.outcomes[0].label = OutcomeLabel::Failed;
    attach_analysis(
        &mut changed,
        AnalysisRecommendation::RecommendIndependentReview,
    );
    assert!(
        service.record(changed, executions).is_err(),
        "cannot replace a negative result after peeking"
    );
}

#[test]
fn sealed_dataset_retains_failures_cancellation_rework_and_independent_rejection() {
    let (mut service, registration, mut evidence, executions) = registered(TrialMode::Fresh);
    evidence.outcomes[0].label = OutcomeLabel::Failed;
    evidence.outcomes[1].label = OutcomeLabel::Cancelled;
    evidence.outcomes[1].rework = true;
    attach_analysis(&mut evidence, AnalysisRecommendation::Reject);
    let dataset = service
        .record(evidence, executions)
        .expect("record negative results");
    let authority = ExactReview {
        digest: dataset.dossier_digest().expect("dossier"),
        sensitive: false,
    };
    assert!(
        service
            .decide(
                &registration.spec.trial_id,
                0,
                reviewer(),
                Verdict::Accept,
                &authority
            )
            .is_err()
    );
    service
        .decide(
            &registration.spec.trial_id,
            0,
            reviewer(),
            Verdict::Reject,
            &authority,
        )
        .expect("independent rejection");
    assert_eq!(
        service
            .pin_new_work(&registration.spec.project_id)
            .expect("champion")
            .bundle_digest(),
        registration.champion
    );
    let (_, bytes) = service
        .export_dataset(&registration.spec.trial_id)
        .expect("negative export");
    let export: serde_json::Value = serde_json::from_slice(&bytes).expect("JSON");
    assert_eq!(
        export["evidence"]["outcomes"]
            .as_array()
            .expect("all roots")
            .len(),
        4
    );
    assert_eq!(export["evidence"]["outcomes"][0]["label"], "failed");
    assert_eq!(export["evidence"]["outcomes"][1]["label"], "cancelled");
    assert_eq!(export["evidence"]["outcomes"][1]["rework"], true);
    assert_eq!(export["decision"]["verdict"], "reject");
}

#[test]
fn replay_stale_missing_unknown_usage_self_analysis_and_contamination_cannot_promote() {
    for case in [
        "replay",
        "stale",
        "missing",
        "usage",
        "overflow",
        "analyst",
        "contaminated",
        "immature",
        "peeking",
    ] {
        let mode = if case == "replay" {
            TrialMode::Replay
        } else {
            TrialMode::Fresh
        };
        let (mut service, mut registration, mut evidence, mut executions) = registered(mode);
        match case {
            "stale" => evidence.features[0].available_tick = DecimalU64::new(1),
            "missing" => evidence.features[0].missingness = Missingness::Unknown,
            "usage" => executions[0].usage_digest = None,
            "overflow" => {
                executions[0].execution_cost_micros = Some(DecimalU64::new(u64::MAX));
            }
            "contaminated" => evidence.outcomes[0].heldout_contaminated = true,
            "immature" => evidence.outcomes[0].label = OutcomeLabel::Immature,
            "peeking" => evidence.look_count = DecimalU64::new(2),
            _ => {}
        }
        attach_analysis(
            &mut evidence,
            AnalysisRecommendation::RecommendIndependentReview,
        );
        if matches!(case, "stale" | "missing") {
            registration.features = evidence.features.clone();
            service = ExperimentService::default();
            service
                .register_bundle(bundle("champion-v1"))
                .expect("champion");
            service
                .register_bundle(bundle("candidate-v1"))
                .expect("candidate");
            service
                .register_trial(registration.clone())
                .expect("freeze stale/missing manifests honestly");
        }
        if case == "analyst" {
            evidence
                .independent_analysis
                .as_mut()
                .expect("receipt")
                .analyst = registration.owner.clone();
        }
        let dataset = service
            .record(evidence, executions)
            .expect("retain insufficient evidence");
        let authority = ExactReview {
            digest: dataset.dossier_digest().expect("dossier"),
            sensitive: true,
        };
        assert!(
            service
                .decide(
                    &registration.spec.trial_id,
                    0,
                    reviewer(),
                    Verdict::Accept,
                    &authority
                )
                .is_err(),
            "{case}"
        );
        service
            .decide(
                &registration.spec.trial_id,
                0,
                reviewer(),
                Verdict::InsufficientEvidence,
                &authority,
            )
            .expect("honest insufficient evidence");
        assert_eq!(
            service
                .pin_new_work(&registration.spec.project_id)
                .expect("champion")
                .bundle_digest(),
            registration.champion
        );
    }
}

#[test]
fn shared_manager_and_actual_execution_substitution_fail_closed() {
    let (_, mut registration, _, _) = registered(TrialMode::Fresh);
    let mut fresh = ExperimentService::default();
    fresh
        .register_bundle(bundle("champion-v1"))
        .expect("champion");
    fresh
        .register_bundle(bundle("candidate-v1"))
        .expect("candidate");
    for manager in registration.managers.values_mut() {
        *manager = PrincipalId::new("shared-captain").expect("manager");
    }
    assert!(
        fresh.register_trial(registration).is_err(),
        "one captain cannot be split across arms"
    );
    let (mut service, _, evidence, mut executions) = registered(TrialMode::Fresh);
    executions[0].bundle_digest = digest('9');
    assert!(
        service.record(evidence, executions).is_err(),
        "actual execution must match pinned assignment"
    );
}

#[test]
fn evaluator_and_authority_changes_require_configured_sensitive_review() {
    let (_, mut registration, mut evidence, mut executions) = registered(TrialMode::Fresh);
    let mut service = ExperimentService::default();
    service
        .register_bundle(bundle("champion-v1"))
        .expect("champion");
    let mut candidate = bundle("candidate-v1");
    candidate.evaluator_digest = digest('8');
    candidate.authority_digest = digest('9');
    let new_digest = service
        .register_bundle(candidate)
        .expect("sensitive candidate");
    for execution in &mut executions {
        if execution.bundle_digest == registration.candidate {
            execution.bundle_digest = new_digest.clone();
        }
    }
    registration.candidate = new_digest.clone();
    registration.spec.immutable_bundle_digests[1] = new_digest;
    evidence.trial = registration.spec.clone();
    evidence.bundle_digests_at_analysis = evidence.trial.immutable_bundle_digests.clone();
    evidence.assignments = service
        .register_trial(registration.clone())
        .expect("register sensitive trial");
    attach_analysis(
        &mut evidence,
        AnalysisRecommendation::RecommendIndependentReview,
    );
    let dataset = service
        .record(evidence, executions)
        .expect("sensitive dossier");
    let mut authority = ExactReview {
        digest: dataset.dossier_digest().expect("digest"),
        sensitive: false,
    };
    assert!(
        service
            .decide(
                &registration.spec.trial_id,
                0,
                reviewer(),
                Verdict::Accept,
                &authority
            )
            .is_err()
    );
    assert_eq!(
        service
            .pin_new_work(&registration.spec.project_id)
            .expect("unchanged alias")
            .revision(),
        0
    );
    authority.sensitive = true;
    service
        .decide(
            &registration.spec.trial_id,
            0,
            reviewer(),
            Verdict::Accept,
            &authority,
        )
        .expect("configured sensitive review");
}

#[test]
fn feature_manifests_are_frozen_before_assignment_and_multiplicity_is_conservative() {
    let (mut service, _, mut evidence, executions) = registered(TrialMode::Fresh);
    evidence.features[0].extraction_cost_micros = Some(DecimalU64::new(0));
    attach_analysis(
        &mut evidence,
        AnalysisRecommendation::RecommendIndependentReview,
    );
    assert!(
        service.record(evidence, executions).is_err(),
        "cannot replace extraction costs after assignment"
    );

    let (_, mut registration, mut evidence, executions) = registered(TrialMode::Fresh);
    let mut service = ExperimentService::default();
    service
        .register_bundle(bundle("champion-v1"))
        .expect("champion");
    service
        .register_bundle(bundle("candidate-v1"))
        .expect("candidate");
    registration.spec.planned_looks = DecimalU64::new(2);
    evidence.trial = registration.spec.clone();
    evidence.assignments = service
        .register_trial(registration.clone())
        .expect("register multi-look design");
    attach_analysis(
        &mut evidence,
        AnalysisRecommendation::RecommendIndependentReview,
    );
    let dataset = service
        .record(evidence, executions)
        .expect("retain unadjusted multi-look analysis");
    assert!(
        dataset
            .ineligibility
            .iter()
            .any(|r| r.contains("multiplicity"))
    );
    let authority = ExactReview {
        digest: dataset.dossier_digest().expect("digest"),
        sensitive: true,
    };
    assert!(
        service
            .decide(
                &registration.spec.trial_id,
                0,
                reviewer(),
                Verdict::Accept,
                &authority
            )
            .is_err()
    );
}
