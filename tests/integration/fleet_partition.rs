use admiralty_core::{
    ProjectId, RootId, TaskId,
    placement::{
        ActiveRoot, BudgetRequirement, Capacity, DecimalU64, IsolatedPermit, Known, NodeId,
        NodeOffer, Observation, PlacementError, PlacementInput, PlacementPolicy, PlacementRequest,
        decode_input, propose_placement,
    },
};
use admiralty_testkit::clock::FakeClock;

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
    assert_eq!(
        (
            &proposal.project,
            &proposal.root,
            &proposal.task,
            &proposal.contract_digest,
            &proposal.authority_ref,
        ),
        (
            &request.project,
            &request.root,
            &request.task,
            &request.contract_digest,
            &request.authority_ref,
        ),
        "placement must preserve the exact task and authority identity",
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

#[test]
fn fake_clock_skew_makes_old_ownership_snapshot_ineligible() {
    let mut clock = FakeClock::new(10);
    clock.advance_ms(11).expect("bounded simulated clock skew");

    let proposal = propose_placement(&request(), &[offer()], &policy(), clock.now_ms())
        .expect("stale evidence is a safe rejection");
    assert!(proposal.candidates.is_empty());
    assert_eq!(proposal.rejected.len(), 1);
    assert_eq!(
        proposal.rejected[0].reason,
        "topology ownership unknown, stale, or unreconciled"
    );
    assert!(!proposal.execution_granted);
    assert!(!proposal.publication_granted);
    assert!(!proposal.authority_granted);
}

fn isolated_owner() -> NodeOffer {
    let request = request();
    let mut node = offer();
    node.controller_connected = false;
    node.existing_active_roots = vec![ActiveRoot {
        root: request.root.clone(),
        ordinary: true,
    }];
    node.continuation_permit = Some(IsolatedPermit {
        permit_id: "partition-permit".into(),
        root: request.root,
        project: request.project,
        task: request.task,
        node_id: node.node_id.clone(),
        authority_ref: request.authority_ref,
        expires_at: d(20),
        immutable_digest: request.contract_digest,
        authority_generation: d(1),
        reboot_id: node.boot_id.clone(),
        remaining_budget: d(5),
        spool_reserve: d(8),
        minimum_spool_reserve: d(4),
    });
    node
}

#[test]
fn continuation_requires_current_free_spool_to_cover_its_permit_reserve() {
    for free in [2, 4, 7, 8, 9] {
        let mut node = isolated_owner();
        node.spool_free.value = Known::Known { value: d(free) };
        let proposal = propose_placement(&request(), &[node], &policy(), 10).expect("proposal");
        assert_eq!(proposal.candidates.len(), usize::from(free >= 8));
        if free < 8 {
            assert_eq!(
                proposal.rejected[0].reason,
                "continuation spool reserve depleted"
            );
        }
        assert!(!proposal.execution_granted);
        assert!(!proposal.publication_granted);
        assert!(!proposal.authority_granted);
    }
}

#[test]
fn four_uuid_nodes_preserve_root_ownership_across_partition_reboot_and_reconnect() {
    let mut owner = isolated_owner();
    owner.node_id = NodeId::new(uuid::Uuid::new_v4().to_string()).expect("UUID node");
    owner.continuation_permit.as_mut().expect("permit").node_id = owner.node_id.clone();
    let mut topology = vec![owner];
    for _ in 0..3 {
        let mut node = offer();
        node.node_id = NodeId::new(uuid::Uuid::new_v4().to_string()).expect("UUID node");
        topology.push(node);
    }
    let partitioned = propose_placement(&request(), &topology, &policy(), 10).expect("partition");
    assert_eq!(partitioned.candidates.len(), 1);
    assert_eq!(partitioned.candidates[0].node_id, topology[0].node_id);
    topology.reverse();
    assert_eq!(
        partitioned,
        propose_placement(&request(), &topology, &policy(), 10).expect("replay")
    );
    topology.reverse();

    // A reboot invalidates the old permit without releasing the observed root.
    topology[0].boot_id = "new-boot".into();
    let rebooted = propose_placement(&request(), &topology, &policy(), 10).expect("reboot");
    assert!(rebooted.candidates.is_empty());
    assert_eq!(rebooted.rejected.len(), 4);

    // Reconnect alone is not termination evidence or permission to move work.
    topology[0].controller_connected = true;
    topology[0].reconciliation_required = true;
    let reconnect = propose_placement(&request(), &topology, &policy(), 10).expect("reconnect");
    assert!(reconnect.candidates.is_empty());
    assert!(
        reconnect
            .rejected
            .iter()
            .all(|r| r.reason.contains("unreconciled"))
    );
    topology[0].reconciliation_required = false;
    let still_owned =
        propose_placement(&request(), &topology, &policy(), 10).expect("retained root");
    assert!(still_owned.candidates.is_empty());
    assert!(
        still_owned
            .rejected
            .iter()
            .all(|r| r.reason == "root already active in topology")
    );

    // An observation from a future clock cannot authorize any topology member.
    topology[0].ownership_observed_at = Some(d(11));
    assert!(
        propose_placement(&request(), &topology, &policy(), 10)
            .expect("clock skew")
            .candidates
            .is_empty()
    );
    assert!(!reconnect.execution_granted);
    assert!(!reconnect.publication_granted);
    assert!(!reconnect.authority_granted);
}
