//! Round 10 Item 3 deterministic P04/P10 workspace gate audit.

use admiralty_core::{ProjectId, RootId};
use admiralty_runtime::workspace::registry::{NodeRef, WorkspaceSpec};
use std::{fs, path::PathBuf};

#[test]
fn p04_p10_round10_item3_workspace_gate_audit() {
    use admiralty_core::{AttemptId, PrincipalId, TaskId};
    use admiralty_runtime::reservations::{
        AccountId, CurrencyUnit, MinorUnits, ReservationEvent, ReservationLedger, ReservationScope,
        ScopeLimits, StopReason, reduce_reservation, reserve_budget,
    };

    // P04: a lane registration owns only its declared source tree. Private and
    // metadata paths remain excluded even when a broader source tree is owned.
    let temp = tempfile::tempdir().expect("isolated lane fixture");
    let lane = temp.path().join("lane");
    fs::create_dir_all(lane.join("src/private")).expect("private source fixture");
    fs::create_dir_all(lane.join(".git")).expect("git metadata fixture");
    fs::write(lane.join("src/main.rs"), b"lane source").expect("source fixture");
    let lane = fs::canonicalize(lane).expect("canonical lane root");
    let registration = WorkspaceSpec {
        node_id: NodeRef::new("round10-worker").expect("node ID"),
        root_id: RootId::new("round10-root").expect("root ID"),
        project_id: ProjectId::new("round10-project").expect("project ID"),
        workspace: lane,
        owned_paths: vec![PathBuf::from("src")],
        forbidden_paths: vec![PathBuf::from("src/private"), PathBuf::from(".git")],
    }
    .validate()
    .expect("scoped lane registration");
    assert!(registration.owns(std::path::Path::new("src/main.rs")));
    assert!(!registration.owns(std::path::Path::new("src/private/secret")));
    assert!(!registration.owns(std::path::Path::new(".git/config")));
    assert!(!registration.owns(std::path::Path::new("../other/src/main.rs")));

    // P10: requesting stop after lease expiry does not erase unobserved spend.
    let scope = ReservationScope {
        project_id: ProjectId::new("round10-project").expect("project ID"),
        root_id: RootId::new("round10-root").expect("root ID"),
        task_id: TaskId::new("round10-task").expect("task ID"),
        attempt_id: AttemptId::new("round10-attempt").expect("attempt ID"),
        owner: PrincipalId::new("round10-owner").expect("owner ID"),
        incarnation: "worker-boot-1".into(),
        contract_revision: 1,
        contract_digest: format!("sha256:{}", "d".repeat(64)),
        authority_ref: "grant-round10".into(),
        resource_claims_digest: format!("sha256:{}", "e".repeat(64)),
        account_id: AccountId::new("round10-account").expect("account ID"),
        unit: CurrencyUnit::new("USD", 2).expect("currency unit"),
    };
    let mut ledger = ReservationLedger::new();
    ledger.limits.insert(
        "round10-account".into(),
        ScopeLimits {
            unit: CurrencyUnit::new("USD", 2).expect("currency unit"),
            account: Some(MinorUnits::new(100)),
            project: Some(MinorUnits::new(100)),
            root: Some(MinorUnits::new(100)),
        },
    );
    let reserved = reserve_budget(
        &ledger,
        "reserve-event",
        "lane-reservation",
        scope.clone(),
        "lane-correlation",
        MinorUnits::new(40),
    )
    .expect("bounded reservation");
    let stopped = reduce_reservation(
        &reserved,
        ReservationEvent::RequestStop {
            event_id: "expiry-event".into(),
            reservation_id: "lane-reservation".into(),
            binding: reserved.reservations["lane-reservation"].binding(),
            reason: StopReason::LeaseExpiry,
        },
    )
    .expect("stop request records without settling");
    assert!(stopped.reservations["lane-reservation"].stop_requested);
    assert_eq!(
        stopped
            .unresolved_liabilities
            .get(&scope)
            .expect("retained liability")
            .get(),
        40,
        "stop is not evidence that the worker incurred no usage"
    );
    assert_eq!(stopped.revision, reserved.revision + 1);
    let replay = reduce_reservation(
        &reserved,
        ReservationEvent::RequestStop {
            event_id: "expiry-event".into(),
            reservation_id: "lane-reservation".into(),
            binding: reserved.reservations["lane-reservation"].binding(),
            reason: StopReason::LeaseExpiry,
        },
    )
    .expect("exact expiry replay is idempotent");
    assert_eq!(replay, stopped);
}
