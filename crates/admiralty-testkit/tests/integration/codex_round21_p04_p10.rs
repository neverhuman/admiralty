#![allow(clippy::unwrap_used, clippy::expect_used)]

use admiralty_core::{AttemptId, PrincipalId, content_digest};
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
use std::{error::Error, fs, path::PathBuf};

#[test]
fn codex_round21_p04_p10_hermetic_ci_cross_packet_release() -> Result<(), Box<dyn Error>> {
    // P00: immutable task and execution scope identities replay deterministically.
    let contract = task_fixture("round21-hermetic-release");
    let contract_digest = content_digest(&contract)?;
    assert_eq!(content_digest(&contract)?, contract_digest);
    let scope = ReservationScope {
        project_id: contract.project_id.clone(),
        root_id: contract.root_id.clone(),
        task_id: contract.task_id.clone(),
        attempt_id: AttemptId::new("round21-attempt")?,
        owner: PrincipalId::new("round21-worker")?,
        incarnation: "round21-incarnation".into(),
        contract_revision: contract.revision,
        contract_digest: contract_digest.clone(),
        authority_ref: contract.authority_ref.clone(),
        resource_claims_digest: content_digest(&contract.owned_paths)?,
        account_id: AccountId::new("round21-account")?,
        unit: CurrencyUnit::new("USD", 2)?,
    };

    // P01: fault outcomes and lease boundaries depend only on recorded inputs.
    let rules = vec![
        FaultRule {
            point: "release-audit".into(),
            occurrence: 1,
            effect: FaultEffect::DelayMs(9),
        },
        FaultRule {
            point: "release-audit".into(),
            occurrence: 3,
            effect: FaultEffect::Fail,
        },
    ];
    let mut first_plan = FaultPlan::new(rules.clone())?;
    let mut replay_plan = FaultPlan::new(rules)?;
    for expected in [
        Some(FaultEffect::DelayMs(9)),
        None,
        Some(FaultEffect::Fail),
        None,
    ] {
        assert_eq!(first_plan.hit("release-audit")?, expected);
        assert_eq!(replay_plan.hit("release-audit")?, expected);
    }
    let mut clock = FakeClock::new(250);
    let lease = clock.deadline_after_ms(20)?;
    assert!(!lease.is_due(clock.now_ms()));
    assert_eq!(clock.advance_to_ms(lease.at_ms())?, 270);
    assert!(lease.is_due(clock.now_ms()));

    // P04: temporary workspaces define admitted paths without host-specific
    // checkout assumptions, and private or parent paths remain excluded.
    let fixture = tempfile::tempdir()?;
    let workspace_root = fixture.path().join("workspace");
    fs::create_dir_all(workspace_root.join("src"))?;
    fs::create_dir_all(workspace_root.join("private"))?;
    let workspace = WorkspaceSpec {
        node_id: NodeRef::new("round21-worker")?,
        root_id: scope.root_id.clone(),
        project_id: scope.project_id.clone(),
        workspace: fs::canonicalize(workspace_root)?,
        owned_paths: vec![PathBuf::from("src")],
        forbidden_paths: vec![PathBuf::from("private")],
    }
    .validate()?;
    assert!(workspace.owns(std::path::Path::new("src/main.rs")));
    assert!(!workspace.owns(std::path::Path::new("private/token")));
    assert!(!workspace.owns(std::path::Path::new("../sibling/command")));

    // P10: expired work retains exposure until complete matching evidence is
    // supplied; receipt bytes and typed schema replay identically.
    let mut ledger = ReservationLedger::new();
    ledger.limits.insert(
        "round21-account".into(),
        ScopeLimits {
            unit: CurrencyUnit::new("USD", 2)?,
            account: Some(MinorUnits::new(75)),
            project: Some(MinorUnits::new(75)),
            root: Some(MinorUnits::new(75)),
        },
    );
    ledger = reserve_budget(
        &ledger,
        "round21-reserve",
        "round21-reservation",
        scope.clone(),
        "round21-envelope",
        MinorUnits::new(50),
    )?;
    let stop = ReservationEvent::RequestStop {
        event_id: "round21-expiry".into(),
        reservation_id: "round21-reservation".into(),
        binding: ledger
            .reservations
            .get("round21-reservation")
            .ok_or("missing reservation")?
            .binding(),
        reason: StopReason::LeaseExpiry,
    };
    let expired = reduce_reservation(&ledger, stop.clone())?;
    assert_eq!(reduce_reservation(&expired, stop)?, expired);
    assert_eq!(
        expired.unresolved_liabilities.get(&scope),
        Some(&MinorUnits::new(50))
    );
    let evidence = FinalEvidence::SettledAndTerminated {
        evidence_ref: "round21-verified-receipt".into(),
        reconciliation: ReconciliationEvidence {
            scope_digest: content_digest(&scope)?,
            resource_claims_digest: scope.resource_claims_digest.clone(),
            usage_complete: Some(true),
            effects_reconciled: Some(true),
            resource_claims_reconciled: Some(true),
        },
    };
    let settled = settle_usage(
        &expired,
        "round21-settle",
        "round21-reservation",
        MinorUnits::new(32),
        Some(evidence),
    )?;
    assert_eq!(settled.settled.get(&scope), Some(&MinorUnits::new(32)));
    assert_eq!(
        settled.unresolved_liabilities.get(&scope),
        Some(&MinorUnits::new(0))
    );

    let bundle = serde_json::json!({
        "schema_version": 1,
        "contract_digest": contract_digest,
        "reservation_ledger": settled,
        "workspace_owned_paths": ["src"],
        "workspace_forbidden_paths": ["private"],
    });
    let first_bytes = serde_json::to_vec(&bundle)?;
    let replay_bytes = serde_json::to_vec(&bundle)?;
    assert_eq!(first_bytes, replay_bytes);
    let decoded: serde_json::Value = serde_json::from_slice(&first_bytes)?;
    assert_eq!(decoded["schema_version"], 1);
    let decoded_ledger: ReservationLedger =
        serde_json::from_value(decoded["reservation_ledger"].clone())?;
    assert_eq!(decoded_ledger, settled);
    let mut unknown_schema: serde_json::Value = serde_json::from_slice(&first_bytes)?;
    unknown_schema["reservation_ledger"]["unreviewed_authority"] = true.into();
    assert!(
        serde_json::from_value::<ReservationLedger>(unknown_schema["reservation_ledger"].clone())
            .is_err()
    );
    Ok(())
}
