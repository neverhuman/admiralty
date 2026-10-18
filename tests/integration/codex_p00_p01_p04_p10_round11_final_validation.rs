//! Round 11 Item 4 end-to-end deterministic invariants across P00/P01/P04/P10.

use admiralty_core::{DomainEvent, ProjectId, Projection, RootId, SchedulingState, reduce};
use admiralty_runtime::{
    reservations::{
        AccountId, CurrencyUnit, MinorUnits, ReservationEvent, ReservationLedger, ReservationScope,
        ScopeLimits, StopReason, reduce_reservation, reserve_budget,
    },
    workspace::registry::{NodeRef, WorkspaceSpec},
};
use admiralty_testkit::{
    clock::FakeClock,
    faults::{FaultEffect, FaultPlan, FaultRule},
    task_fixture,
};
use std::{fs, path::PathBuf};

#[test]
fn p00_p01_p04_p10_round11_item4_final_validation_gate() {
    use admiralty_core::{AttemptId, PrincipalId, TaskId};

    // P00: submitting a task creates one ready projection; a duplicate identity
    // with altered scope is rejected without changing that projection.
    let contract = task_fixture("round11-final-gate");
    let submitted = reduce(
        &Projection::default(),
        &DomainEvent::TaskSubmitted {
            contract: contract.clone(),
        },
    )
    .expect("valid deterministic task submission");
    assert_eq!(
        submitted.tasks[&contract.task_id].state,
        SchedulingState::Ready
    );
    let before = submitted.clone();
    let mut substituted = contract.clone();
    substituted.owned_paths = vec!["src/foreign.rs".into()];
    assert!(
        reduce(
            &submitted,
            &DomainEvent::TaskSubmitted {
                contract: substituted,
            },
        )
        .is_err()
    );
    assert_eq!(submitted, before);

    // P01: fault scheduling and fake time produce a stable bounded retry point.
    let mut plan = FaultPlan::new(vec![FaultRule {
        point: "lane-retry".into(),
        occurrence: 1,
        effect: FaultEffect::DelayMs(7),
    }])
    .expect("bounded deterministic fault plan");
    let mut clock = FakeClock::new(100);
    assert_eq!(
        plan.hit("lane-retry").expect("first scripted event"),
        Some(FaultEffect::DelayMs(7))
    );
    clock.advance_ms(6).expect("advance within deadline");
    assert_eq!(clock.now_ms(), 106);
    clock.advance_ms(1).expect("reach exact deadline");
    assert_eq!(clock.now_ms(), 107);
    assert_eq!(plan.hits("lane-retry").expect("stable occurrence count"), 1);

    // P04: the registered lane owns declared source only; private, metadata,
    // and traversal paths do not pass the workspace gate.
    let temp = tempfile::tempdir().expect("isolated workspace fixture");
    let lane = temp.path().join("lane");
    fs::create_dir_all(lane.join("src/private")).expect("workspace source tree");
    fs::create_dir_all(lane.join(".git")).expect("metadata fixture");
    fs::write(lane.join("src/main.rs"), b"owned").expect("owned source");
    let lane = fs::canonicalize(lane).expect("canonical workspace");
    let workspace = WorkspaceSpec {
        node_id: NodeRef::new("round11-node").expect("node ID"),
        root_id: RootId::new("round11-root").expect("root ID"),
        project_id: ProjectId::new("round11-project").expect("project ID"),
        workspace: lane,
        owned_paths: vec![PathBuf::from("src")],
        forbidden_paths: vec![PathBuf::from("src/private"), PathBuf::from(".git")],
    }
    .validate()
    .expect("workspace admission");
    assert!(workspace.owns(std::path::Path::new("src/main.rs")));
    assert!(!workspace.owns(std::path::Path::new("src/private/secret")));
    assert!(!workspace.owns(std::path::Path::new(".git/config")));
    assert!(!workspace.owns(std::path::Path::new("../outside/secret")));

    // P10: expiry stops work but retains the reservation as unresolved exposure;
    // an exact event replay is idempotent and cannot free the liability.
    let scope = ReservationScope {
        project_id: ProjectId::new("round11-project").expect("project ID"),
        root_id: RootId::new("round11-root").expect("root ID"),
        task_id: TaskId::new("round11-task").expect("task ID"),
        attempt_id: AttemptId::new("round11-attempt").expect("attempt ID"),
        owner: PrincipalId::new("round11-owner").expect("owner ID"),
        incarnation: "worker-incarnation-1".into(),
        contract_revision: 1,
        contract_digest: format!("sha256:{}", "a".repeat(64)),
        authority_ref: "grant-round11".into(),
        resource_claims_digest: format!("sha256:{}", "b".repeat(64)),
        account_id: AccountId::new("round11-account").expect("account ID"),
        unit: CurrencyUnit::new("USD", 2).expect("currency unit"),
    };
    let mut ledger = ReservationLedger::new();
    ledger.limits.insert(
        "round11-account".into(),
        ScopeLimits {
            unit: CurrencyUnit::new("USD", 2).expect("currency unit"),
            account: Some(MinorUnits::new(100)),
            project: Some(MinorUnits::new(100)),
            root: Some(MinorUnits::new(100)),
        },
    );
    let reserved = reserve_budget(
        &ledger,
        "round11-reserve-event",
        "round11-reservation",
        scope.clone(),
        "round11-correlation",
        MinorUnits::new(40),
    )
    .expect("reservation within scoped limits");
    let stop = ReservationEvent::RequestStop {
        event_id: "round11-expiry-event".into(),
        reservation_id: "round11-reservation".into(),
        binding: reserved.reservations["round11-reservation"].binding(),
        reason: StopReason::LeaseExpiry,
    };
    let stopped = reduce_reservation(&reserved, stop.clone()).expect("expiry request");
    assert!(stopped.reservations["round11-reservation"].stop_requested);
    assert_eq!(
        stopped.unresolved_liabilities[&scope].get(),
        40,
        "lease expiry is not proof of zero usage"
    );
    assert_eq!(
        reduce_reservation(&reserved, stop).expect("exact replay"),
        stopped
    );
}
