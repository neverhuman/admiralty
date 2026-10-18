//! Round 17 cross-packet invariants for deterministic placement and topology.

use admiralty_core::placement::{
    ActiveRoot, BudgetRequirement, Capacity, DecimalU64, Known, NodeId, NodeOffer, Observation,
    PlacementError, PlacementPolicy, PlacementRequest, propose_placement,
};
use admiralty_core::{ProjectId, RootId, TaskId};

fn n(value: u64) -> DecimalU64 {
    DecimalU64::new(value)
}

fn request() -> PlacementRequest {
    PlacementRequest {
        project: ProjectId::new("round17-project").expect("valid project"),
        root: RootId::new("round17-root").expect("valid root"),
        task: TaskId::new("round17-task").expect("valid task"),
        contract_digest: format!("sha256:{}", "d".repeat(64)),
        authority_ref: "round17-authority".into(),
        required_capabilities: vec!["rust".into()],
        locality_refs: vec!["src".into(), "docs".into()],
        budget_requirement: BudgetRequirement {
            account_pool: "round17-pool".into(),
            amount: n(6),
        },
    }
}

fn policy() -> PlacementPolicy {
    PlacementPolicy {
        max_observation_age: n(4),
        maximum_pressure: n(3),
        minimum_spool_reserve: n(2),
        disconnected_continuation_only: true,
        ownership_revision: Some(n(12)),
    }
}

fn offer(id: &str, slots: u64, budget: u64, locality: &[&str]) -> NodeOffer {
    NodeOffer {
        node_id: NodeId::new(id).expect("valid node"),
        boot_id: "round17-boot".into(),
        authority_generation: n(5),
        visible_scopes: Observation {
            value: vec!["round17-project".into()],
            observed_at: n(80),
        },
        capabilities: Observation {
            value: vec!["rust".into()],
            observed_at: n(80),
        },
        account_pool: Observation {
            value: "round17-pool".into(),
            observed_at: n(80),
        },
        capacity: Observation {
            value: Known::Known {
                value: Capacity {
                    slots: n(slots),
                    budget_remaining: n(budget),
                },
            },
            observed_at: n(80),
        },
        current_pressure: Observation {
            value: Known::Known { value: n(1) },
            observed_at: n(80),
        },
        spool_free: Observation {
            value: Known::Known { value: n(9) },
            observed_at: n(80),
        },
        existing_active_roots: vec![],
        controller_connected: true,
        locality_refs: locality.iter().map(|item| (*item).to_owned()).collect(),
        continuation_permit: None,
        reconciliation_required: false,
        ownership_observed_at: Some(n(80)),
        ownership_revision: Some(n(12)),
    }
}

fn assert_non_authorizing(proposal: &admiralty_core::placement::PlacementProposal) {
    assert!(!proposal.execution_granted);
    assert!(!proposal.publication_granted);
    assert!(!proposal.authority_granted);
}

#[test]
fn codex_round17_p24_p29() {
    let request = request();
    let policy = policy();
    let candidates = vec![
        offer("node-z", 1, 8, &["src"]),
        offer("node-a", 2, 10, &["src", "docs"]),
        offer("node-m", 1, 5, &["docs"]),
    ];

    // Input ordering cannot change candidate ranking, rejections, or provenance digest.
    let forward = propose_placement(&request, &candidates, &policy, 80)
        .expect("valid topology produces a deterministic proposal");
    let reverse: Vec<_> = candidates.iter().cloned().rev().collect();
    let replay = propose_placement(&request, &reverse, &policy, 80)
        .expect("permuted topology produces the same proposal");
    assert_eq!(forward, replay);
    assert_eq!(forward.candidates.len(), 2);
    assert_eq!(forward.candidates[0].node_id.as_str(), "node-a");
    assert_eq!(forward.candidates[1].node_id.as_str(), "node-z");
    assert_eq!(forward.rejected.len(), 1);
    assert_eq!(forward.rejected[0].node_id.as_str(), "node-m");
    assert_eq!(
        forward.rejected[0].reason,
        "capacity or budget insufficient"
    );
    assert_non_authorizing(&forward);

    // One observed active root blocks every other node in the topology.
    let mut active = offer("node-active", 2, 10, &["src", "docs"]);
    active.existing_active_roots = vec![ActiveRoot {
        root: request.root.clone(),
        ordinary: true,
    }];
    let blocked = propose_placement(
        &request,
        &[active, offer("node-other", 2, 10, &["src"])],
        &policy,
        80,
    )
    .expect("known active-root ownership blocks new placement");
    assert!(blocked.candidates.is_empty());
    assert_eq!(blocked.rejected.len(), 2);
    assert!(
        blocked
            .rejected
            .iter()
            .all(|rejection| { rejection.reason == "root already active in topology" })
    );
    assert_non_authorizing(&blocked);

    // Conflicting reports for the same active root are rejected as ambiguous evidence.
    let mut first_owner = offer("node-owner-a", 1, 10, &["src"]);
    first_owner.existing_active_roots = vec![ActiveRoot {
        root: request.root.clone(),
        ordinary: true,
    }];
    let mut second_owner = offer("node-owner-b", 1, 10, &["docs"]);
    second_owner.existing_active_roots = first_owner.existing_active_roots.clone();
    assert_eq!(
        propose_placement(&request, &[first_owner, second_owner], &policy, 80,),
        Err(PlacementError::DuplicateRootEvidence)
    );
}
