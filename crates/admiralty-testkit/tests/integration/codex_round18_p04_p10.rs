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
fn codex_round18_p04_p10_lease_recovery_deterministic_receipt_audit() -> Result<(), Box<dyn Error>>
{
    // P00: receipt scope and contract identity are deterministic across replay.
    let contract = task_fixture("round18-expired-worker");
    let contract_digest = content_digest(&contract)?;
    assert_eq!(content_digest(&contract)?, contract_digest);
    let scope = ReservationScope {
        project_id: contract.project_id.clone(),
        root_id: contract.root_id.clone(),
        task_id: contract.task_id.clone(),
        attempt_id: AttemptId::new("round18-attempt")?,
        owner: PrincipalId::new("round18-owner")?,
        incarnation: "round18-worker-boot".into(),
        contract_revision: contract.revision,
        contract_digest,
        authority_ref: contract.authority_ref.clone(),
        resource_claims_digest: content_digest(&contract.owned_paths)?,
        account_id: AccountId::new("round18-account")?,
        unit: CurrencyUnit::new("USD", 2)?,
    };

    // P01: audit interruptions are deterministic and do not advance logical
    // time until the explicit lease-expiry transition is reached.
    let rules = vec![
        FaultRule {
            point: "receipt-reconciliation".into(),
            occurrence: 1,
            effect: FaultEffect::DelayMs(6),
        },
        FaultRule {
            point: "receipt-reconciliation".into(),
            occurrence: 2,
            effect: FaultEffect::Fail,
        },
    ];
    let mut first = FaultPlan::new(rules.clone())?;
    let mut replay = FaultPlan::new(rules)?;
    for expected in [Some(FaultEffect::DelayMs(6)), Some(FaultEffect::Fail), None] {
        assert_eq!(first.hit("receipt-reconciliation")?, expected);
        assert_eq!(replay.hit("receipt-reconciliation")?, expected);
    }
    let mut clock = FakeClock::new(40);
    let lease_deadline = clock.deadline_after_ms(10)?;
    assert!(!lease_deadline.is_due(clock.now_ms()));
    assert_eq!(clock.advance_to_ms(lease_deadline.at_ms())?, 50);
    assert!(lease_deadline.is_due(clock.now_ms()));

    // P04: recovery retains the same workspace custody boundary after expiry.
    let directory = tempfile::tempdir()?;
    let root = directory.path().join("worker");
    fs::create_dir_all(root.join("src"))?;
    fs::create_dir_all(root.join(".git"))?;
    fs::write(root.join("src/main.rs"), b"lease recovery source")?;
    let workspace = WorkspaceSpec {
        node_id: NodeRef::new("round18-worker")?,
        root_id: scope.root_id.clone(),
        project_id: scope.project_id.clone(),
        workspace: fs::canonicalize(root)?,
        owned_paths: vec![PathBuf::from("src")],
        forbidden_paths: vec![PathBuf::from(".git")],
    }
    .validate()?;
    assert!(workspace.owns(std::path::Path::new("src/main.rs")));
    assert!(!workspace.owns(std::path::Path::new(".git/config")));
    assert!(!workspace.owns(std::path::Path::new("../outside/secret")));

    // P10: expiry requests stop but does not erase the reserved amount. A
    // forged receipt is rejected; complete evidence settles measured usage and
    // releases only the remaining liability. The serialized receipt replays
    // byte-for-byte and unknown fields fail closed.
    let mut ledger = ReservationLedger::new();
    ledger.limits.insert(
        "round18-account".into(),
        ScopeLimits {
            unit: CurrencyUnit::new("USD", 2)?,
            account: Some(MinorUnits::new(100)),
            project: Some(MinorUnits::new(100)),
            root: Some(MinorUnits::new(100)),
        },
    );
    ledger = reserve_budget(
        &ledger,
        "round18-reserve-event",
        "round18-reservation",
        scope.clone(),
        "round18-execution-envelope",
        MinorUnits::new(40),
    )?;
    let stop_event = ReservationEvent::RequestStop {
        event_id: "round18-expiry-event".into(),
        reservation_id: "round18-reservation".into(),
        binding: ledger
            .reservations
            .get("round18-reservation")
            .ok_or("reservation missing before expiry")?
            .binding(),
        reason: StopReason::LeaseExpiry,
    };
    let expired = reduce_reservation(&ledger, stop_event.clone())?;
    assert_eq!(reduce_reservation(&expired, stop_event)?, expired);
    assert_eq!(
        expired.unresolved_liabilities.get(&scope),
        Some(&MinorUnits::new(40))
    );

    let forged = FinalEvidence::SettledAndTerminated {
        evidence_ref: "round18-forged-receipt".into(),
        reconciliation: ReconciliationEvidence {
            scope_digest: "sha256:wrong".into(),
            resource_claims_digest: scope.resource_claims_digest.clone(),
            usage_complete: Some(true),
            effects_reconciled: Some(false),
            resource_claims_reconciled: Some(true),
        },
    };
    assert!(
        settle_usage(
            &expired,
            "round18-forged-settle-event",
            "round18-reservation",
            MinorUnits::new(0),
            Some(forged),
        )
        .is_err()
    );
    assert_eq!(
        expired.unresolved_liabilities.get(&scope),
        Some(&MinorUnits::new(40))
    );

    let evidence = FinalEvidence::SettledAndTerminated {
        evidence_ref: "round18-verified-receipt".into(),
        reconciliation: ReconciliationEvidence {
            scope_digest: content_digest(&scope)?,
            resource_claims_digest: scope.resource_claims_digest.clone(),
            usage_complete: Some(true),
            effects_reconciled: Some(true),
            resource_claims_reconciled: Some(true),
        },
    };
    let recovered = settle_usage(
        &expired,
        "round18-settle-event",
        "round18-reservation",
        MinorUnits::new(12),
        Some(evidence),
    )?;
    assert_eq!(recovered.settled.get(&scope), Some(&MinorUnits::new(12)));
    assert_eq!(
        recovered.unresolved_liabilities.get(&scope),
        Some(&MinorUnits::new(0))
    );

    let receipt_bytes = serde_json::to_vec(&recovered)?;
    let replay_bytes = serde_json::to_vec(&recovered)?;
    assert_eq!(receipt_bytes, replay_bytes);
    assert_eq!(
        serde_json::from_slice::<ReservationLedger>(&receipt_bytes)?,
        recovered
    );
    assert_eq!(content_digest(&recovered)?, content_digest(&recovered)?);
    let mut tampered: serde_json::Value = serde_json::from_slice(&receipt_bytes)?;
    tampered["receipt_authorized"] = serde_json::Value::Bool(true);
    assert!(serde_json::from_value::<ReservationLedger>(tampered).is_err());

    // The release bundle binds the contract, admitted workspace boundary, and
    // settled ledger under one explicit schema. Rebuilding it yields the same
    // digest, and decoding preserves both wrapper and ledger schema parity.
    let bundled_contract_digest = content_digest(&contract)?;
    let release_bundle = || {
        serde_json::json!({
            "schema_version": 1,
            "contract_digest": bundled_contract_digest,
            "workspace_owned_paths": ["src"],
            "workspace_forbidden_paths": [".git"],
            "reservation_ledger": recovered,
        })
    };
    let bundle = release_bundle();
    let bundle_bytes = serde_json::to_vec(&bundle)?;
    let bundle_digest = content_digest(&bundle)?;
    let replayed_bundle = release_bundle();
    assert_eq!(content_digest(&replayed_bundle)?, bundle_digest);
    let decoded_bundle: serde_json::Value = serde_json::from_slice(&bundle_bytes)?;
    assert_eq!(decoded_bundle, replayed_bundle);
    assert_eq!(decoded_bundle["schema_version"], 1);
    assert_eq!(
        decoded_bundle["reservation_ledger"]
            .as_object()
            .map(|ledger| ledger.len()),
        Some(6),
        "release ledger schema has exactly the six declared fields"
    );
    let decoded_ledger: ReservationLedger =
        serde_json::from_value(decoded_bundle["reservation_ledger"].clone())?;
    assert_eq!(decoded_ledger, recovered);
    Ok(())
}
