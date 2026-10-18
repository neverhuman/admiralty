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
fn codex_round15_p04_p10_release_smoke_multistep_stress() -> Result<(), Box<dyn Error>> {
    // P00: stable contract bytes keep the execution identity stable, and a
    // changed scope produces a different identity before any effect is proposed.
    let contract = task_fixture("round15-stress-task");
    let contract_digest = content_digest(&contract)?;
    assert_eq!(content_digest(&contract)?, contract_digest);
    let mut changed_contract = contract.clone();
    changed_contract
        .forbidden_paths
        .push("src/credentials".into());
    assert_ne!(content_digest(&changed_contract)?, contract_digest);

    let scope = ReservationScope {
        project_id: contract.project_id.clone(),
        root_id: contract.root_id.clone(),
        task_id: contract.task_id.clone(),
        attempt_id: AttemptId::new("round15-attempt")?,
        owner: PrincipalId::new("round15-owner")?,
        incarnation: "round15-worker-boot".into(),
        contract_revision: contract.revision,
        contract_digest,
        authority_ref: contract.authority_ref.clone(),
        resource_claims_digest: content_digest(&contract.owned_paths)?,
        account_id: AccountId::new("round15-account")?,
        unit: CurrencyUnit::new("USD", 2)?,
    };

    // P01: scripted faults and logical time replay with no wall-clock input.
    let rules = vec![
        FaultRule {
            point: "reservation-step".into(),
            occurrence: 2,
            effect: FaultEffect::DelayMs(7),
        },
        FaultRule {
            point: "reservation-step".into(),
            occurrence: 5,
            effect: FaultEffect::Fail,
        },
    ];
    let mut first = FaultPlan::new(rules.clone())?;
    let mut replay = FaultPlan::new(rules)?;
    for _ in 0..5 {
        assert_eq!(
            first.hit("reservation-step")?,
            replay.hit("reservation-step")?
        );
    }
    assert_eq!(first, replay);
    assert!(
        FaultPlan::new(vec![FaultRule {
            point: "invalid-step".into(),
            occurrence: 0,
            effect: FaultEffect::Drop,
        }])
        .is_err()
    );
    let mut clock = FakeClock::new(100);
    assert_eq!(clock.advance_ms(7)?, 107);
    assert_eq!(clock.now_ms(), 107);

    // P04: only the admitted source subtree belongs to the execution envelope.
    let directory = tempfile::tempdir()?;
    let root = directory.path().join("workspace");
    fs::create_dir_all(root.join("src/private"))?;
    fs::create_dir_all(root.join(".git"))?;
    fs::write(root.join("src/main.rs"), b"release smoke source")?;
    let root = fs::canonicalize(root)?;
    let workspace = WorkspaceSpec {
        node_id: NodeRef::new("round15-worker")?,
        root_id: scope.root_id.clone(),
        project_id: scope.project_id.clone(),
        workspace: root,
        owned_paths: vec![PathBuf::from("src")],
        forbidden_paths: vec![PathBuf::from("src/private"), PathBuf::from(".git")],
    }
    .validate()?;
    assert!(workspace.owns(std::path::Path::new("src/main.rs")));
    assert!(!workspace.owns(std::path::Path::new("src/private/key")));
    assert!(!workspace.owns(std::path::Path::new("../outside/command")));

    // P10: stress eight reservation lifecycles. Every reserve, stop, partial
    // settlement and final settlement is replayable, while collisions and
    // incomplete release evidence fail closed without changing prior state.
    let mut ledger = ReservationLedger::new();
    ledger.limits.insert(
        "round15-account".into(),
        ScopeLimits {
            unit: CurrencyUnit::new("USD", 2)?,
            account: Some(MinorUnits::new(100)),
            project: Some(MinorUnits::new(100)),
            root: Some(MinorUnits::new(100)),
        },
    );
    let scope_digest = content_digest(&scope)?;
    for step in 0..8 {
        let reservation_id = format!("round15-reservation-{step}");
        let correlation_id = format!("round15-envelope-{step}");
        let reserve_event_id = format!("round15-reserve-event-{step}");
        let reserve_event = ReservationEvent::Reserve {
            event_id: reserve_event_id.clone(),
            reservation_id: reservation_id.clone(),
            scope: scope.clone(),
            correlation_id: correlation_id.clone(),
            amount: MinorUnits::new(10),
        };
        let reserved = reduce_reservation(&ledger, reserve_event.clone())?;
        assert_eq!(
            reduce_reservation(&reserved, reserve_event)?,
            reserved,
            "step {step} reserve replay"
        );
        assert!(matches!(
            reserve_budget(
                &reserved,
                &reserve_event_id,
                &reservation_id,
                scope.clone(),
                format!("{correlation_id}-forged"),
                MinorUnits::new(10),
            ),
            Err(admiralty_runtime::reservations::ReservationError::EventCollision)
        ));

        let stop_event = ReservationEvent::RequestStop {
            event_id: format!("round15-stop-event-{step}"),
            reservation_id: reservation_id.clone(),
            binding: reserved
                .reservations
                .get(&reservation_id)
                .ok_or("reservation missing after admission")?
                .binding(),
            reason: StopReason::LeaseExpiry,
        };
        let stopped = reduce_reservation(&reserved, stop_event.clone())?;
        assert_eq!(reduce_reservation(&stopped, stop_event)?, stopped);
        assert_eq!(
            stopped.unresolved_liabilities.get(&scope),
            Some(&MinorUnits::new(10))
        );

        let partial_event_id = format!("round15-partial-event-{step}");
        let partial = settle_usage(
            &stopped,
            &partial_event_id,
            &reservation_id,
            MinorUnits::new(3),
            None,
        )?;
        assert_eq!(
            partial.unresolved_liabilities.get(&scope),
            Some(&MinorUnits::new(7))
        );

        let final_event_id = format!("round15-final-event-{step}");
        let evidence = FinalEvidence::SettledAndTerminated {
            evidence_ref: format!("round15-final-evidence-{step}"),
            reconciliation: ReconciliationEvidence {
                scope_digest: scope_digest.clone(),
                resource_claims_digest: scope.resource_claims_digest.clone(),
                usage_complete: Some(true),
                effects_reconciled: Some(true),
                resource_claims_reconciled: Some(true),
            },
        };
        let settled = settle_usage(
            &partial,
            &final_event_id,
            &reservation_id,
            MinorUnits::new(7),
            Some(evidence.clone()),
        )?;
        let final_binding = partial
            .reservations
            .get(&reservation_id)
            .ok_or("reservation missing before final settlement")?
            .binding();
        assert_eq!(
            reduce_reservation(
                &settled,
                ReservationEvent::Settle {
                    event_id: final_event_id,
                    reservation_id: reservation_id.clone(),
                    binding: final_binding,
                    amount: MinorUnits::new(7),
                    final_evidence: Some(evidence),
                },
            )?,
            settled,
            "step {step} final settlement replay"
        );
        assert_eq!(
            settled.unresolved_liabilities.get(&scope),
            Some(&MinorUnits::new(0))
        );

        let forged_evidence = FinalEvidence::SettledAndTerminated {
            evidence_ref: format!("round15-forged-evidence-{step}"),
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
                &stopped,
                format!("round15-forged-event-{step}"),
                &reservation_id,
                MinorUnits::new(0),
                Some(forged_evidence),
            )
            .is_err()
        );
        ledger = settled;
    }
    assert_eq!(ledger.revision, 32);
    assert_eq!(ledger.settled.get(&scope), Some(&MinorUnits::new(80)));
    Ok(())
}
