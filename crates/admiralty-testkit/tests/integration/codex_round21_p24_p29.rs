#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Round 21 hermetic cross-packet release checks for scheduling and placement.

use admiralty_core::placement::{
    BudgetRequirement, Capacity, DecimalU64, Known, NodeId, NodeOffer, Observation,
    PlacementPolicy, PlacementRequest, propose_placement,
};
use admiralty_core::scheduler::{
    Knowledge, LeaseObservation, ScheduledTask, SchedulerSnapshot, SchedulingScope,
    TaskSchedulingState, propose_ready_tasks,
};
use admiralty_core::{
    AcceptanceCriterion, CheckSpec, ProjectId, RootId, TaskContract, TaskId, content_digest,
};

fn d(value: u64) -> DecimalU64 {
    DecimalU64::new(value)
}

fn scope() -> SchedulingScope {
    SchedulingScope {
        project_id: ProjectId::new("round21-project").expect("project fixture"),
        root_id: RootId::new("round21-root").expect("root fixture"),
        revision: 21,
    }
}

fn contract() -> TaskContract {
    let scope = scope();
    TaskContract {
        schema_version: 1,
        project_id: scope.project_id,
        root_id: scope.root_id,
        task_id: TaskId::new("round21-task").expect("task fixture"),
        revision: 4,
        title: "Round 21 placement candidate".into(),
        objective: "Verify schedulability and placement remain independent proposals".into(),
        non_goals: vec![],
        owned_paths: vec!["src".into()],
        forbidden_paths: vec![],
        read_first: vec![],
        dependencies: vec![],
        criteria: vec![AcceptanceCriterion {
            id: "check-passes".into(),
            text: "The bounded fixture check passes".into(),
            check_ids: vec!["check".into()],
        }],
        checks: vec![CheckSpec {
            id: "check".into(),
            program: "cargo".into(),
            args: vec!["check".into()],
            cwd: ".".into(),
            timeout_secs: 30,
        }],
        authority_ref: "round21-authority".into(),
        max_attempts: 1,
        simulated: true,
    }
}

fn scheduler_snapshot() -> SchedulerSnapshot {
    let contract = contract();
    let digest = content_digest(&contract).expect("stable contract digest");
    SchedulerSnapshot {
        schema_version: 1,
        scope: scope(),
        tasks: vec![ScheduledTask {
            observed_contract_revision: contract.revision,
            contract,
            contract_digest: digest,
            state: TaskSchedulingState::Pending,
            lease: LeaseObservation::Absent,
        }],
        available_quota: Knowledge::Known(1),
        available_capacity: Knowledge::Known(1),
    }
}

fn placement_request() -> PlacementRequest {
    let scope = scope();
    PlacementRequest {
        project: scope.project_id,
        root: scope.root_id,
        task: TaskId::new("round21-task").expect("task fixture"),
        contract_digest: content_digest(&contract()).expect("stable placement contract digest"),
        authority_ref: "round21-authority".into(),
        required_capabilities: vec!["rust".into()],
        locality_refs: vec!["src".into()],
        budget_requirement: BudgetRequirement {
            account_pool: "round21-pool".into(),
            amount: d(5),
        },
    }
}

fn policy() -> PlacementPolicy {
    PlacementPolicy {
        max_observation_age: d(4),
        maximum_pressure: d(4),
        minimum_spool_reserve: d(2),
        disconnected_continuation_only: true,
        ownership_revision: Some(d(21)),
    }
}

fn offer(tick: u64, capacity: Known<Capacity>, ownership_revision: u64) -> NodeOffer {
    NodeOffer {
        node_id: NodeId::new("round21-node").expect("node fixture"),
        boot_id: "round21-boot".into(),
        authority_generation: d(3),
        visible_scopes: Observation {
            value: vec!["round21-project".into()],
            observed_at: d(tick),
        },
        capabilities: Observation {
            value: vec!["rust".into()],
            observed_at: d(tick),
        },
        account_pool: Observation {
            value: "round21-pool".into(),
            observed_at: d(tick),
        },
        capacity: Observation {
            value: capacity,
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

#[test]
fn codex_round21_p24_p29_hermetic_cross_packet_release_qualification() {
    let scheduling = propose_ready_tasks(&scheduler_snapshot(), &scope())
        .expect("valid scheduler evidence yields a proposal");
    assert_eq!(scheduling.ready.len(), 1);
    assert!(!scheduling.execution_granted);

    let unknown_capacity = offer(
        210,
        Known::Unknown {
            reason: "capacity report unavailable".into(),
        },
        21,
    );
    let blocked = propose_placement(&placement_request(), &[unknown_capacity], &policy(), 210)
        .expect("placement capacity uncertainty fails closed");
    assert!(blocked.candidates.is_empty());
    assert_eq!(blocked.rejected.len(), 1);
    assert!(!blocked.execution_granted);
    assert!(!blocked.publication_granted);
    assert!(!blocked.authority_granted);

    // A ready scheduler proposal cannot bypass stale topology evidence.
    let stale_topology = propose_placement(
        &placement_request(),
        &[offer(
            211,
            Known::Known {
                value: Capacity {
                    slots: d(1),
                    budget_remaining: d(10),
                },
            },
            20,
        )],
        &policy(),
        211,
    )
    .expect("stale topology remains a safe rejection");
    assert!(stale_topology.candidates.is_empty());
    assert!(!stale_topology.execution_granted);

    // Fresh evidence allows a deterministic candidate proposal, still with no grant.
    let fresh_offer = offer(
        212,
        Known::Known {
            value: Capacity {
                slots: d(1),
                budget_remaining: d(10),
            },
        },
        21,
    );
    let placed = propose_placement(&placement_request(), &[fresh_offer.clone()], &policy(), 212)
        .expect("fresh placement evidence yields a candidate");
    let replay = propose_placement(&placement_request(), &[fresh_offer], &policy(), 212)
        .expect("hermetic replay is deterministic");
    assert_eq!(placed, replay);
    assert_eq!(placed.candidates.len(), 1);
    assert!(!placed.execution_granted);
    assert!(!placed.publication_granted);
    assert!(!placed.authority_granted);
}
