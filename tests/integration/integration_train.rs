use admiralty_core::{
    ProjectId, RootId, TaskId,
    conflicts::{
        CandidateChange, CandidateConstraints, ConflictError, ConflictRisk, FileChange, HunkRange,
        InterfaceRevision, analyze_conflicts, probe_cache_key,
    },
};

fn digest(byte: char) -> String {
    format!("sha256:{}", byte.to_string().repeat(64))
}

fn candidate(id: &str, base: char, paths: &[&str]) -> CandidateChange {
    CandidateChange {
        project_id: ProjectId::new("project").expect("valid project id"),
        root_id: RootId::new("root").expect("valid root id"),
        task_id: TaskId::new(id).expect("valid task id"),
        candidate_id: id.to_owned(),
        base_sha: digest(base),
        head_sha: digest('b'),
        tree_sha: digest('c'),
        ancestry_digest: Some(digest('d')),
        files: paths
            .iter()
            .map(|path| FileChange {
                path: (*path).to_owned(),
                hunks: None,
            })
            .collect(),
        constraints: Some(CandidateConstraints {
            contract_revision: 1,
            contract_digest: digest('e'),
            owned_paths: paths.iter().map(|path| (*path).to_owned()).collect(),
            denied_paths: Vec::new(),
            interfaces: vec![InterfaceRevision {
                interface_id: "runtime-api".to_owned(),
                revision: 1,
            }],
        }),
    }
}

#[test]
fn overlapping_changes_produce_conflict_edges_without_claiming_merge_safety() {
    let mut left = candidate("left", 'a', &["crates/admiralty-runtime/src/lib.rs"]);
    let mut right = candidate("right", 'a', &["crates/admiralty-runtime/src/lib.rs"]);
    left.files[0].hunks = Some(vec![HunkRange { start: 10, end: 20 }]);
    right.files[0].hunks = Some(vec![HunkRange { start: 20, end: 24 }]);

    let graph = analyze_conflicts(&[left, right], "policy-1").expect("bounded graph");
    assert_eq!(graph.edges.len(), 1);
    assert!(
        graph.edges[0]
            .risks
            .contains(&ConflictRisk::OverlappingHunks)
    );
    assert!(graph.generation.combined_verification_required);
}

#[test]
fn changed_ancestry_changes_cache_identity_even_when_tree_is_unchanged() {
    let original = candidate("candidate", 'a', &["crates/admiralty-runtime/src/lib.rs"]);
    let mut changed_ancestry = original.clone();
    changed_ancestry.ancestry_digest = Some(digest('f'));

    assert_eq!(original.tree_sha, changed_ancestry.tree_sha);
    assert_ne!(
        probe_cache_key(&[original], "policy-1").expect("cache key"),
        probe_cache_key(&[changed_ancestry], "policy-1").expect("cache key")
    );
}

#[test]
fn stale_bases_are_reported_and_candidate_order_is_stable() {
    let first = candidate("first", 'a', &["crates/admiralty-runtime/src/first.rs"]);
    let second = candidate("second", 'b', &["crates/admiralty-runtime/src/second.rs"]);
    let forward =
        analyze_conflicts(&[first.clone(), second.clone()], "policy-1").expect("conflict analysis");
    let reverse = analyze_conflicts(&[second, first], "policy-1").expect("conflict analysis");

    assert_eq!(forward, reverse);
    assert!(forward.edges[0].risks.contains(&ConflictRisk::BaseMismatch));
    assert_eq!(forward.generation.candidate_ids, ["first", "second"]);
}

#[test]
fn three_candidates_freeze_one_deterministic_generation() {
    let candidates = [
        candidate("a", 'a', &["crates/a.rs"]),
        candidate("b", 'a', &["crates/b.rs"]),
        candidate("c", 'a', &["crates/c.rs"]),
    ];
    let graph = analyze_conflicts(&candidates, "policy-1").expect("conflict analysis");

    assert!(graph.edges.is_empty());
    assert_eq!(graph.generation.candidate_ids, ["a", "b", "c"]);
    assert!(graph.generation.combined_verification_required);
}

#[test]
fn invalid_candidate_input_is_rejected_before_analysis() {
    let mut invalid = candidate("candidate", 'a', &["../outside.rs"]);
    invalid.files[0].hunks = Some(vec![HunkRange { start: 0, end: 1 }]);

    assert!(matches!(
        analyze_conflicts(&[invalid], "policy-1"),
        Err(ConflictError::InvalidPath(_))
    ));
}
