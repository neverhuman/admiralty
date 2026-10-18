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
fn codex_round20_p04_p10_post_pr57_release_qualification() -> Result<(), Box<dyn Error>> {
    // P00: the bundle is pinned to a stable task contract and execution scope.
    let contract = task_fixture("round20-pr57-release-task");
    let contract_digest = content_digest(&contract)?;
    assert_eq!(content_digest(&contract)?, contract_digest);
    let scope = ReservationScope {
        project_id: contract.project_id.clone(),
        root_id: contract.root_id.clone(),
        task_id: contract.task_id.clone(),
        attempt_id: AttemptId::new("round20-attempt")?,
        owner: PrincipalId::new("round20-worker")?,
        incarnation: "round20-worker-boot".into(),
        contract_revision: contract.revision,
        contract_digest: contract_digest.clone(),
        authority_ref: contract.authority_ref.clone(),
        resource_claims_digest: content_digest(&contract.owned_paths)?,
        account_id: AccountId::new("round20-account")?,
        unit: CurrencyUnit::new("USD", 2)?,
    };

    // P01: fault and lease decisions replay from logical inputs alone.
    let rules = vec![FaultRule {
        point: "round20-receipt-audit".into(),
        occurrence: 2,
        effect: FaultEffect::Fail,
    }];
    let mut first_faults = FaultPlan::new(rules.clone())?;
    let mut replay_faults = FaultPlan::new(rules)?;
    for expected in [None, Some(FaultEffect::Fail), None] {
        assert_eq!(first_faults.hit("round20-receipt-audit")?, expected);
        assert_eq!(replay_faults.hit("round20-receipt-audit")?, expected);
    }
    let mut clock = FakeClock::new(100);
    let deadline = clock.deadline_after_ms(25)?;
    assert!(!deadline.is_due(clock.now_ms()));
    assert_eq!(clock.advance_to_ms(deadline.at_ms())?, 125);
    assert!(deadline.is_due(clock.now_ms()));

    // P04: the release envelope contains admitted source and excludes private
    // and traversal paths.
    let directory = tempfile::tempdir()?;
    let root = directory.path().join("workspace");
    fs::create_dir_all(root.join("src"))?;
    fs::create_dir_all(root.join(".private"))?;
    let workspace = WorkspaceSpec {
        node_id: NodeRef::new("round20-worker")?,
        root_id: scope.root_id.clone(),
        project_id: scope.project_id.clone(),
        workspace: fs::canonicalize(root)?,
        owned_paths: vec![PathBuf::from("src")],
        forbidden_paths: vec![PathBuf::from(".private")],
    }
    .validate()?;
    assert!(workspace.owns(std::path::Path::new("src/main.rs")));
    assert!(!workspace.owns(std::path::Path::new(".private/token")));
    assert!(!workspace.owns(std::path::Path::new("../other/command")));

    // P10: lease stop preserves liability until complete, scope-matched
    // reconciliation evidence settles usage. Exact event replay is idempotent.
    let mut ledger = ReservationLedger::new();
    ledger.limits.insert(
        "round20-account".into(),
        ScopeLimits {
            unit: CurrencyUnit::new("USD", 2)?,
            account: Some(MinorUnits::new(50)),
            project: Some(MinorUnits::new(50)),
            root: Some(MinorUnits::new(50)),
        },
    );
    ledger = reserve_budget(
        &ledger,
        "round20-reserve-event",
        "round20-reservation",
        scope.clone(),
        "round20-execution-envelope",
        MinorUnits::new(30),
    )?;
    let stop = ReservationEvent::RequestStop {
        event_id: "round20-lease-expiry".into(),
        reservation_id: "round20-reservation".into(),
        binding: ledger
            .reservations
            .get("round20-reservation")
            .ok_or("reservation missing")?
            .binding(),
        reason: StopReason::LeaseExpiry,
    };
    let expired = reduce_reservation(&ledger, stop.clone())?;
    assert_eq!(reduce_reservation(&expired, stop)?, expired);
    assert_eq!(
        expired.unresolved_liabilities.get(&scope),
        Some(&MinorUnits::new(30))
    );
    let evidence = FinalEvidence::SettledAndTerminated {
        evidence_ref: "round20-release-receipt".into(),
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
        "round20-settle-event",
        "round20-reservation",
        MinorUnits::new(18),
        Some(evidence),
    )?;
    assert_eq!(settled.settled.get(&scope), Some(&MinorUnits::new(18)));
    assert_eq!(
        settled.unresolved_liabilities.get(&scope),
        Some(&MinorUnits::new(0))
    );

    let bundle = serde_json::json!({
        "schema_version": 1,
        "contract_digest": contract_digest,
        "workspace_owned_paths": ["src"],
        "workspace_forbidden_paths": [".private"],
        "reservation_ledger": settled,
    });
    let bundle_bytes = serde_json::to_vec(&bundle)?;
    let replay_bundle = serde_json::json!({
        "schema_version": 1,
        "contract_digest": content_digest(&contract)?,
        "workspace_owned_paths": ["src"],
        "workspace_forbidden_paths": [".private"],
        "reservation_ledger": settled,
    });
    let replay_bytes = serde_json::to_vec(&replay_bundle)?;
    assert_eq!(bundle_bytes, replay_bytes);
    let decoded: serde_json::Value = serde_json::from_slice(&bundle_bytes)?;
    assert_eq!(decoded, replay_bundle);
    let decoded_ledger: ReservationLedger =
        serde_json::from_value(decoded["reservation_ledger"].clone())?;
    assert_eq!(decoded_ledger, settled);
    let mut unknown_field: serde_json::Value = serde_json::from_slice(&bundle_bytes)?;
    unknown_field["reservation_ledger"]["release_authorized"] = true.into();
    assert!(
        serde_json::from_value::<ReservationLedger>(unknown_field["reservation_ledger"].clone())
            .is_err()
    );
    Ok(())
}
