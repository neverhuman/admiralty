use admiralty_core::conflicts::{CandidateChange, ConflictError, FileChange, analyze_conflicts};
use admiralty_core::trials::{
    AnalysisReceipt, AnalysisRecommendation, ClusterAssignment, ClusterId, ClusterMember,
    DecimalU64, DecisionPhase, FeatureId, FeatureManifest, Missingness, Outcome, OutcomeLabel,
    TrialDecision, TrialEvidence, TrialId, TrialSpec, VariantId, assign_trial,
    propose_trial_decision, trial_evidence_digest,
};
use admiralty_core::{PrincipalId, ProjectId, RootId, TaskId};
use std::error::Error;

fn digest(byte: char) -> String {
    format!("sha256:{}", byte.to_string().repeat(64))
}

#[test]
fn codex_round14_p28_p32() -> Result<(), Box<dyn Error>> {
    let variants = [VariantId::new("control")?, VariantId::new("candidate")?];
    let clusters = [ClusterId::new("canary-a")?, ClusterId::new("canary-b")?];
    let cohort = [
        ("canary-root-a1", 0),
        ("canary-root-a2", 0),
        ("canary-root-b1", 1),
        ("canary-root-b2", 1),
    ]
    .into_iter()
    .map(|(root, cluster)| {
        Ok(ClusterMember {
            root_id: RootId::new(root)?,
            cluster_id: clusters[cluster].clone(),
        })
    })
    .collect::<Result<Vec<_>, Box<dyn Error>>>()?;
    let trial = TrialSpec {
        trial_id: TrialId::new("round14-p28-p32")?,
        project_id: ProjectId::new("round14-qualification")?,
        scope_root_id: RootId::new("round14-scope")?,
        scope_task_id: TaskId::new("round14-task")?,
        contract_revision: 1,
        contract_digest: digest('a'),
        fixed_cohort: cohort.clone(),
        immutable_bundle_digests: vec![digest('b'), digest('c')],
        protected_oracle_digest: digest('d'),
        pre_registered_seed: "round14-replay-seed".into(),
        decision_tick: DecimalU64::new(50),
        cluster_assignments: clusters
            .iter()
            .map(|cluster_id| ClusterAssignment {
                cluster_id: cluster_id.clone(),
                variants: variants.to_vec(),
            })
            .collect(),
        required_maturity: DecimalU64::new(4),
        planned_looks: DecimalU64::new(1),
    };

    // P28 replay: permuting delivery cannot change the preregistered assignment.
    let assignments = assign_trial(&trial, &cohort, 1, 2)?;
    let mut replay_cohort = cohort.clone();
    replay_cohort.reverse();
    assert_eq!(assignments, assign_trial(&trial, &replay_cohort, 1, 2)?);
    for cluster_id in [&clusters[0], &clusters[1]] {
        let cluster_arms = assignments
            .iter()
            .filter(|assignment| &assignment.cluster_id == cluster_id)
            .map(|assignment| assignment.variant.clone())
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(cluster_arms.len(), 1, "canary cluster must stay in one arm");
    }

    // A complete canary produces a deterministic proposal, while the core
    // remains fail-closed: external review is required for release sign-off.
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
    let mut evidence = TrialEvidence {
        bundle_digests_at_analysis: trial.immutable_bundle_digests.clone(),
        protected_oracle_digest_at_analysis: trial.protected_oracle_digest.clone(),
        trial,
        features: vec![FeatureManifest {
            feature_id: FeatureId::new("round14-canary-feature")?,
            digest: digest('e'),
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
    };
    evidence.independent_analysis = Some(AnalysisReceipt {
        receipt_digest: digest('f'),
        analyst: PrincipalId::new("round14-independent-reviewer")?,
        analyzed_evidence_digest: trial_evidence_digest(&evidence)?,
        recommendation: AnalysisRecommendation::RecommendIndependentReview,
    });
    let decision = propose_trial_decision(&evidence, vec![digest('b')])?;
    assert_eq!(
        decision,
        propose_trial_decision(&evidence, vec![digest('b')])?
    );
    assert!(matches!(
        decision,
        TrialDecision::PromotableProposal {
            promotion_authorized: false,
            rollback,
            ..
        } if rollback.applies_to_new_work_only
    ));

    // Tampering with canary evidence invalidates the receipt binding and
    // cannot authorize promotion.
    let mut tampered = evidence.clone();
    tampered.outcomes[0].analysis_value_micros = Some(DecimalU64::new(1201));
    assert!(matches!(
        propose_trial_decision(&tampered, vec![digest('b')])?,
        TrialDecision::InsufficientEvidence {
            promotion_authorized: false,
            reasons,
        } if reasons.iter().any(|reason| reason == "analysis receipt does not bind evidence")
    ));

    // P32: replay order is stable, but unsafe paths and identity collisions
    // stop conflict generation before any release review can proceed.
    let candidate =
        |candidate_id: &str, file_path: &str| -> Result<CandidateChange, Box<dyn Error>> {
            Ok(CandidateChange {
                project_id: ProjectId::new("round14-qualification")?,
                root_id: RootId::new("round14-scope")?,
                task_id: TaskId::new(candidate_id)?,
                candidate_id: candidate_id.into(),
                base_sha: digest('1'),
                head_sha: digest('2'),
                tree_sha: digest('3'),
                ancestry_digest: Some(digest('4')),
                files: vec![FileChange {
                    path: file_path.into(),
                    hunks: None,
                }],
                constraints: None,
            })
        };
    let candidates = vec![
        candidate("canary-a", "variants/control/handler.rs")?,
        candidate("canary-b", "variants/candidate/handler.rs")?,
    ];
    let generation = analyze_conflicts(&candidates, "round14-release-policy")?;
    let mut reversed = candidates.clone();
    reversed.reverse();
    assert_eq!(
        generation,
        analyze_conflicts(&reversed, "round14-release-policy")?
    );
    assert_eq!(generation.generation.candidate_ids.len(), 2);
    assert!(generation.generation.combined_verification_required);

    let unsafe_candidate = candidate("unsafe-canary", "../release.toml")?;
    assert!(matches!(
        analyze_conflicts(&[unsafe_candidate], "round14-release-policy"),
        Err(ConflictError::InvalidPath(_))
    ));
    let original = candidate("stable-canary", "src/handler.rs")?;
    let changed = candidate("stable-canary", "src/changed.rs")?;
    assert!(matches!(
        analyze_conflicts(&[original, changed], "round14-release-policy"),
        Err(ConflictError::CandidateCollision(_))
    ));

    Ok(())
}
