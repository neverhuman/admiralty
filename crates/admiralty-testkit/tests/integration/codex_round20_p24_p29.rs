#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Round 20 post-PR-57 placement release qualification and audit.

use admiralty_core::placement::{
    ActiveRoot, BudgetRequirement, Capacity, DecimalU64, Known, NodeId, NodeOffer, Observation,
    PlacementError, PlacementPolicy, PlacementRequest, propose_placement,
};
use admiralty_core::{ProjectId, RootId, TaskId};

fn d(value: u64) -> DecimalU64 {
    DecimalU64::new(value)
}

fn request() -> PlacementRequest {
    PlacementRequest {
        project: ProjectId::new("round20-project").expect("project fixture"),
        root: RootId::new("round20-root").expect("root fixture"),
        task: TaskId::new("round20-task").expect("task fixture"),
        contract_digest: format!("sha256:{}", "f".repeat(64)),
        authority_ref: "round20-authority".into(),
        required_capabilities: vec!["rust".into()],
        locality_refs: vec!["src".into()],
        budget_requirement: BudgetRequirement {
            account_pool: "round20-pool".into(),
            amount: d(5),
        },
    }
}

fn policy() -> PlacementPolicy {
    PlacementPolicy {
        max_observation_age: d(5),
        maximum_pressure: d(4),
        minimum_spool_reserve: d(2),
        disconnected_continuation_only: true,
        ownership_revision: Some(d(21)),
    }
}

fn offer(id: &str, slots: u64, ownership_revision: u64, tick: u64) -> NodeOffer {
    NodeOffer {
        node_id: NodeId::new(id).expect("node fixture"),
        boot_id: "round20-boot".into(),
        authority_generation: d(3),
        visible_scopes: Observation {
            value: vec!["round20-project".into()],
            observed_at: d(tick),
        },
        capabilities: Observation {
            value: vec!["rust".into()],
            observed_at: d(tick),
        },
        account_pool: Observation {
            value: "round20-pool".into(),
            observed_at: d(tick),
        },
        capacity: Observation {
            value: Known::Known {
                value: Capacity {
                    slots: d(slots),
                    budget_remaining: d(10),
                },
            },
            observed_at: d(tick),
        },
        current_pressure: Observation {
            value: Known::Known { value: d(1) },
            observed_at: d(tick),
        },
        spool_free: Observation {
            value: Known::Known { value: d(8) },
            observed_at: d(tick),
        },
        existing_active_roots: vec![],
        controller_connected: true,
        locality_refs: vec!["src".into()],
        continuation_permit: None,
        reconciliation_required: false,
        ownership_observed_at: Some(d(tick)),
        ownership_revision: Some(d(ownership_revision)),
    }
}

fn assert_candidate_only(proposal: &admiralty_core::placement::PlacementProposal) {
    assert!(!proposal.execution_granted);
    assert!(!proposal.publication_granted);
    assert!(!proposal.authority_granted);
}

#[test]
fn codex_round20_p24_p29_post_pr57_release_qualification() {
    let task = request();
    let rules = policy();
    let inventory = vec![offer("node-b", 1, 21, 210), offer("node-a", 2, 21, 210)];

    // The exact same release inventory yields identical digest and ranking on replay.
    let initial = propose_placement(&task, &inventory, &rules, 210).expect("valid inventory");
    let replay = propose_placement(&task, &inventory, &rules, 210).expect("same inventory replay");
    assert_eq!(initial, replay);
    assert_eq!(initial.candidates.len(), 2);
    assert_eq!(initial.candidates[0].node_id.as_str(), "node-a");
    assert_candidate_only(&initial);

    // Capacity loss and stale ownership each stop candidate creation.
    let mut capacity_unknown = offer("node-unknown", 1, 21, 211);
    capacity_unknown.capacity.value = Known::Unknown {
        reason: "capacity measurement absent".into(),
    };
    let capacity_blocked =
        propose_placement(&task, &[capacity_unknown], &rules, 211).expect("closed capacity gate");
    assert!(capacity_blocked.candidates.is_empty());
    assert_eq!(capacity_blocked.rejected.len(), 1);
    assert_candidate_only(&capacity_blocked);

    let topology_blocked =
        propose_placement(&task, &[offer("node-stale", 1, 20, 211)], &rules, 211)
            .expect("closed topology gate");
    assert!(topology_blocked.candidates.is_empty());
    assert_eq!(
        topology_blocked.rejected[0].reason,
        "topology ownership unknown, stale, or unreconciled"
    );
    assert_candidate_only(&topology_blocked);

    // Conflicting active-root ownership is a hard error, never a fallback candidate.
    let mut owned = offer("node-owner", 2, 21, 212);
    owned.existing_active_roots = vec![ActiveRoot {
        root: task.root.clone(),
        ordinary: true,
    }];
    let mut duplicate_owner = offer("node-duplicate-owner", 2, 21, 212);
    duplicate_owner.existing_active_roots = owned.existing_active_roots.clone();
    assert_eq!(
        propose_placement(&task, &[owned, duplicate_owner], &rules, 212,),
        Err(PlacementError::DuplicateRootEvidence)
    );
}
