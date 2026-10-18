use admiralty_core::{
    AttemptId, PrincipalId, ProjectId, RootId, TaskId, content_digest,
    economics::{
        BoundedId, CohortSpec, Coverage, DecimalU64, DecimalU128, ObservedAmount, ParentInclusion,
        RootBinding, UsageRecord, summarize_cohort,
    },
};
use admiralty_runtime::reservations::{
    self, AccountId, CurrencyUnit, FinalEvidence, MinorUnits, ReconciliationEvidence,
    ReservationError, ReservationLedger, ReservationScope, ScopeLimits,
};
use std::error::Error;

const CONTRACT_DIGEST: &str =
    "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const ORACLE_DIGEST: &str =
    "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const VALUATION_DIGEST: &str =
    "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

fn reservation_scope() -> Result<ReservationScope, Box<dyn Error>> {
    Ok(ReservationScope {
        project_id: ProjectId::new("round15-project")?,
        root_id: RootId::new("round15-root")?,
        task_id: TaskId::new("round15-task")?,
        attempt_id: AttemptId::new("round15-attempt")?,
        owner: PrincipalId::new("round15-worker")?,
        incarnation: "round15-incarnation".to_owned(),
        contract_revision: 1,
        contract_digest: CONTRACT_DIGEST.to_owned(),
        authority_ref: "fixture:round15-budget".to_owned(),
        resource_claims_digest: ORACLE_DIGEST.to_owned(),
        account_id: AccountId::new("round15-account")?,
        unit: CurrencyUnit::new("USD", 2)?,
    })
}

fn reconciliation(scope: &ReservationScope) -> Result<ReconciliationEvidence, Box<dyn Error>> {
    Ok(ReconciliationEvidence {
        scope_digest: content_digest(scope)?,
        resource_claims_digest: scope.resource_claims_digest.clone(),
        usage_complete: Some(true),
        effects_reconciled: Some(true),
        resource_claims_reconciled: Some(true),
    })
}

fn usage_record(
    project: &ProjectId,
    binding: &RootBinding,
    observation_id: String,
    unit: &str,
    amount: ObservedAmount,
) -> Result<UsageRecord, Box<dyn Error>> {
    Ok(UsageRecord {
        project_id: project.clone(),
        binding: binding.clone(),
        observation_id: BoundedId::new(observation_id)?,
        revision: DecimalU64(1),
        root_scope: binding.root_id.clone(),
        role: BoundedId::new("worker")?,
        purpose: BoundedId::new("release-smoke")?,
        provider: BoundedId::new("provider")?,
        unit: BoundedId::new(unit)?,
        amount,
        parent_inclusion: ParentInclusion::Standalone,
    })
}

#[test]
fn p26_p31_round15_release_smoke_multistep_stress_qualification() -> Result<(), Box<dyn Error>> {
    // Stress the quota ledger through repeated reserve/replay, failed over-cap
    // admission, partial settlement, and evidence-backed closure.
    let scope = reservation_scope()?;
    let mut ledger = ReservationLedger::new();
    ledger.limits.insert(
        scope.account_id.as_str().to_owned(),
        ScopeLimits {
            unit: scope.unit.clone(),
            account: Some(MinorUnits::new(500)),
            project: Some(MinorUnits::new(500)),
            root: Some(MinorUnits::new(500)),
        },
    );

    for index in 0..4_u8 {
        let event_id = format!("reserve-event-{index}");
        let reservation_id = format!("reservation-{index}");
        let correlation_id = format!("correlation-{index}");
        let next = reservations::reserve_budget(
            &ledger,
            event_id.clone(),
            reservation_id,
            scope.clone(),
            correlation_id,
            MinorUnits::new(100),
        )?;
        assert_eq!(
            reservations::reserve_budget(
                &ledger,
                event_id,
                format!("reservation-{index}"),
                scope.clone(),
                format!("correlation-{index}"),
                MinorUnits::new(100),
            )?,
            next,
            "identical event replay is idempotent at every ledger step"
        );
        ledger = next;
    }
    assert_eq!(
        ledger
            .unresolved_liabilities
            .get(&scope)
            .map(MinorUnits::get),
        Some(400)
    );
    let before_rejected_reserve = ledger.clone();
    assert!(matches!(
        reservations::reserve_budget(
            &ledger,
            "reserve-over-cap",
            "reservation-over-cap",
            scope.clone(),
            "correlation-over-cap",
            MinorUnits::new(101),
        ),
        Err(ReservationError::LimitExceeded("account"))
    ));
    assert_eq!(
        ledger, before_rejected_reserve,
        "failed quota admission has no state effect"
    );

    let partial = reservations::settle_usage(
        &ledger,
        "settle-partial",
        "reservation-0",
        MinorUnits::new(40),
        None,
    )?;
    assert_eq!(
        partial
            .unresolved_liabilities
            .get(&scope)
            .map(MinorUnits::get),
        Some(360)
    );
    let before_close = partial.clone();
    let closed = reservations::settle_usage(
        &partial,
        "settle-final",
        "reservation-0",
        MinorUnits::new(60),
        Some(FinalEvidence::SettledAndTerminated {
            evidence_ref: "fixture:reconciled-close".to_owned(),
            reconciliation: reconciliation(&scope)?,
        }),
    )?;
    assert_eq!(
        closed
            .unresolved_liabilities
            .get(&scope)
            .map(MinorUnits::get),
        Some(300)
    );
    assert_eq!(closed.settled.get(&scope).map(MinorUnits::get), Some(100));
    assert_eq!(
        reservations::settle_usage(
            &before_close,
            "settle-final",
            "reservation-0",
            MinorUnits::new(60),
            Some(FinalEvidence::SettledAndTerminated {
                evidence_ref: "fixture:reconciled-close".to_owned(),
                reconciliation: reconciliation(&scope)?,
            }),
        )?,
        closed,
        "final evidence replay preserves the closed ledger"
    );
    assert!(matches!(
        reservations::reserve_budget(
            &closed,
            "reserve-event-0",
            "reservation-0",
            scope.clone(),
            "correlation-0",
            MinorUnits::new(99),
        ),
        Err(ReservationError::EventCollision)
    ));

    // Multi-step cost-envelope replay remains exact, order independent, and
    // unit scoped; unknown cost is explicitly incomplete, never zero.
    let project = scope.project_id.clone();
    let binding = RootBinding {
        root_id: scope.root_id.clone(),
        task_id: scope.task_id.clone(),
        contract_revision: 1,
        contract_digest: CONTRACT_DIGEST.to_owned(),
        protected_oracle_digest: ORACLE_DIGEST.to_owned(),
    };
    let cohort = CohortSpec {
        cohort_id: BoundedId::new("round15-fixed-cohort")?,
        project_id: project.clone(),
        fixed_assignment_roots: vec![scope.root_id.clone()],
        required_delivery_stage: BoundedId::new("release")?,
        valuation_digest: VALUATION_DIGEST.to_owned(),
        assigned_at: DecimalU64(15),
        bindings: vec![binding.clone()],
    };
    let mut records = Vec::new();
    for amount in 1..=32_u128 {
        records.push(usage_record(
            &project,
            &binding,
            format!("token-meter-{amount}"),
            "tokens",
            ObservedAmount::Known {
                amount: DecimalU128(amount),
            },
        )?);
    }
    for amount in [125_u128, 250, 375] {
        records.push(usage_record(
            &project,
            &binding,
            format!("cost-meter-{amount}"),
            "USD:2",
            ObservedAmount::Known {
                amount: DecimalU128(amount),
            },
        )?);
    }
    records.push(usage_record(
        &project,
        &binding,
        "cost-meter-unknown".to_owned(),
        "USD:2",
        ObservedAmount::Unknown {
            reason: "provider cost interval is incomplete".to_owned(),
        },
    )?);
    let replay = records.iter().rev().cloned().collect::<Vec<_>>();
    let first = admiralty_core::economics::summarize_cohort(&cohort, &records, &[], 0)?;
    let second = summarize_cohort(&cohort, &replay, &[], 0)?;
    assert_eq!(
        first, second,
        "economics replay is stable under input reordering"
    );
    assert!(first.units.iter().any(|total| {
        total.unit.as_str() == "tokens"
            && total
                .amount
                .as_ref()
                .is_some_and(|amount| amount.as_u128() == 528)
            && total.coverage == Coverage::Complete
    }));
    assert!(first.units.iter().any(|total| {
        total.unit.as_str() == "USD:2"
            && total
                .amount
                .as_ref()
                .is_some_and(|amount| amount.as_u128() == 750)
            && total.coverage == Coverage::Partial
    }));
    Ok(())
}
