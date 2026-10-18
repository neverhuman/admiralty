//! Round 8 Item 2 cross-packet packaging qualification.
//!
//! This offline integration check binds the dashboard's published build entry
//! points (P12) to deterministic trial and integration artifacts (P28/P32).
//! It does not run a browser build or authorize production rollout.

use admiralty_core::{
    PrincipalId, ProjectId, RootId, TaskId,
    conflicts::{CandidateChange, ConflictError, FileChange, analyze_conflicts},
    economics::{DecimalU128, ObservedAmount},
    placement::decode_input,
    trials::{
        AnalysisReceipt, AnalysisRecommendation, ClusterAssignment, ClusterId, ClusterMember,
        DecimalU64, DecisionPhase, FeatureId, FeatureManifest, Missingness, Outcome, OutcomeLabel,
        TrialDecision, TrialEvidence, TrialId, TrialSpec, VariantId, assign_trial,
        propose_trial_decision, trial_evidence_digest,
    },
};
use admiralty_runtime::{
    providers::claude::{ClaudeError, parse_stream},
    recovery::{Blocker, plan_recovery},
    session::{InputKind, LaunchSpec, SessionError, SessionHost},
};
use admiralty_store::artifacts::ArtifactStore;
use admiralty_testkit::clock::{ClockError, FakeClock};
use serde_json::Value;
use std::os::unix::fs::PermissionsExt;
use std::{fs, path::PathBuf};
use tempfile::tempdir;

#[path = "support/recovery.rs"]
mod recovery_fixture;

const DASHBOARD_PACKAGE: &str = include_str!("../../apps/dashboard/package.json");
const DASHBOARD_INDEX: &str = include_str!("../../apps/dashboard/index.html");
const DASHBOARD_VITE: &str = include_str!("../../apps/dashboard/vite.config.ts");
const TEMPORAL_PLOTS: &str = include_str!("../../apps/dashboard/src/ui/TemporalPlots.tsx");

fn digest(byte: char) -> String {
    format!("sha256:{}", byte.to_string().repeat(64))
}

#[test]
fn p28_p32_round8_p12_packaging_qualification() {
    // The P12 package must expose the same build and test entry points that CI
    // and operators use, and its HTML entry must resolve to the Vite app.
    let package: Value = serde_json::from_str(DASHBOARD_PACKAGE).expect("dashboard package JSON");
    assert_eq!(package["name"], "@admiralty/dashboard");
    assert_eq!(package["scripts"]["build"], "tsc -b && vite build");
    assert_eq!(package["scripts"]["test"], "vitest");
    assert!(DASHBOARD_INDEX.contains("/src/main.tsx"));
    assert!(DASHBOARD_VITE.contains("defineConfig"));
    assert!(TEMPORAL_PLOTS.contains("TemporalPlots"));

    // P28: fixed input order and a preregistered seed produce byte-for-byte
    // equivalent assignments, including the manager-cluster grouping.
    let control = VariantId::new("control").expect("control variant");
    let candidate = VariantId::new("candidate").expect("candidate variant");
    let cluster_a = ClusterId::new("manager-a").expect("cluster A");
    let cluster_b = ClusterId::new("manager-b").expect("cluster B");
    let cohort = [
        ("root-a1", cluster_a.clone()),
        ("root-a2", cluster_a),
        ("root-b1", cluster_b.clone()),
        ("root-b2", cluster_b),
    ]
    .into_iter()
    .map(|(root, cluster_id)| ClusterMember {
        root_id: RootId::new(root).expect("root ID"),
        cluster_id,
    })
    .collect::<Vec<_>>();
    let spec = TrialSpec {
        trial_id: TrialId::new("round8-item2").expect("trial ID"),
        project_id: ProjectId::new("packaging").expect("project ID"),
        scope_root_id: RootId::new("scope-root").expect("scope root"),
        scope_task_id: TaskId::new("scope-task").expect("scope task"),
        contract_revision: 1,
        contract_digest: digest('a'),
        fixed_cohort: cohort.clone(),
        immutable_bundle_digests: vec![digest('b')],
        protected_oracle_digest: digest('c'),
        pre_registered_seed: "round8-seed".into(),
        decision_tick: DecimalU64::new(10),
        cluster_assignments: vec![
            ClusterAssignment {
                cluster_id: ClusterId::new("manager-a").expect("cluster A"),
                variants: vec![control.clone(), candidate.clone()],
            },
            ClusterAssignment {
                cluster_id: ClusterId::new("manager-b").expect("cluster B"),
                variants: vec![control, candidate],
            },
        ],
        required_maturity: DecimalU64::new(2),
        planned_looks: DecimalU64::new(1),
    };
    let first = assign_trial(&spec, &cohort, 1, 2).expect("first deterministic assignment");
    let replay = assign_trial(&spec, &cohort, 1, 2).expect("replayed deterministic assignment");
    assert_eq!(first, replay);
    for cluster_id in ["manager-a", "manager-b"] {
        let arms = first
            .iter()
            .filter(|assignment| assignment.cluster_id.as_str() == cluster_id)
            .map(|assignment| &assignment.variant)
            .collect::<Vec<_>>();
        assert_eq!(arms.len(), 2);
        assert_eq!(arms[0], arms[1]);
    }

    // P32 integration artifact: candidate serialization order cannot perturb
    // the frozen conflict generation or its stable candidate IDs.
    let make_candidate = |id: &str, path: &str| CandidateChange {
        project_id: ProjectId::new("packaging").expect("project ID"),
        root_id: RootId::new("scope-root").expect("root ID"),
        task_id: TaskId::new(id).expect("task ID"),
        candidate_id: id.into(),
        base_sha: digest('d'),
        head_sha: digest('e'),
        tree_sha: digest('f'),
        ancestry_digest: None,
        files: vec![FileChange {
            path: path.into(),
            hunks: None,
        }],
        constraints: None,
    };
    let a = make_candidate("candidate-a", "apps/dashboard/src/main.tsx");
    let b = make_candidate("candidate-b", "apps/dashboard/src/ui/App.tsx");
    let forward =
        analyze_conflicts(&[a.clone(), b.clone()], "round8").expect("integration generation");
    let reverse = analyze_conflicts(&[b, a], "round8").expect("replayed integration generation");
    assert_eq!(forward, reverse);
    assert_eq!(
        forward.generation.candidate_ids,
        ["candidate-a", "candidate-b"]
    );
}

#[test]
fn p28_round9_item3_golden_webhook_hitl_qualification() {
    // P04: content-addressed delivery is verifiable and byte-limit rejection
    // leaves the accepted object unchanged.
    let temp = tempdir().expect("temporary fixture");
    let artifact_root = temp.path().join("objects");
    fs::create_dir(&artifact_root).expect("artifact root");
    fs::set_permissions(&artifact_root, fs::Permissions::from_mode(0o700))
        .expect("private artifact root");
    let store = ArtifactStore::open(&artifact_root, 8).expect("artifact store");
    let artifact = store
        .publish_bytes(b"golden")
        .expect("publish webhook evidence");
    assert_eq!(
        store.read_verified(&artifact).expect("verified evidence"),
        b"golden"
    );
    assert!(store.publish_bytes(b"too many bytes").is_err());
    assert_eq!(
        store
            .read_verified(&artifact)
            .expect("evidence remains intact"),
        b"golden"
    );

    // P07 HITL edge: a human reply advances the epoch and makes a queued
    // webhook automation reply stale without changing the journal.
    let spool = temp.path().join("spool");
    fs::create_dir(&spool).expect("spool root");
    fs::set_permissions(&spool, fs::Permissions::from_mode(0o700)).expect("private spool root");
    let host = SessionHost::new();
    host.launch(LaunchSpec {
        run_id: "round9-hitl".into(),
        program: PathBuf::from("/bin/cat"),
        args: vec![],
        cwd: temp.path().to_owned(),
        spool_dir: spool.join("run"),
    })
    .expect("launch deterministic echo fixture");
    let human = host
        .write_input("round9-hitl", InputKind::Human, b"approve\n")
        .expect("human reply");
    assert_eq!(human.input_epoch, 1);
    assert!(matches!(
        host.write_input(
            "round9-hitl",
            InputKind::Automation { expected_epoch: 0 },
            b"stale\n"
        ),
        Err(SessionError::StaleInput {
            expected: 0,
            current: 1
        })
    ));
    host.close_input_if_epoch("round9-hitl", 1)
        .expect("close fixture input");
    assert!(host.wait("round9-hitl").expect("reap fixture").success());

    // P18: simulated webhook timestamps cannot rewind the qualification clock.
    let mut clock = FakeClock::new(40);
    let deadline = clock.deadline_after_ms(5).expect("fixed webhook deadline");
    assert_eq!(
        clock.advance_to_ms(39),
        Err(ClockError::Backwards {
            now_ms: 40,
            target_ms: 39
        })
    );
    assert_eq!(clock.now_ms(), 40);
    assert!(!deadline.is_due(clock.now_ms()));

    // P21: golden provider fixture is accepted, but missing terminal error
    // metadata remains unknown and never becomes trusted acceptance.
    let resume = include_bytes!("../fixtures/providers/claude/turn-boundary-resume.jsonl");
    let observed =
        parse_stream(resume, Some("fixture-session")).expect("golden provider observation");
    assert!(observed.is_error.is_none());
    assert!(!observed.trusted_acceptance());
    assert_eq!(
        parse_stream(resume, Some("foreign")),
        Err(ClaudeError::ResumeMismatch)
    );

    // P24: malformed webhook placement input is rejected before proposal.
    assert!(decode_input(br#"{"schema_version":1,"unknown":true}"#).is_err());

    // P25: a missing external fence blocks recovery even when the backup is
    // otherwise complete and deterministic.
    let (manifest, observation, _fence) = recovery_fixture::valid_recovery_evidence();
    let recovery = plan_recovery(&manifest, &observation, None).expect("recovery proposal");
    assert!(!recovery.resume_proposed);
    assert!(recovery.blockers.contains(&Blocker::MissingExternalFence));

    // P26: unknown provider spend is carried as unknown rather than zero.
    let unknown_cost = ObservedAmount::Unknown {
        reason: "webhook omitted cost".into(),
    };
    assert_eq!(
        serde_json::to_value(unknown_cost).expect("serialize cost")["kind"],
        "unknown"
    );
    assert!(matches!(
        DecimalU128::parse("01"),
        Err(admiralty_core::economics::EconomicsError::Invalid(_))
    ));
}

#[test]
fn p28_round10_item3_workspace_gate_audit() {
    // P28: reverse delivery order for the fixed preregistered cohort must not
    // change its seeded assignment.
    let control = VariantId::new("control").expect("control variant");
    let treatment = VariantId::new("treatment").expect("treatment variant");
    let cluster_a = ClusterId::new("cluster-a").expect("cluster A");
    let cluster_b = ClusterId::new("cluster-b").expect("cluster B");
    let cohort = vec![
        ClusterMember {
            root_id: RootId::new("root-a").expect("root A"),
            cluster_id: cluster_a.clone(),
        },
        ClusterMember {
            root_id: RootId::new("root-b").expect("root B"),
            cluster_id: cluster_b.clone(),
        },
    ];
    let spec = TrialSpec {
        trial_id: TrialId::new("round10-item2").expect("trial ID"),
        project_id: ProjectId::new("round10").expect("project ID"),
        scope_root_id: RootId::new("scope-root").expect("scope root"),
        scope_task_id: TaskId::new("scope-task").expect("scope task"),
        contract_revision: 1,
        contract_digest: digest('a'),
        fixed_cohort: cohort.clone(),
        immutable_bundle_digests: vec![digest('b')],
        protected_oracle_digest: digest('c'),
        pre_registered_seed: "golden-seed-round10".into(),
        decision_tick: DecimalU64::new(100),
        cluster_assignments: vec![
            ClusterAssignment {
                cluster_id: cluster_a,
                variants: vec![control.clone(), treatment.clone()],
            },
            ClusterAssignment {
                cluster_id: cluster_b,
                variants: vec![control, treatment],
            },
        ],
        required_maturity: DecimalU64::new(2),
        planned_looks: DecimalU64::new(1),
    };
    let expected = assign_trial(&spec, &cohort, 1, 2).expect("seeded cohort assignment");
    let mut replay_cohort = cohort.clone();
    replay_cohort.reverse();
    assert_eq!(
        expected,
        assign_trial(&spec, &replay_cohort, 1, 2).expect("replay reordered delivery")
    );

    // P25: a present but negative fencing observation cannot authorize resume.
    let (manifest, observation, mut fence) = recovery_fixture::valid_recovery_evidence();
    fence.old_executor_fenced = Some(false);
    let proposal =
        plan_recovery(&manifest, &observation, Some(&fence)).expect("blocked recovery proposal");
    assert!(!proposal.resume_proposed);
    assert!(proposal.blockers.contains(&Blocker::OldExecutorNotFenced));

    // P26: omitted costs stay explicitly unknown, and malformed exact counters
    // cannot be normalized into a plausible economic result.
    let absent_cost = ObservedAmount::Unknown {
        reason: "golden webhook omitted cost".into(),
    };
    let value = serde_json::to_value(absent_cost).expect("serialize unknown amount");
    assert_eq!(value["kind"], "unknown");
    assert_eq!(value["reason"], "golden webhook omitted cost");
    assert!(DecimalU128::parse("340282366920938463463374607431768211456").is_err());
}

#[test]
fn p28_p32_round11_item4_final_validation_gate() {
    // Freeze one P28 trial assignment, then carry that exact cohort into the
    // P32 combined candidate review. Reordered delivery must preserve both
    // the assignments and the conflict generation.
    let variants = [
        VariantId::new("control").expect("control variant"),
        VariantId::new("candidate").expect("candidate variant"),
    ];
    let clusters = [
        ClusterId::new("manager-a").expect("manager A"),
        ClusterId::new("manager-b").expect("manager B"),
    ];
    let cohort = [
        ("root-a1", 0),
        ("root-a2", 0),
        ("root-b1", 1),
        ("root-b2", 1),
    ]
    .into_iter()
    .map(|(root, cluster)| ClusterMember {
        root_id: RootId::new(root).expect("cohort root"),
        cluster_id: clusters[cluster].clone(),
    })
    .collect::<Vec<_>>();
    let spec = TrialSpec {
        trial_id: TrialId::new("round11-final-gate").expect("trial ID"),
        project_id: ProjectId::new("final-gate").expect("project ID"),
        scope_root_id: RootId::new("scope-root").expect("scope root"),
        scope_task_id: TaskId::new("scope-task").expect("scope task"),
        contract_revision: 1,
        contract_digest: digest('a'),
        fixed_cohort: cohort.clone(),
        immutable_bundle_digests: vec![digest('b')],
        protected_oracle_digest: digest('c'),
        pre_registered_seed: "round11-fixed-seed".into(),
        decision_tick: DecimalU64::new(100),
        cluster_assignments: clusters
            .iter()
            .map(|cluster_id| ClusterAssignment {
                cluster_id: cluster_id.clone(),
                variants: variants.to_vec(),
            })
            .collect(),
        required_maturity: DecimalU64::new(2),
        planned_looks: DecimalU64::new(1),
    };
    let assignments = assign_trial(&spec, &cohort, 1, 2).expect("fixed trial assignment");
    let mut reordered_cohort = cohort.clone();
    reordered_cohort.reverse();
    assert_eq!(
        assignments,
        assign_trial(&spec, &reordered_cohort, 1, 2).expect("replay reordered cohort")
    );
    for manager in ["manager-a", "manager-b"] {
        let arm_ids = assignments
            .iter()
            .filter(|assignment| assignment.cluster_id.as_str() == manager)
            .map(|assignment| assignment.variant.clone())
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(arm_ids.len(), 1, "shared-manager cohort stays in one arm");
    }

    let candidates = assignments
        .iter()
        .map(|assignment| {
            let id = format!("candidate-{}", assignment.root_id.as_str());
            CandidateChange {
                project_id: spec.project_id.clone(),
                root_id: spec.scope_root_id.clone(),
                task_id: TaskId::new(&id).expect("candidate task ID"),
                candidate_id: id,
                base_sha: digest('d'),
                head_sha: digest('e'),
                tree_sha: digest('f'),
                ancestry_digest: Some(digest('0')),
                files: vec![FileChange {
                    path: format!("variants/{}/src/handler.rs", assignment.variant.as_str()),
                    hunks: None,
                }],
                constraints: None,
            }
        })
        .collect::<Vec<_>>();
    let forward = analyze_conflicts(&candidates, "round11-policy").expect("combined validation");
    let mut reversed_candidates = candidates;
    reversed_candidates.reverse();
    let replay = analyze_conflicts(&reversed_candidates, "round11-policy")
        .expect("replayed combined validation");
    assert_eq!(forward, replay);
    assert_eq!(forward.generation.candidate_ids.len(), cohort.len());
    assert!(forward.generation.combined_verification_required);
    assert!(forward.edges.iter().all(|edge| {
        edge.risks
            .contains(&admiralty_core::conflicts::ConflictRisk::UnknownWriteScope)
    }));
}

#[test]
fn p28_p32_round12_item5_redteam_audit_gate() {
    // P28: an otherwise well-formed analysis receipt cannot authorize a
    // proposal after its bound evidence has changed.
    let variants = [
        VariantId::new("control").expect("control variant"),
        VariantId::new("candidate").expect("candidate variant"),
    ];
    let clusters = [
        ClusterId::new("audit-a").expect("cluster A"),
        ClusterId::new("audit-b").expect("cluster B"),
    ];
    let cohort = [("audit-root-a", 0), ("audit-root-b", 1)]
        .into_iter()
        .map(|(root, cluster)| ClusterMember {
            root_id: RootId::new(root).expect("cohort root"),
            cluster_id: clusters[cluster].clone(),
        })
        .collect::<Vec<_>>();
    let spec = TrialSpec {
        trial_id: TrialId::new("round12-redteam").expect("trial ID"),
        project_id: ProjectId::new("redteam").expect("project ID"),
        scope_root_id: RootId::new("audit-scope").expect("scope root"),
        scope_task_id: TaskId::new("audit-task").expect("scope task"),
        contract_revision: 1,
        contract_digest: digest('a'),
        fixed_cohort: cohort.clone(),
        immutable_bundle_digests: vec![digest('b')],
        protected_oracle_digest: digest('c'),
        pre_registered_seed: "round12-redteam-seed".into(),
        decision_tick: DecimalU64::new(50),
        cluster_assignments: clusters
            .iter()
            .map(|cluster_id| ClusterAssignment {
                cluster_id: cluster_id.clone(),
                variants: variants.to_vec(),
            })
            .collect(),
        required_maturity: DecimalU64::new(2),
        planned_looks: DecimalU64::new(1),
    };
    let assignments = assign_trial(&spec, &cohort, 1, 2).expect("assign fixed cohort");
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
        bundle_digests_at_analysis: spec.immutable_bundle_digests.clone(),
        protected_oracle_digest_at_analysis: spec.protected_oracle_digest.clone(),
        trial: spec,
        features: vec![FeatureManifest {
            feature_id: FeatureId::new("redteam-feature").expect("feature ID"),
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
    };
    let bound_digest = trial_evidence_digest(&evidence).expect("digest evidence");
    evidence.independent_analysis = Some(AnalysisReceipt {
        receipt_digest: digest('7'),
        analyst: PrincipalId::new("redteam-reviewer").expect("analyst ID"),
        analyzed_evidence_digest: bound_digest,
        recommendation: AnalysisRecommendation::RecommendIndependentReview,
    });
    evidence.outcomes[0].analysis_value_micros = Some(DecimalU64::new(1201));
    assert!(matches!(
        propose_trial_decision(&evidence, vec![digest('b')]).expect("fail-closed decision"),
        TrialDecision::InsufficientEvidence {
            promotion_authorized: false,
            reasons,
        } if reasons.iter().any(|reason| reason == "analysis receipt does not bind evidence")
    ));

    // P32: invalid paths and conflicting reuse of a candidate identity are
    // rejected before a conflict generation can be emitted.
    let candidate = CandidateChange {
        project_id: ProjectId::new("redteam").expect("project ID"),
        root_id: RootId::new("audit-scope").expect("root ID"),
        task_id: TaskId::new("audit-candidate").expect("task ID"),
        candidate_id: "audit-candidate".into(),
        base_sha: digest('d'),
        head_sha: digest('e'),
        tree_sha: digest('f'),
        ancestry_digest: Some(digest('0')),
        files: vec![FileChange {
            path: "src/handler.rs".into(),
            hunks: None,
        }],
        constraints: None,
    };
    let mut traversal = candidate.clone();
    traversal.files[0].path = "../escape.rs".into();
    assert!(matches!(
        analyze_conflicts(&[traversal], "round12-audit"),
        Err(ConflictError::InvalidPath(_))
    ));
    let mut collision = candidate.clone();
    collision.files[0].path = "src/changed.rs".into();
    assert_eq!(
        analyze_conflicts(&[candidate, collision], "round12-audit"),
        Err(ConflictError::CandidateCollision("audit-candidate".into()))
    );
}
