//! Round 15 release smoke and multi-step deterministic placement stress.

use admiralty_core::placement::{
    BudgetRequirement, Capacity, DecimalU64, Known, NodeId, NodeOffer, Observation, PlacementError,
    PlacementPolicy, PlacementRequest, propose_placement, propose_placement_recorded,
};
use admiralty_core::{ProjectId, RootId, TaskId};

fn number(value: u64) -> DecimalU64 {
    DecimalU64::new(value)
}

fn request() -> PlacementRequest {
    PlacementRequest {
        project: ProjectId::new("round15-project").expect("fixture project"),
        root: RootId::new("round15-root").expect("fixture root"),
        task: TaskId::new("round15-task").expect("fixture task"),
        contract_digest: format!("sha256:{}", "b".repeat(64)),
        authority_ref: "round15-authority".into(),
        required_capabilities: vec!["rust".into()],
        locality_refs: vec!["src".into()],
        budget_requirement: BudgetRequirement {
            account_pool: "round15-pool".into(),
            amount: number(3),
        },
    }
}

fn policy() -> PlacementPolicy {
    PlacementPolicy {
        max_observation_age: number(4),
        maximum_pressure: number(5),
        minimum_spool_reserve: number(2),
        disconnected_continuation_only: true,
        ownership_revision: Some(number(11)),
    }
}

fn offer(id: &str, slots: u64, revision: u64, observed_at: u64) -> NodeOffer {
    NodeOffer {
        node_id: NodeId::new(id).expect("fixture node"),
        boot_id: "round15-boot-1".into(),
        authority_generation: number(3),
        visible_scopes: Observation {
            value: vec!["round15-project".into()],
            observed_at: number(observed_at),
        },
        capabilities: Observation {
            value: vec!["rust".into()],
            observed_at: number(observed_at),
        },
        account_pool: Observation {
            value: "round15-pool".into(),
            observed_at: number(observed_at),
        },
        capacity: Observation {
            value: Known::Known {
                value: Capacity {
                    slots: number(slots),
                    budget_remaining: number(9),
                },
            },
            observed_at: number(observed_at),
        },
        current_pressure: Observation {
            value: Known::Known { value: number(1) },
            observed_at: number(observed_at),
        },
        spool_free: Observation {
            value: Known::Known { value: number(7) },
            observed_at: number(observed_at),
        },
        existing_active_roots: vec![],
        controller_connected: true,
        locality_refs: vec!["src".into()],
        continuation_permit: None,
        reconciliation_required: false,
        ownership_observed_at: Some(number(observed_at)),
        ownership_revision: Some(number(revision)),
    }
}

#[test]
fn codex_round15_p24_p29_release_smoke_and_multi_step_replay() {
    let request = request();
    let policy = policy();

    // Repeated scheduling across successive ticks must preserve stable results
    // for the same observations while allowing capacity to recover next step.
    for (tick, slots) in [(30, 2), (31, 1), (32, 0), (33, 2), (34, 2)] {
        let observations = vec![offer("node-east", slots, 11, tick)];
        let scheduled = propose_placement(&request, &observations, &policy, tick)
            .expect("valid step produces a placement proposal");
        let replay = propose_placement(&request, &observations, &policy, tick)
            .expect("identical step replay produces a proposal");
        assert_eq!(scheduled, replay, "replay diverged at tick {tick}");
        assert!(!scheduled.execution_granted);
        assert!(!scheduled.publication_granted);
        assert!(!scheduled.authority_granted);
        if slots == 0 {
            assert!(scheduled.candidates.is_empty());
            assert_eq!(scheduled.rejected.len(), 1);
        } else {
            assert_eq!(scheduled.candidates.len(), 1);
            assert!(scheduled.rejected.is_empty());
        }
    }

    // Old topology and capacity observations cannot be reused at a later tick.
    let stale = propose_placement(&request, &[offer("node-stale", 2, 11, 30)], &policy, 35)
        .expect("stale topology and capacity evidence is a safe rejection");
    assert!(stale.candidates.is_empty());
    assert_eq!(stale.rejected.len(), 1);
    assert!(!stale.execution_granted);
    assert!(!stale.publication_granted);
    assert!(!stale.authority_granted);

    // Unknown capacity is a normal fail-closed rejection with no candidate.
    let mut unknown_capacity = offer("node-unknown", 2, 11, 36);
    unknown_capacity.capacity.value = Known::Unknown {
        reason: "capacity telemetry unavailable".into(),
    };
    let unknown = propose_placement(&request, &[unknown_capacity], &policy, 36)
        .expect("unknown capacity is represented as a safe rejection");
    assert!(unknown.candidates.is_empty());
    assert_eq!(unknown.rejected.len(), 1);
    assert!(!unknown.execution_granted);
    assert!(!unknown.publication_granted);
    assert!(!unknown.authority_granted);

    // A rolled-back decision clock is a hard error, even with valid evidence.
    assert_eq!(
        propose_placement_recorded(
            &request,
            &[offer("node-clock", 2, 11, 36)],
            &policy,
            Some(35),
            Some(36),
        ),
        Err(PlacementError::Clock),
        "a rolled-back scheduler tick must be rejected before admission"
    );
}
