use admiralty_core::conflicts::{CandidateChange, ConflictError, FileChange, analyze_conflicts};
use admiralty_core::trials::{
    AnalysisReceipt, AnalysisRecommendation, ClusterAssignment, ClusterId, ClusterMember,
    DecimalU64, DecisionPhase, FeatureId, FeatureManifest, Missingness, Outcome, OutcomeLabel,
    TrialDecision, TrialEvidence, TrialId, TrialSpec, VariantId, assign_trial,
    propose_trial_decision, trial_evidence_digest, validate_trial,
};
use admiralty_core::{PrincipalId, ProjectId, RootId, TaskId};
use std::error::Error;

const REPLAY_STEPS: usize = 96;

fn digest(byte: char) -> String {
    format!("sha256:{}", byte.to_string().repeat(64))
}

fn build_trial() -> Result<TrialSpec, Box<dyn Error>> {
    let control = VariantId::new("control")?;
    let candidate = VariantId::new("candidate")?;
    let cluster_a = ClusterId::new("release-manager-a")?;
    let cluster_b = ClusterId::new("release-manager-b")?;
    let fixed_cohort = [
        ("release-root-a1", cluster_a.clone()),
        ("release-root-a2", cluster_a.clone()),
        ("release-root-b1", cluster_b.clone()),
        ("release-root-b2", cluster_b.clone()),
    ]
    .into_iter()
    .map(|(root, cluster_id)| {
        Ok(ClusterMember {
            root_id: RootId::new(root)?,
            cluster_id,
        })
    })
    .collect::<Result<Vec<_>, Box<dyn Error>>>()?;

    Ok(TrialSpec {
        trial_id: TrialId::new("round17-release-smoke")?,
        project_id: ProjectId::new("round17-release")?,
        scope_root_id: RootId::new("release-scope")?,
        scope_task_id: TaskId::new("release-task")?,
        contract_revision: 1,
        contract_digest: digest('a'),
        fixed_cohort,
        immutable_bundle_digests: vec![digest('b'), digest('c')],
        protected_oracle_digest: digest('d'),
        pre_registered_seed: "round17-deterministic-canary".into(),
        decision_tick: DecimalU64::new(100),
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
    })
}

fn build_evidence(trial: TrialSpec) -> Result<TrialEvidence, Box<dyn Error>> {
    let assignments = assign_trial(&trial, &trial.fixed_cohort, 1, 2)?;
    let outcomes = assignments
        .iter()
        .map(|assignment| Outcome {
            root_id: assignment.root_id.clone(),
            label: OutcomeLabel::Mature,
            observed_tick: DecimalU64::new(105),
            heldout_contaminated: false,
            rework: false,
            analysis_value_micros: Some(DecimalU64::new(1500)),
        })
        .collect();
    let mut evidence = TrialEvidence {
        bundle_digests_at_analysis: trial.immutable_bundle_digests.clone(),
        protected_oracle_digest_at_analysis: trial.protected_oracle_digest.clone(),
        trial,
        features: vec![FeatureManifest {
            feature_id: FeatureId::new("round17-prior-cost-band")?,
            digest: digest('e'),
            available_tick: DecimalU64::new(99),
            decision_phase: DecisionPhase::InitialAssignment,
            missingness: Missingness::Known,
            extraction_cost_micros: Some(DecimalU64::new(25)),
        }],
        assignments,
        outcomes,
        look_count: DecimalU64::new(1),
        analysis_tick: DecimalU64::new(110),
        independent_analysis: None,
    };
    evidence.independent_analysis = Some(AnalysisReceipt {
        receipt_digest: digest('f'),
        analyst: PrincipalId::new("round17-independent-reviewer")?,
        analyzed_evidence_digest: trial_evidence_digest(&evidence)?,
        recommendation: AnalysisRecommendation::RecommendIndependentReview,
    });
    Ok(evidence)
}

fn candidate(id: &str, path: &str) -> Result<CandidateChange, Box<dyn Error>> {
    Ok(CandidateChange {
        project_id: ProjectId::new("round17-release")?,
        root_id: RootId::new("release-scope")?,
        task_id: TaskId::new(id)?,
        candidate_id: id.into(),
        base_sha: digest('1'),
        head_sha: digest('2'),
        tree_sha: digest('3'),
        ancestry_digest: Some(digest('4')),
        files: vec![FileChange {
            path: path.into(),
            hunks: None,
        }],
        constraints: None,
    })
}

#[test]
fn codex_round17_p28_p32() -> Result<(), Box<dyn Error>> {
    let trial = build_trial()?;
    let evidence = build_evidence(trial.clone())?;
    validate_trial(&evidence)?;

    let expected_assignments = assign_trial(&trial, &trial.fixed_cohort, 1, 2)?;
    let mut reversed_cohort = trial.fixed_cohort.clone();
    reversed_cohort.reverse();
    let mut expected_decision = None;
    let mut expected_conflicts = None;

    // Re-run the complete assignment, canary, and P32 review sequence many
    // times to detect replay drift across the release smoke path.
    for step in 0..REPLAY_STEPS {
        let assignments = assign_trial(&trial, &trial.fixed_cohort, 1, 2)?;
        assert_eq!(
            assignments, expected_assignments,
            "assignment replay {step}"
        );
        assert_eq!(
            assignments,
            assign_trial(&trial, &reversed_cohort, 1, 2)?,
            "reordered cohort replay {step}"
        );

        let decision = propose_trial_decision(&evidence, vec![digest('b')])?;
        assert!(matches!(
            &decision,
            TrialDecision::PromotableProposal {
                promotion_authorized: false,
                rollback,
                ..
            } if rollback.applies_to_new_work_only
        ));
        if let Some(expected) = &expected_decision {
            assert_eq!(&decision, expected, "canary decision replay {step}");
        } else {
            expected_decision = Some(decision.clone());
        }

        let candidates = [
            candidate("release-a", "src/worker.rs")?,
            candidate("release-b", "src/worker.rs")?,
            candidate("release-c", "src/policy.rs")?,
        ];
        let generation = analyze_conflicts(&candidates, "round17-release-policy")?;
        let mut reverse_candidates = candidates.to_vec();
        reverse_candidates.reverse();
        assert_eq!(
            generation,
            analyze_conflicts(&reverse_candidates, "round17-release-policy")?,
            "conflict replay {step}"
        );
        assert_eq!(generation.generation.candidate_ids.len(), 3);
        assert!(generation.generation.combined_verification_required);
        if let Some(expected) = &expected_conflicts {
            assert_eq!(&generation, expected, "P32 generation replay {step}");
        } else {
            expected_conflicts = Some(generation);
        }
    }

    // A stale analysis receipt and incomplete canary evidence remain
    // fail-closed even after all replay steps have completed.
    let mut stale_receipt = evidence.clone();
    stale_receipt.outcomes[0].analysis_value_micros = Some(DecimalU64::new(1501));
    assert!(matches!(
        propose_trial_decision(&stale_receipt, vec![digest('b')])?,
        TrialDecision::InsufficientEvidence {
            promotion_authorized: false,
            reasons,
        } if reasons.iter().any(|reason| reason == "analysis receipt does not bind evidence")
    ));
    let mut missing_receipt = evidence;
    missing_receipt.independent_analysis = None;
    assert!(matches!(
        propose_trial_decision(&missing_receipt, vec![digest('b')])?,
        TrialDecision::InsufficientEvidence {
            promotion_authorized: false,
            reasons,
        } if reasons.iter().any(|reason| reason == "independent analysis receipt required")
    ));

    assert!(matches!(
        analyze_conflicts(
            &[candidate("unsafe-release", "../release.toml")?],
            "round17-release-policy"
        ),
        Err(ConflictError::InvalidPath(_))
    ));
    assert!(matches!(
        analyze_conflicts(
            &[
                candidate("same-release-id", "src/worker.rs")?,
                candidate("same-release-id", "src/changed.rs")?,
            ],
            "round17-release-policy"
        ),
        Err(ConflictError::CandidateCollision(_))
    ));

    Ok(())
}
