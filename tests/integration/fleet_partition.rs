use admiralty_core::{
    ProjectId, RootId, TaskId,
    placement::{
        ActiveRoot, BudgetRequirement, Capacity, DecimalU64, IsolatedPermit, Known, NodeId,
        NodeOffer, Observation, PlacementError, PlacementInput, PlacementPolicy, PlacementRequest,
        decode_input, propose_placement,
    },
};

fn d(value: u64) -> DecimalU64 {
    DecimalU64::new(value)
}

fn request() -> PlacementRequest {
    PlacementRequest {
        project: ProjectId::new("project").expect("project fixture"),
        root: RootId::new("root").expect("root fixture"),
        task: TaskId::new("task").expect("task fixture"),
        contract_digest: format!("sha256:{}", "a".repeat(64)),
        authority_ref: "grant-1".into(),
        required_capabilities: vec!["rust".into()],
        locality_refs: vec!["src".into()],
        budget_requirement: BudgetRequirement {
            account_pool: "pool".into(),
            amount: d(5),
        },
    }
}

fn policy() -> PlacementPolicy {
    PlacementPolicy {
        max_observation_age: d(10),
        maximum_pressure: d(5),
        minimum_spool_reserve: d(2),
        disconnected_continuation_only: true,
        ownership_revision: Some(d(1)),
    }
}

fn offer() -> NodeOffer {
    NodeOffer {
        node_id: NodeId::new("node-a").expect("node fixture"),
        boot_id: "boot-1".into(),
        authority_generation: d(1),
        visible_scopes: Observation {
            value: vec!["project".into()],
            observed_at: d(10),
        },
        capabilities: Observation {
            value: vec!["rust".into()],
            observed_at: d(10),
        },
        account_pool: Observation {
            value: "pool".into(),
            observed_at: d(10),
        },
        capacity: Observation {
            value: Known::Known {
                value: Capacity {
                    slots: d(1),
                    budget_remaining: d(10),
                },
            },
            observed_at: d(10),
        },
        current_pressure: Observation {
            value: Known::Known { value: d(1) },
            observed_at: d(10),
        },
        spool_free: Observation {
            value: Known::Known { value: d(10) },
            observed_at: d(10),
        },
        existing_active_roots: vec![],
        controller_connected: true,
        locality_refs: vec!["src".into()],
        continuation_permit: None,
        reconciliation_required: false,
        ownership_observed_at: Some(d(10)),
        ownership_revision: Some(d(1)),
    }
}

#[test]
fn disconnected_node_cannot_start_ordinary_work_without_a_bound_continuation() {
    let mut node = offer();
    node.controller_connected = false;

    let proposal = propose_placement(&request(), &[node], &policy(), 10).expect("proposal");
    assert!(proposal.candidates.is_empty());
    assert!(!proposal.execution_granted);
    assert!(!proposal.publication_granted);
    assert!(!proposal.authority_granted);
}

#[test]
fn disconnected_node_can_only_propose_its_existing_permitted_continuation() {
    let request = request();
    let mut node = offer();
    node.controller_connected = false;
    node.existing_active_roots = vec![ActiveRoot {
        root: request.root.clone(),
        ordinary: true,
    }];
    node.continuation_permit = Some(IsolatedPermit {
        permit_id: "permit-1".into(),
        root: request.root.clone(),
        project: request.project.clone(),
        task: request.task.clone(),
        node_id: node.node_id.clone(),
        authority_ref: request.authority_ref.clone(),
        expires_at: d(20),
        immutable_digest: request.contract_digest.clone(),
        authority_generation: d(1),
        reboot_id: node.boot_id.clone(),
        remaining_budget: d(5),
        spool_reserve: d(3),
        minimum_spool_reserve: d(2),
    });

    let proposal = propose_placement(&request, &[node.clone()], &policy(), 10).expect("proposal");
    assert_eq!(proposal.candidates.len(), 1);
    assert_eq!(
        proposal.candidates[0].continuation_permit_id.as_deref(),
        Some("permit-1")
    );
    assert!(!proposal.execution_granted);
    assert!(!proposal.publication_granted);
    assert!(!proposal.authority_granted);

    node.boot_id = "rebooted".into();
    let proposal = propose_placement(&request, &[node], &policy(), 10).expect("safe rejection");
    assert!(proposal.candidates.is_empty());
}

#[test]
fn disconnected_continuation_with_spool_below_reserve_is_rejected() {
    let request = request();
    let mut node = offer();
    node.controller_connected = false;
    node.existing_active_roots = vec![ActiveRoot {
        root: request.root.clone(),
        ordinary: true,
    }];
    node.spool_free.value = Known::Known { value: d(1) };
    node.continuation_permit = Some(IsolatedPermit {
        permit_id: "permit-low-spool".into(),
        root: request.root.clone(),
        project: request.project.clone(),
        task: request.task.clone(),
        node_id: node.node_id.clone(),
        authority_ref: request.authority_ref.clone(),
        expires_at: d(20),
        immutable_digest: request.contract_digest.clone(),
        authority_generation: d(1),
        reboot_id: node.boot_id.clone(),
        remaining_budget: d(5),
        spool_reserve: d(3),
        minimum_spool_reserve: d(2),
    });

    let proposal = propose_placement(&request, &[node], &policy(), 10).expect("safe rejection");
    assert!(proposal.candidates.is_empty());
    assert!(!proposal.execution_granted);
    assert!(!proposal.publication_granted);
    assert!(!proposal.authority_granted);
    assert_eq!(proposal.rejected.len(), 1);
    assert_eq!(proposal.rejected[0].reason, "spool reserve insufficient");
}

#[test]
fn disconnected_continuation_expiring_at_decision_time_is_rejected() {
    let request = request();
    let mut node = offer();
    node.controller_connected = false;
    node.existing_active_roots = vec![ActiveRoot {
        root: request.root.clone(),
        ordinary: true,
    }];
    node.continuation_permit = Some(IsolatedPermit {
        permit_id: "permit-expiring-now".into(),
        root: request.root.clone(),
        project: request.project.clone(),
        task: request.task.clone(),
        node_id: node.node_id.clone(),
        authority_ref: request.authority_ref.clone(),
        expires_at: d(10),
        immutable_digest: request.contract_digest.clone(),
        authority_generation: d(1),
        reboot_id: node.boot_id.clone(),
        remaining_budget: d(5),
        spool_reserve: d(3),
        minimum_spool_reserve: d(2),
    });

    let proposal =
        propose_placement(&request, &[node.clone()], &policy(), 10).expect("safe rejection");
    assert!(proposal.candidates.is_empty());
    assert!(!proposal.execution_granted);
    assert!(!proposal.publication_granted);
    assert!(!proposal.authority_granted);
    assert_eq!(proposal.rejected.len(), 1);
    assert_eq!(proposal.rejected[0].reason, "continuation permit expired");
    assert_eq!(
        proposal,
        propose_placement(&request, &[node], &policy(), 10).expect("deterministic replay")
    );
}

#[test]
fn corrupted_placement_snapshot_with_unknown_field_is_rejected() {
    let input = PlacementInput {
        request: request(),
        offers: vec![offer()],
        policy: policy(),
        now: Some(d(10)),
        previous_tick: Some(d(9)),
    };
    let mut value = serde_json::to_value(input).expect("valid snapshot");
    value["unexpected_authority"] = serde_json::Value::Bool(true);

    let corrupted = serde_json::to_vec(&value).expect("corrupted snapshot JSON");
    assert_eq!(
        decode_input(&corrupted),
        Err(PlacementError::Invalid("placement JSON"))
    );
}
