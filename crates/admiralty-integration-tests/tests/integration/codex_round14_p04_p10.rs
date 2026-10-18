use admiralty_core::{AttemptId, PrincipalId, ProjectId, RootId, TaskId};
use admiralty_runtime::{
    reservations::{
        AccountId, CurrencyUnit, FinalEvidence, MinorUnits, ReconciliationEvidence,
        ReservationEvent, ReservationLedger, ReservationScope, ScopeLimits, StopReason,
        reduce_reservation, reserve_budget, settle_usage,
    },
    workspace::registry::{NodeRef, WorkspaceSpec},
};
use std::{collections::BTreeMap, error::Error, fs, path::PathBuf};

fn scope() -> Result<ReservationScope, Box<dyn Error>> {
    Ok(ReservationScope {
        project_id: ProjectId::new("round14-project")?,
        root_id: RootId::new("round14-root")?,
        task_id: TaskId::new("round14-task")?,
        attempt_id: AttemptId::new("round14-attempt")?,
        owner: PrincipalId::new("round14-owner")?,
        incarnation: "round14-worker-boot".into(),
        contract_revision: 1,
        contract_digest: format!("sha256:{}", "a".repeat(64)),
        authority_ref: "round14-grant".into(),
        resource_claims_digest: format!("sha256:{}", "b".repeat(64)),
        account_id: AccountId::new("round14-account")?,
        unit: CurrencyUnit::new("USD", 2)?,
    })
}

fn limited_ledger() -> Result<ReservationLedger, Box<dyn Error>> {
    let mut ledger = ReservationLedger::new();
    ledger.limits.insert(
        "round14-account".into(),
        ScopeLimits {
            unit: CurrencyUnit::new("USD", 2)?,
            account: Some(MinorUnits::new(100)),
            project: Some(MinorUnits::new(100)),
            root: Some(MinorUnits::new(100)),
        },
    );
    Ok(ledger)
}

#[test]
fn codex_round14_p04_p10_release_readiness_and_deterministic_replay() -> Result<(), Box<dyn Error>>
{
    let scope = scope()?;
    let initial = limited_ledger()?;
    let reserved = reserve_budget(
        &initial,
        "round14-reserve-event",
        "round14-reservation",
        scope.clone(),
        "round14-execution-envelope",
        MinorUnits::new(25),
    )?;

    // P10: replaying the same event is stable and does not increase exposure.
    let replay = reduce_reservation(
        &reserved,
        ReservationEvent::Reserve {
            event_id: "round14-reserve-event".into(),
            reservation_id: "round14-reservation".into(),
            scope: scope.clone(),
            correlation_id: "round14-execution-envelope".into(),
            amount: MinorUnits::new(25),
        },
    )?;
    assert_eq!(replay, reserved);
    assert_eq!(
        replay.unresolved_liabilities.get(&scope),
        Some(&MinorUnits::new(25))
    );

    // P04: registered workspace ownership fails closed for traversal and
    // forbidden execution-envelope paths.
    let directory = tempfile::tempdir()?;
    let root = directory.path().join("workspace");
    fs::create_dir_all(root.join("src/private"))?;
    fs::create_dir_all(root.join(".git"))?;
    fs::write(root.join("src/main.rs"), b"stable source")?;
    let root = fs::canonicalize(root)?;
    let workspace = WorkspaceSpec {
        node_id: NodeRef::new("round14-worker")?,
        root_id: RootId::new("round14-root")?,
        project_id: ProjectId::new("round14-project")?,
        workspace: root,
        owned_paths: vec![PathBuf::from("src")],
        forbidden_paths: vec![PathBuf::from("src/private"), PathBuf::from(".git")],
    }
    .validate()?;
    assert!(workspace.owns(std::path::Path::new("src/main.rs")));
    assert!(!workspace.owns(std::path::Path::new("src/private/credentials")));
    assert!(!workspace.owns(std::path::Path::new("../outside/command")));

    // A stop request is intent only. Incomplete release evidence must not
    // settle the reservation or remove unresolved liability.
    let stopped = reduce_reservation(
        &reserved,
        ReservationEvent::RequestStop {
            event_id: "round14-stop-event".into(),
            reservation_id: "round14-reservation".into(),
            binding: reserved
                .reservations
                .get("round14-reservation")
                .ok_or("reservation missing after reserve")?
                .binding(),
            reason: StopReason::LeaseExpiry,
        },
    )?;
    let forged = FinalEvidence::SettledAndTerminated {
        evidence_ref: "round14-incomplete-evidence".into(),
        reconciliation: ReconciliationEvidence {
            scope_digest: "sha256:wrong-scope".into(),
            resource_claims_digest: scope.resource_claims_digest.clone(),
            usage_complete: Some(true),
            effects_reconciled: Some(false),
            resource_claims_reconciled: Some(true),
        },
    };
    assert!(
        settle_usage(
            &stopped,
            "round14-settlement-event",
            "round14-reservation",
            MinorUnits::new(0),
            Some(forged),
        )
        .is_err()
    );
    assert_eq!(
        stopped.unresolved_liabilities.get(&scope),
        Some(&MinorUnits::new(25))
    );

    // Fail closed when the account exposure is unknown, and keep the input
    // ledger untouched after the rejected proposal.
    let unknown = ReservationLedger {
        revision: initial.revision,
        limits: BTreeMap::new(),
        settled: initial.settled.clone(),
        unresolved_liabilities: initial.unresolved_liabilities.clone(),
        reservations: initial.reservations.clone(),
        processed_events: initial.processed_events.clone(),
    };
    assert!(
        reserve_budget(
            &unknown,
            "round14-unknown-limit-event",
            "round14-unknown-limit-reservation",
            scope,
            "round14-unknown-limit-envelope",
            MinorUnits::new(1),
        )
        .is_err()
    );
    assert_eq!(initial.revision, 0);
    Ok(())
}
