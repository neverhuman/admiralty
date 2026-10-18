#![allow(clippy::unwrap_used, clippy::expect_used)]
use admiralty_core::{AttemptId, PrincipalId, TaskContract, content_digest};
use admiralty_runtime::{
    reservations::{
        AccountId, CurrencyUnit, MinorUnits, ReservationError, ReservationEvent, ReservationLedger,
        ReservationScope, ScopeLimits, reduce_reservation, reserve_budget,
    },
    workspace::registry::{NodeRef, WorkspaceSpec},
};
use admiralty_testkit::{
    faults::{FaultEffect, FaultPlan, FaultRule},
    task_fixture,
};
use std::{error::Error, fs, path::PathBuf, thread};

fn scope(contract: &TaskContract, worker: &str) -> Result<ReservationScope, Box<dyn Error>> {
    Ok(ReservationScope {
        project_id: contract.project_id.clone(),
        root_id: contract.root_id.clone(),
        task_id: contract.task_id.clone(),
        attempt_id: AttemptId::new(format!("{worker}-attempt"))?,
        owner: PrincipalId::new(worker)?,
        incarnation: format!("{worker}-boot"),
        contract_revision: contract.revision,
        contract_digest: content_digest(contract)?,
        authority_ref: contract.authority_ref.clone(),
        resource_claims_digest: content_digest(&contract.owned_paths)?,
        account_id: AccountId::new("round17-shared-account")?,
        unit: CurrencyUnit::new("USD", 2)?,
    })
}

#[test]
fn codex_round17_p04_p10_multi_worker_reservation_contention() -> Result<(), Box<dyn Error>> {
    // P00: two workers bind separate immutable work identities to one account.
    let first_contract = task_fixture("round17-worker-one");
    let second_contract = task_fixture("round17-worker-two");
    let first_scope = scope(&first_contract, "round17-worker-one")?;
    let second_scope = scope(&second_contract, "round17-worker-two")?;
    assert_ne!(first_scope.contract_digest, second_scope.contract_digest);

    // P01: both workers replay the same deterministic admission fault schedule.
    let rules = vec![FaultRule {
        point: "shared-account-admission".into(),
        occurrence: 2,
        effect: FaultEffect::Fail,
    }];
    let mut first_faults = FaultPlan::new(rules.clone())?;
    let mut second_faults = FaultPlan::new(rules)?;
    for expected in [None, Some(FaultEffect::Fail), None] {
        assert_eq!(first_faults.hit("shared-account-admission")?, expected);
        assert_eq!(second_faults.hit("shared-account-admission")?, expected);
    }

    // P04: worker one sees only its admitted source subtree; credentials and
    // traversal paths remain outside the workspace execution envelope.
    let directory = tempfile::tempdir()?;
    let root = directory.path().join("worker-one");
    fs::create_dir_all(root.join("src/private"))?;
    fs::create_dir_all(root.join(".git"))?;
    fs::write(root.join("src/main.rs"), b"worker one source")?;
    let workspace = WorkspaceSpec {
        node_id: NodeRef::new("round17-worker-one")?,
        root_id: first_scope.root_id.clone(),
        project_id: first_scope.project_id.clone(),
        workspace: fs::canonicalize(root)?,
        owned_paths: vec![PathBuf::from("src")],
        forbidden_paths: vec![PathBuf::from("src/private"), PathBuf::from(".git")],
    }
    .validate()?;
    assert!(workspace.owns(std::path::Path::new("src/main.rs")));
    assert!(!workspace.owns(std::path::Path::new("src/private/key")));
    assert!(!workspace.owns(std::path::Path::new("../worker-two/command")));

    // P10: two threads propose against the same snapshot. Both local proposals
    // fit alone, but linearized application rechecks the shared account limit;
    // the losing worker cannot overwrite the winner or exceed the cap.
    let mut initial = ReservationLedger::new();
    initial.limits.insert(
        "round17-shared-account".into(),
        ScopeLimits {
            unit: CurrencyUnit::new("USD", 2)?,
            account: Some(MinorUnits::new(100)),
            project: Some(MinorUnits::new(100)),
            root: Some(MinorUnits::new(100)),
        },
    );
    let second_event = ReservationEvent::Reserve {
        event_id: "round17-reserve-two".into(),
        reservation_id: "round17-reservation-two".into(),
        scope: second_scope.clone(),
        correlation_id: "round17-envelope-two".into(),
        amount: MinorUnits::new(60),
    };
    let (first_proposal, second_proposal) = thread::scope(|threads| {
        let first_ledger = &initial;
        let first_worker_scope = first_scope.clone();
        let first_worker = threads.spawn(move || {
            reserve_budget(
                first_ledger,
                "round17-reserve-one",
                "round17-reservation-one",
                first_worker_scope,
                "round17-envelope-one",
                MinorUnits::new(60),
            )
        });
        let second_ledger = &initial;
        let second_worker_event = second_event.clone();
        let second_worker =
            threads.spawn(move || reduce_reservation(second_ledger, second_worker_event));
        let first_result = first_worker
            .join()
            .map_err(|_| std::io::Error::other("first worker panicked"))??;
        let second_result = second_worker
            .join()
            .map_err(|_| std::io::Error::other("second worker panicked"))??;
        Ok::<_, Box<dyn Error>>((first_result, second_result))
    })?;
    assert_eq!(first_proposal.revision, 1);
    assert_eq!(second_proposal.revision, 1);
    assert_ne!(first_proposal, second_proposal);
    let applied = first_proposal.clone();
    assert!(matches!(
        reduce_reservation(&applied, second_event),
        Err(ReservationError::LimitExceeded("account"))
    ));
    assert_eq!(applied, first_proposal);
    assert_eq!(
        applied.unresolved_liabilities.get(&first_scope),
        Some(&MinorUnits::new(60))
    );
    assert_eq!(
        applied.unresolved_liabilities.get(&second_scope),
        None,
        "rejected worker exposure is not admitted"
    );

    // An exact retry for the winning worker remains idempotent after contention.
    let winning_replay = reduce_reservation(
        &applied,
        ReservationEvent::Reserve {
            event_id: "round17-reserve-one".into(),
            reservation_id: "round17-reservation-one".into(),
            scope: first_scope,
            correlation_id: "round17-envelope-one".into(),
            amount: MinorUnits::new(60),
        },
    )?;
    assert_eq!(winning_replay, applied);
    Ok(())
}
