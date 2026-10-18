use admiralty_core::trials::{
    AnalysisReceipt, AnalysisRecommendation, ClusterAssignment, ClusterId, ClusterMember,
    DecimalU64, DecisionPhase, FeatureId, FeatureManifest, Missingness, Outcome, OutcomeLabel,
    TrialDecision, TrialEvidence, TrialId, TrialSpec, VariantId, assign_trial,
    propose_trial_decision, trial_evidence_digest, validate_trial,
};
use admiralty_core::{PrincipalId, ProjectId, RootId, TaskId};

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
