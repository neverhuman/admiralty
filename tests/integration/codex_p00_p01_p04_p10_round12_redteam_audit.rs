//! Round 12 red-team receipt and fail-closed audit for P00/P01/P04/P10.

use admiralty_core::{
    AttemptId, DomainEvent, PrincipalId, ProjectId, Projection, RootId, TaskId, content_digest,
    reduce,
};
use admiralty_runtime::{
    reservations::{
        AccountId, CurrencyUnit, FinalEvidence, MinorUnits, ReconciliationEvidence,
        ReservationEvent, ReservationLedger, ReservationScope, ScopeLimits, StopReason,
        reduce_reservation, reserve_budget, settle_usage,
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
fn p00_p01_p04_p10_round12_item5_redteam_audit_gate() {
    // P00 receipt integrity: a task digest binds the full contract, and replaying
    // a task ID with altered scope is rejected without changing the projection.
    let contract = task_fixture("round12-redteam");
    let digest = content_digest(&contract).expect("canonical task digest");
    let projection = reduce(
        &Projection::default(),
        &DomainEvent::TaskSubmitted {
            contract: contract.clone(),
        },
    )
    .expect("valid task submission");
    let mut forged_contract = contract.clone();
    forged_contract.owned_paths.push("src/escape.rs".into());
    assert_ne!(
        digest,
        content_digest(&forged_contract).expect("forged digest differs")
    );
    let before = projection.clone();
    assert!(
        reduce(
            &projection,
            &DomainEvent::TaskSubmitted {
                contract: forged_contract,
            },
        )
        .is_err()
    );
    assert_eq!(projection, before);

    // P01 fault receipt integrity: malformed scripts fail closed; identical
    // scripts replay the same fault without depending on wall-clock time.
    assert!(
        FaultPlan::new(vec![FaultRule {
            point: "bad-occurrence".into(),
            occurrence: 0,
            effect: FaultEffect::Drop,
        }])
        .is_err()
    );
    let rules = vec![FaultRule {
        point: "receipt-replay".into(),
        occurrence: 1,
        effect: FaultEffect::DelayMs(9),
    }];
    let mut first = FaultPlan::new(rules.clone()).expect("valid script");
    let mut replay = FaultPlan::new(rules).expect("identical replay script");
    assert_eq!(
        first.hit("receipt-replay").expect("first hit"),
        Some(FaultEffect::DelayMs(9))
    );
    assert_eq!(
        replay.hit("receipt-replay").expect("replay hit"),
        Some(FaultEffect::DelayMs(9))
    );
    let mut clock = FakeClock::new(41);
    clock.advance_ms(9).expect("deterministic bounded clock");
    assert_eq!(clock.now_ms(), 50);

    // P04 workspace gate: reject the entire registration when an owned path
    // crosses a symlink, and keep forbidden metadata outside owned scope.
    let temp = tempfile::tempdir().expect("private fixture root");
    let lane = temp.path().join("lane");
    let outside = temp.path().join("outside");
    fs::create_dir_all(lane.join("src")).expect("lane source");
    fs::create_dir_all(&outside).expect("outside source");
    fs::create_dir_all(lane.join(".git")).expect("metadata fixture");
    fs::write(outside.join("secret"), b"sentinel").expect("outside sentinel");
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        symlink(&outside, lane.join("src/escape")).expect("symlink attack fixture");
    }
    let lane = fs::canonicalize(lane).expect("canonical lane");
    let workspace = WorkspaceSpec {
        node_id: NodeRef::new("round12-node").expect("node ID"),
        root_id: RootId::new("round12-root").expect("root ID"),
        project_id: ProjectId::new("round12-project").expect("project ID"),
        workspace: lane.clone(),
        owned_paths: vec![PathBuf::from("src")],
        forbidden_paths: vec![PathBuf::from(".git")],
    };
    let validated = workspace.validate().expect("workspace registration");
    #[cfg(unix)]
    assert!(!validated.owns(std::path::Path::new("src/escape/secret")));
    assert!(!validated.owns(std::path::Path::new(".git/config")));
    assert!(!validated.owns(std::path::Path::new("../outside/secret")));
    assert_eq!(
        fs::read(outside.join("secret")).expect("sentinel unchanged"),
        b"sentinel"
    );

    // P10 receipt audit: unknown ledger fields and forged reconciliation facts
    // cannot erase the held liability after stop/expiry.
    let scope = ReservationScope {
        project_id: ProjectId::new("round12-project").expect("project ID"),
        root_id: RootId::new("round12-root").expect("root ID"),
        task_id: TaskId::new("round12-task").expect("task ID"),
        attempt_id: AttemptId::new("round12-attempt").expect("attempt ID"),
        owner: PrincipalId::new("round12-owner").expect("owner ID"),
        incarnation: "worker-1".into(),
        contract_revision: 1,
        contract_digest: format!("sha256:{}", "c".repeat(64)),
        authority_ref: "grant-round12".into(),
        resource_claims_digest: format!("sha256:{}", "d".repeat(64)),
        account_id: AccountId::new("round12-account").expect("account ID"),
        unit: CurrencyUnit::new("USD", 2).expect("currency"),
    };
    let mut ledger = ReservationLedger::new();
    ledger.limits.insert(
        "round12-account".into(),
        ScopeLimits {
            unit: CurrencyUnit::new("USD", 2).expect("currency"),
            account: Some(MinorUnits::new(100)),
            project: Some(MinorUnits::new(100)),
            root: Some(MinorUnits::new(100)),
        },
    );
    let reserved = reserve_budget(
        &ledger,
        "reserve-12",
        "reservation-12",
        scope.clone(),
        "corr-12",
        MinorUnits::new(60),
    )
    .expect("bounded reservation");
    let stopped = reduce_reservation(
        &reserved,
        ReservationEvent::RequestStop {
            event_id: "stop-12".into(),
            reservation_id: "reservation-12".into(),
            binding: reserved.reservations["reservation-12"].binding(),
            reason: StopReason::LeaseExpiry,
        },
    )
    .expect("stop requested");
    let tampered = serde_json::to_value(&stopped).expect("serialize ledger");
    let mut forged = tampered.clone();
    forged["receipt_authorized"] = serde_json::Value::Bool(true);
    assert!(serde_json::from_value::<ReservationLedger>(forged).is_err());

    let forged_reconciliation = FinalEvidence::SettledAndTerminated {
        evidence_ref: "untrusted-receipt".into(),
        reconciliation: ReconciliationEvidence {
            scope_digest: "sha256:forged".into(),
            resource_claims_digest: "sha256:forged".into(),
            usage_complete: Some(true),
            effects_reconciled: Some(false),
            resource_claims_reconciled: Some(true),
        },
    };
    assert!(
        settle_usage(
            &stopped,
            "forged-settlement",
            "reservation-12",
            MinorUnits::new(0),
            Some(forged_reconciliation),
        )
        .is_err()
    );
    assert_eq!(stopped.unresolved_liabilities[&scope].get(), 60);
}
