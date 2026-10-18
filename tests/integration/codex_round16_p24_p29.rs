//! Round 16 end-to-end placement cutover and fault-containment checks.

use admiralty_core::placement::{
    BudgetRequirement, Capacity, DecimalU64, Known, NodeId, NodeOffer, Observation, PlacementError,
    PlacementInput, PlacementPolicy, PlacementRequest, decode_input, propose_placement,
    propose_placement_recorded,
};
use admiralty_core::{ProjectId, RootId, TaskId};

fn value(number: u64) -> DecimalU64 {
    DecimalU64::new(number)
}

fn request() -> PlacementRequest {
    PlacementRequest {
        project: ProjectId::new("round16-project").expect("fixture project ID"),
        root: RootId::new("round16-root").expect("fixture root ID"),
        task: TaskId::new("round16-task").expect("fixture task ID"),
        contract_digest: format!("sha256:{}", "c".repeat(64)),
        authority_ref: "round16-authority".into(),
        required_capabilities: vec!["rust".into()],
        locality_refs: vec!["src".into()],
        budget_requirement: BudgetRequirement {
            account_pool: "round16-pool".into(),
            amount: value(5),
        },
    }
}

fn policy() -> PlacementPolicy {
    PlacementPolicy {
        max_observation_age: value(3),
        maximum_pressure: value(4),
        minimum_spool_reserve: value(2),
        disconnected_continuation_only: true,
        ownership_revision: Some(value(9)),
    }
}

fn offer(node: &str, slots: u64, revision: u64, observed_at: u64) -> NodeOffer {
    NodeOffer {
        node_id: NodeId::new(node).expect("fixture node ID"),
        boot_id: "round16-boot-a".into(),
        authority_generation: value(4),
        visible_scopes: Observation {
            value: vec!["round16-project".into()],
            observed_at: value(observed_at),
        },
        capabilities: Observation {
            value: vec!["rust".into()],
            observed_at: value(observed_at),
        },
        account_pool: Observation {
            value: "round16-pool".into(),
            observed_at: value(observed_at),
        },
        capacity: Observation {
            value: Known::Known {
                value: Capacity {
                    slots: value(slots),
                    budget_remaining: value(12),
                },
            },
            observed_at: value(observed_at),
        },
        current_pressure: Observation {
            value: Known::Known { value: value(1) },
            observed_at: value(observed_at),
        },
        spool_free: Observation {
            value: Known::Known { value: value(8) },
            observed_at: value(observed_at),
        },
        existing_active_roots: vec![],
        controller_connected: true,
        locality_refs: vec!["src".into()],
        continuation_permit: None,
        reconciliation_required: false,
        ownership_observed_at: Some(value(observed_at)),
        ownership_revision: Some(value(revision)),
    }
}

fn assert_no_authority(proposal: &admiralty_core::placement::PlacementProposal) {
    assert!(!proposal.execution_granted);
    assert!(!proposal.publication_granted);
    assert!(!proposal.authority_granted);
}

#[test]
fn codex_round16_p24_p29() {
    let task = request();
    let rules = policy();

    // Cutover starts with fresh, revision-matched topology and available capacity.
    let fresh_offer = offer("node-cutover", 2, 9, 50);
    let input = PlacementInput {
        request: task.clone(),
        offers: vec![fresh_offer.clone()],
        policy: rules.clone(),
        now: Some(value(50)),
        previous_tick: Some(value(49)),
    };
    let encoded = serde_json::to_vec(&input).expect("serialize stable cutover evidence");
    let decoded = decode_input(&encoded).expect("decode validates cutover evidence");
    assert_eq!(decoded, input);

    let cutover = propose_placement(&task, &[fresh_offer], &rules, 50)
        .expect("fresh capacity and topology yield a candidate");
    let replay = propose_placement(&task, &[offer("node-cutover", 2, 9, 50)], &rules, 50)
        .expect("same cutover evidence replays deterministically");
    assert_eq!(cutover, replay);
    assert_eq!(cutover.candidates.len(), 1);
    assert_no_authority(&cutover);

    // The next observation loses capacity telemetry: placement must stop closed.
    let mut unknown = offer("node-cutover", 2, 9, 51);
    unknown.capacity.value = Known::Unknown {
        reason: "capacity probe interrupted during cutover".into(),
    };
    let blocked = propose_placement(&task, &[unknown], &rules, 51)
        .expect("capacity uncertainty must become a safe rejection");
    assert!(blocked.candidates.is_empty());
    assert_eq!(blocked.rejected.len(), 1);
    assert_no_authority(&blocked);

    // Recovered capacity cannot bypass a topology revision mismatch or stale evidence.
    for unsafe_offer in [
        offer("node-old-topology", 2, 8, 52),
        offer("node-stale", 2, 9, 45),
    ] {
        let stopped = propose_placement(&task, &[unsafe_offer], &rules, 52)
            .expect("unsafe topology becomes a fail-closed rejection");
        assert!(stopped.candidates.is_empty());
        assert_eq!(stopped.rejected.len(), 1);
        assert_no_authority(&stopped);
    }

    // Invalid and rolled-back records are hard errors and never create proposals.
    assert_eq!(
        propose_placement_recorded(
            &task,
            &[offer("node-clock", 1, 9, 53)],
            &rules,
            Some(52),
            Some(53)
        ),
        Err(PlacementError::Clock)
    );
    let mut corrupted = serde_json::to_value(input).expect("serialize fixture input");
    corrupted["unverified_cutover_authority"] = serde_json::Value::Bool(true);
    let corrupted_bytes = serde_json::to_vec(&corrupted).expect("serialize corrupted fixture");
    assert_eq!(
        decode_input(&corrupted_bytes),
        Err(PlacementError::Invalid("placement JSON"))
    );
}
