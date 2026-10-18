#![allow(clippy::unwrap_used, clippy::expect_used)]

use admiralty_core::{AttemptId, PrincipalId, content_digest};
use admiralty_runtime::{
    reservations::{
        AccountId, CurrencyUnit, FinalEvidence, MinorUnits, ReconciliationEvidence,
        ReservationLedger, ReservationScope, ScopeLimits, reserve_budget, settle_usage,
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
fn codex_round22_p04_p10_canary_smoke_release_package() -> Result<(), Box<dyn Error>> {
    // P00: release inputs are pinned to deterministic task and scope identities.
    let contract = task_fixture("round22-canary-task");
    let contract_digest = content_digest(&contract)?;
    let scope = ReservationScope {
        project_id: contract.project_id.clone(),
        root_id: contract.root_id.clone(),
        task_id: contract.task_id.clone(),
        attempt_id: AttemptId::new("round22-attempt")?,
        owner: PrincipalId::new("round22-worker")?,
        incarnation: "round22-canary-incarnation".into(),
        contract_revision: contract.revision,
        contract_digest: contract_digest.clone(),
        authority_ref: contract.authority_ref.clone(),
        resource_claims_digest: content_digest(&contract.owned_paths)?,
        account_id: AccountId::new("round22-account")?,
        unit: CurrencyUnit::new("USD", 2)?,
    };
    assert_eq!(content_digest(&contract)?, contract_digest);

    // P01: a canary's scripted smoke results and logical lease replay exactly.
    let schedule = vec![
        FaultRule {
            point: "canary-smoke".into(),
            occurrence: 1,
            effect: FaultEffect::DelayMs(4),
        },
        FaultRule {
            point: "canary-smoke".into(),
            occurrence: 2,
            effect: FaultEffect::Fail,
        },
    ];
    let mut smoke = FaultPlan::new(schedule.clone())?;
    let mut replay = FaultPlan::new(schedule)?;
    for expected in [Some(FaultEffect::DelayMs(4)), Some(FaultEffect::Fail), None] {
        assert_eq!(smoke.hit("canary-smoke")?, expected);
        assert_eq!(replay.hit("canary-smoke")?, expected);
    }
    let mut clock = FakeClock::new(500);
    let lease = clock.deadline_after_ms(40)?;
    assert!(!lease.is_due(clock.now_ms()));
    assert_eq!(clock.advance_to_ms(lease.at_ms())?, 540);
    assert!(lease.is_due(clock.now_ms()));

    // P04: package workspace admission is hermetic and limits the canary to
    // its declared source files.
    let fixture = tempfile::tempdir()?;
    let root = fixture.path().join("candidate");
    fs::create_dir_all(root.join("src"))?;
    fs::create_dir_all(root.join(".private"))?;
    fs::write(root.join("src/main.rs"), b"synthetic canary")?;
    let workspace = WorkspaceSpec {
        node_id: NodeRef::new("round22-worker")?,
        root_id: scope.root_id.clone(),
        project_id: scope.project_id.clone(),
        workspace: fs::canonicalize(root)?,
        owned_paths: vec![PathBuf::from("src")],
        forbidden_paths: vec![PathBuf::from(".private")],
    }
    .validate()?;
    assert!(workspace.owns(std::path::Path::new("src/main.rs")));
    assert!(!workspace.owns(std::path::Path::new(".private/credential")));
    assert!(!workspace.owns(std::path::Path::new("../other/checkout")));

    // P10: a release receipt binds reserved exposure to complete reconciliation.
    let mut ledger = ReservationLedger::new();
    ledger.limits.insert(
        "round22-account".into(),
        ScopeLimits {
            unit: CurrencyUnit::new("USD", 2)?,
            account: Some(MinorUnits::new(80)),
            project: Some(MinorUnits::new(80)),
            root: Some(MinorUnits::new(80)),
        },
    );
    ledger = reserve_budget(
        &ledger,
        "round22-reserve",
        "round22-reservation",
        scope.clone(),
        "round22-canary-envelope",
        MinorUnits::new(60),
    )?;
    let evidence = FinalEvidence::SettledAndTerminated {
        evidence_ref: "round22-canary-receipt".into(),
        reconciliation: ReconciliationEvidence {
            scope_digest: content_digest(&scope)?,
            resource_claims_digest: scope.resource_claims_digest.clone(),
            usage_complete: Some(true),
            effects_reconciled: Some(true),
            resource_claims_reconciled: Some(true),
        },
    };
    let settled = settle_usage(
        &ledger,
        "round22-settle",
        "round22-reservation",
        MinorUnits::new(37),
        Some(evidence),
    )?;
    assert_eq!(settled.settled.get(&scope), Some(&MinorUnits::new(37)));
    assert_eq!(
        settled.unresolved_liabilities.get(&scope),
        Some(&MinorUnits::new(0))
    );

    // The package manifest is a deterministic, schema-pinned release record.
    // Its smoke status remains synthetic and cannot imply production acceptance.
    let package = serde_json::json!({
        "schema_version": 1,
        "contract_digest": contract_digest,
        "reservation_ledger": settled,
        "workspace_owned_paths": ["src"],
        "workspace_forbidden_paths": [".private"],
        "canary": {
            "synthetic": true,
            "acceptance": false,
            "fleet_rollout_closure": false,
        },
    });
    let package_bytes = serde_json::to_vec(&package)?;
    let replayed_bytes = serde_json::to_vec(&package)?;
    assert_eq!(package_bytes, replayed_bytes);
    let parsed: serde_json::Value = serde_json::from_slice(&package_bytes)?;
    assert_eq!(parsed, package);
    assert_eq!(parsed["canary"]["synthetic"], true);
    assert_eq!(parsed["canary"]["acceptance"], false);
    assert_eq!(parsed["canary"]["fleet_rollout_closure"], false);
    let typed_ledger: ReservationLedger =
        serde_json::from_value(parsed["reservation_ledger"].clone())?;
    assert_eq!(typed_ledger, settled);
    Ok(())
}
