//! Round 8 Item 2 cross-packet packaging qualification.
//!
//! This offline integration check binds the dashboard's published build entry
//! points (P12) to deterministic trial and integration artifacts (P28/P32).
//! It does not run a browser build or authorize production rollout.

use admiralty_core::{
    ProjectId, RootId, TaskId,
    conflicts::{CandidateChange, FileChange, analyze_conflicts},
    trials::{
        ClusterAssignment, ClusterId, ClusterMember, DecimalU64, TrialId, TrialSpec, VariantId,
        assign_trial,
    },
};
use serde_json::Value;

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
