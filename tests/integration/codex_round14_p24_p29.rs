//! Round 14 deterministic release readiness checks for placement and topology.

use admiralty_core::{ProjectId, RootId, TaskId};
use admiralty_core::placement::{
    BudgetRequirement, Capacity, DecimalU64, Known, NodeId, NodeOffer, Observation,
    PlacementError, PlacementPolicy, PlacementRequest, propose_placement,
};

fn amount(value: u64) -> DecimalU64 {
    DecimalU64::new(value)
}

fn request() -> PlacementRequest {
    PlacementRequest {
        project: ProjectId::new("round14-project").expect("valid project fixture"),
        root: RootId::new("round14-root").expect("valid root fixture"),
        task: TaskId::new("round14-task").expect("valid task fixture"),
        contract_digest: format!("sha256:{}", "a".repeat(64)),
        authority_ref: "round14-authority".into(),
        required_capabilities: vec!["rust".into()],
        locality_refs: vec!["src".into()],
        budget_requirement: BudgetRequirement {
            account_pool: "round14-pool".into(),
            amount: amount(4),
        },
    }
}

fn policy() -> PlacementPolicy {
    PlacementPolicy {
        max_observation_age: amount(5),
        maximum_pressure: amount(3),
        minimum_spool_reserve: amount(2),
        disconnected_continuation_only: true,
        ownership_revision: Some(amount(7)),
    }
}

fn offer(id: &str, slots: u64, ownership_revision: u64) -> NodeOffer {
    NodeOffer {
        node_id: NodeId::new(id).expect("valid node fixture"),
        boot_id: "round14-boot".into(),
        authority_generation: amount(2),
        visible_scopes: Observation {
            value: vec!["round14-project".into()],
            observed_at: amount(20),
        },
        capabilities: Observation {
            value: vec!["rust".into()],
            observed_at: amount(20),
        },
        account_pool: Observation {
            value: "round14-pool".into(),
            observed_at: amount(20),
        },
        capacity: Observation {
            value: Known::Known {
                value: Capacity {
                    slots: amount(slots),
                    budget_remaining: amount(10),
                },
            },
            observed_at: amount(20),
        },
        current_pressure: Observation {
            value: Known::Known { value: amount(1) },
            observed_at: amount(20),
        },
        spool_free: Observation {
            value: Known::Known { value: amount(8) },
            observed_at: amount(20),
        },
        existing_active_roots: vec![],
        controller_connected: true,
        locality_refs: vec!["src".into()],
        continuation_permit: None,
        reconciliation_required: false,
        ownership_observed_at: Some(amount(20)),
        ownership_revision: Some(amount(ownership_revision)),
    }
}

#[test]
fn codex_round14_p24_p29() {
    let request = request();
    let policy = policy();
    let offers = vec![offer("node-a", 1, 7), offer("node-b", 2, 7)];

    let first = propose_placement(&request, &offers, &policy, 20)
        .expect("valid capacity and topology evidence yields a proposal");
    let replay = propose_placement(&request, &offers, &policy, 20)
        .expect("replay yields a proposal");
    assert_eq!(first, replay, "identical evidence must replay identically");
    assert_eq!(first.candidates.len(), 2);
    assert!(!first.execution_granted);
    assert!(!first.publication_granted);
    assert!(!first.authority_granted);

    let capacity_blocked = propose_placement(&request, &[offer("node-full", 0, 7)], &policy, 20)
        .expect("capacity exhaustion is represented as a safe rejection");
    assert!(capacity_blocked.candidates.is_empty());
    assert_eq!(capacity_blocked.rejected.len(), 1);
    assert!(!capacity_blocked.execution_granted);

    let stale_topology = propose_placement(&request, &[offer("node-stale", 1, 6)], &policy, 20)
        .expect("stale ownership evidence fails closed");
    assert!(stale_topology.candidates.is_empty());
    assert_eq!(stale_topology.rejected.len(), 1);
    assert_eq!(
        stale_topology.rejected[0].reason,
        "topology ownership unknown, stale, or unreconciled"
    );
    assert!(!stale_topology.execution_granted);
    assert!(!stale_topology.publication_granted);
    assert!(!stale_topology.authority_granted);

    assert_eq!(
        propose_placement(&request, &[offer("duplicate", 1, 7), offer("duplicate", 1, 7)], &policy, 20),
        Err(PlacementError::DuplicateNode),
        "ambiguous duplicate topology must be rejected before scheduling"
    );
}
