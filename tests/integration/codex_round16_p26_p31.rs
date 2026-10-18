use admiralty_core::{
    AttemptId, PrincipalId, ProjectId, RootId, TaskId,
    economics::{
        BoundedId, CohortSpec, Coverage, DecimalU64, DecimalU128, EconomicsError, ObservedAmount,
        ParentInclusion, RootBinding, UsageRecord, summarize_cohort,
    },
};
use admiralty_runtime::reservations::{
    self, AccountId, CurrencyUnit, FinalEvidence, MinorUnits, ReconciliationEvidence,
    ReservationError, ReservationLedger, ReservationScope, ScopeLimits,
};
use admiralty_testkit::faults::{FaultEffect, FaultPlan, FaultRule};
use std::error::Error;

const CONTRACT_DIGEST: &str =
    "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const ORACLE_DIGEST: &str =
    "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const VALUATION_DIGEST: &str =
    "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

fn scope() -> Result<ReservationScope, Box<dyn Error>> {
    Ok(ReservationScope {
        project_id: ProjectId::new("round16-project")?,
        root_id: RootId::new("round16-root")?,
        task_id: TaskId::new("round16-task")?,
        attempt_id: AttemptId::new("round16-attempt")?,
        owner: PrincipalId::new("round16-worker")?,
        incarnation: "round16-incarnation".to_owned(),
        contract_revision: 1,
        contract_digest: CONTRACT_DIGEST.to_owned(),
        authority_ref: "fixture:round16-budget".to_owned(),
        resource_claims_digest: ORACLE_DIGEST.to_owned(),
        account_id: AccountId::new("round16-account")?,
        unit: CurrencyUnit::new("USD", 2)?,
    })
}

fn evidence(
    scope: &ReservationScope,
    complete: Option<bool>,
) -> Result<ReconciliationEvidence, Box<dyn Error>> {
    Ok(ReconciliationEvidence {
        scope_digest: admiralty_core::content_digest(scope)?,
        resource_claims_digest: scope.resource_claims_digest.clone(),
        usage_complete: complete,
        effects_reconciled: Some(true),
        resource_claims_reconciled: Some(true),
    })
}

fn usage(
    project: &ProjectId,
    binding: &RootBinding,
    observation_id: impl Into<String>,
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
        purpose: BoundedId::new("cutover")?,
        provider: BoundedId::new("provider")?,
        unit: BoundedId::new(unit)?,
        amount,
        parent_inclusion: ParentInclusion::Standalone,
    })
}

#[test]
fn p26_p31_round16_end_to_end_cutover_fault_containment_qualification() -> Result<(), Box<dyn Error>>
{
    let scope = scope()?;
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
    let reserved = reservations::reserve_budget(
        &ledger,
        "reserve-cutover-1",
        "reservation-cutover-1",
        scope.clone(),
        "correlation-cutover-1",
        MinorUnits::new(300),
    )?;
    assert_eq!(
        reservations::reserve_budget(
            &ledger,
            "reserve-cutover-1",
            "reservation-cutover-1",
            scope.clone(),
            "correlation-cutover-1",
            MinorUnits::new(300),
        )?,
        reserved,
        "reserve replay must be idempotent"
    );

    // A deterministic pre-settlement crash leaves the original liability
    // intact; the same operation can safely retry without fault.
    let mut fault = FaultPlan::new(vec![FaultRule {
        point: "round16.settle.before_apply".to_owned(),
        occurrence: 1,
        effect: FaultEffect::Crash,
    }])?;
    let before_fault = reserved.clone();
    assert_eq!(
        fault.hit("round16.settle.before_apply")?,
        Some(FaultEffect::Crash)
    );
    assert_eq!(reserved, before_fault);
    assert_eq!(
        reserved
            .unresolved_liabilities
            .get(&scope)
            .map(MinorUnits::get),
        Some(300)
    );
    assert_eq!(fault.hit("round16.settle.before_apply")?, None);

    let partial = reservations::settle_usage(
        &reserved,
        "settle-cutover-partial",
        "reservation-cutover-1",
        MinorUnits::new(100),
        None,
    )?;
    assert_eq!(
        partial
            .unresolved_liabilities
            .get(&scope)
            .map(MinorUnits::get),
        Some(200)
    );
    let bad_close = reservations::settle_usage(
        &partial,
        "settle-cutover-final",
        "reservation-cutover-1",
        MinorUnits::new(200),
        Some(FinalEvidence::SettledAndTerminated {
            evidence_ref: "fixture:incomplete-final-evidence".to_owned(),
            reconciliation: evidence(&scope, None)?,
        }),
    );
    assert!(matches!(
        bad_close,
        Err(ReservationError::InvalidSettlement)
    ));
    assert_eq!(
        partial
            .unresolved_liabilities
            .get(&scope)
            .map(MinorUnits::get),
        Some(200)
    );

    let closed = reservations::settle_usage(
        &partial,
        "settle-cutover-final",
        "reservation-cutover-1",
        MinorUnits::new(200),
        Some(FinalEvidence::SettledAndTerminated {
            evidence_ref: "fixture:complete-final-evidence".to_owned(),
            reconciliation: evidence(&scope, Some(true))?,
        }),
    )?;
    assert_eq!(
        closed
            .unresolved_liabilities
            .get(&scope)
            .map(MinorUnits::get),
        Some(0)
    );
    assert_eq!(closed.settled.get(&scope).map(MinorUnits::get), Some(300));
    assert_eq!(
        reservations::settle_usage(
            &partial,
            "settle-cutover-final",
            "reservation-cutover-1",
            MinorUnits::new(200),
            Some(FinalEvidence::SettledAndTerminated {
                evidence_ref: "fixture:complete-final-evidence".to_owned(),
                reconciliation: evidence(&scope, Some(true))?,
            }),
        )?,
        closed,
        "cutover replay cannot settle twice"
    );

    // Settled cost still counts against the account envelope. Exactly the
    // remaining allowance is admissible; one more minor unit fails atomically.
    let at_limit = reservations::reserve_budget(
        &closed,
        "reserve-cutover-2",
        "reservation-cutover-2",
        scope.clone(),
        "correlation-cutover-2",
        MinorUnits::new(200),
    )?;
    assert_eq!(
        at_limit
            .unresolved_liabilities
            .get(&scope)
            .map(MinorUnits::get),
        Some(200)
    );
    assert!(matches!(
        reservations::reserve_budget(
            &at_limit,
            "reserve-cutover-over-limit",
            "reservation-cutover-over-limit",
            scope.clone(),
            "correlation-cutover-over-limit",
            MinorUnits::new(1),
        ),
        Err(ReservationError::LimitExceeded("account"))
    ));
    assert_eq!(
        at_limit
            .unresolved_liabilities
            .get(&scope)
            .map(MinorUnits::get),
        Some(200)
    );

    // Exact economics replays survive reordering and duplicate delivery, while
    // unknown price coverage remains partial instead of becoming an all-in cost.
    let project = scope.project_id.clone();
    let binding = RootBinding {
        root_id: scope.root_id.clone(),
        task_id: scope.task_id.clone(),
        contract_revision: 1,
        contract_digest: CONTRACT_DIGEST.to_owned(),
        protected_oracle_digest: ORACLE_DIGEST.to_owned(),
    };
    let cohort = CohortSpec {
        cohort_id: BoundedId::new("round16-cohort")?,
        project_id: project.clone(),
        fixed_assignment_roots: vec![scope.root_id.clone()],
        required_delivery_stage: BoundedId::new("release")?,
        valuation_digest: VALUATION_DIGEST.to_owned(),
        assigned_at: DecimalU64(16),
        bindings: vec![binding.clone()],
    };
    let mut records = Vec::new();
    for index in 1..=40_u128 {
        records.push(usage(
            &project,
            &binding,
            format!("tokens-{index}"),
            "tokens",
            ObservedAmount::Known {
                amount: DecimalU128(index),
            },
        )?);
    }
    for index in 1..=6_u128 {
        records.push(usage(
            &project,
            &binding,
            format!("cost-{index}"),
            "USD:6",
            ObservedAmount::Known {
                amount: DecimalU128(index * 125),
            },
        )?);
    }
    records.push(usage(
        &project,
        &binding,
        "cost-unknown",
        "USD:6",
        ObservedAmount::Unknown {
            reason: "cost sample missing at cutover".to_owned(),
        },
    )?);
    let summary = summarize_cohort(&cohort, &records, &[], 0)?;
    let mut replay = records.iter().rev().cloned().collect::<Vec<_>>();
    replay.push(records[0].clone());
    assert_eq!(summary, summarize_cohort(&cohort, &replay, &[], 0)?);
    assert!(summary.units.iter().any(|unit| {
        unit.unit.as_str() == "tokens"
            && unit
                .amount
                .as_ref()
                .is_some_and(|amount| amount.as_u128() == 820)
            && unit.coverage == Coverage::Complete
    }));
    assert!(summary.units.iter().any(|unit| {
        unit.unit.as_str() == "USD:6"
            && unit
                .amount
                .as_ref()
                .is_some_and(|amount| amount.as_u128() == 2625)
            && unit.coverage == Coverage::Partial
    }));

    let overflow = vec![
        usage(
            &project,
            &binding,
            "cost-max",
            "USD:6",
            ObservedAmount::Known {
                amount: DecimalU128(u128::MAX),
            },
        )?,
        usage(
            &project,
            &binding,
            "cost-overflow",
            "USD:6",
            ObservedAmount::Known {
                amount: DecimalU128(1),
            },
        )?,
    ];
    assert_eq!(
        summarize_cohort(&cohort, &overflow, &[], 0),
        Err(EconomicsError::Overflow)
    );
    Ok(())
}
