use admiralty_core::{
    AttemptId, PrincipalId, ProjectId, RootId, TaskId,
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

fn scope() -> Result<ReservationScope, Box<dyn Error>> {
    Ok(ReservationScope {
        project_id: ProjectId::new("round17-project")?,
        root_id: RootId::new("round17-root")?,
        task_id: TaskId::new("round17-task")?,
        attempt_id: AttemptId::new("round17-attempt")?,
        owner: PrincipalId::new("round17-owner")?,
        incarnation: "round17-boot".to_owned(),
        contract_revision: 1,
        contract_digest: CONTRACT_DIGEST.to_owned(),
        authority_ref: "fixture:round17-release".to_owned(),
        resource_claims_digest: ORACLE_DIGEST.to_owned(),
        account_id: AccountId::new("round17-account")?,
        unit: CurrencyUnit::new("USD", 2)?,
    })
}

fn final_evidence(scope: &ReservationScope) -> Result<FinalEvidence, Box<dyn Error>> {
    Ok(FinalEvidence::SettledAndTerminated {
        evidence_ref: "fixture:round17-independent-reconciliation".to_owned(),
        reconciliation: ReconciliationEvidence {
            scope_digest: admiralty_core::content_digest(scope)?,
            resource_claims_digest: scope.resource_claims_digest.clone(),
            usage_complete: Some(true),
            effects_reconciled: Some(true),
            resource_claims_reconciled: Some(true),
        },
    })
}

fn usage(
    project: &ProjectId,
    binding: &RootBinding,
    observation_id: &str,
    amount: ObservedAmount,
) -> Result<UsageRecord, Box<dyn Error>> {
    Ok(UsageRecord {
        project_id: project.clone(),
        binding: binding.clone(),
        observation_id: BoundedId::new(observation_id)?,
        revision: DecimalU64(1),
        root_scope: binding.root_id.clone(),
        role: BoundedId::new("release-worker")?,
        purpose: BoundedId::new("release-validation")?,
        provider: BoundedId::new("provider")?,
        unit: BoundedId::new("USD:2")?,
        amount,
        parent_inclusion: ParentInclusion::Standalone,
    })
}

#[test]
fn p26_p31_round17_post_item5_release_audit_cross_packet_invariants() -> Result<(), Box<dyn Error>>
{
    let scope = scope()?;
    let binding = RootBinding {
        root_id: scope.root_id.clone(),
        task_id: scope.task_id.clone(),
        contract_revision: scope.contract_revision,
        contract_digest: scope.contract_digest.clone(),
        protected_oracle_digest: ORACLE_DIGEST.to_owned(),
    };
    let cohort = CohortSpec {
        cohort_id: BoundedId::new("round17-release-cohort")?,
        project_id: scope.project_id.clone(),
        fixed_assignment_roots: vec![scope.root_id.clone()],
        required_delivery_stage: BoundedId::new("release")?,
        valuation_digest: VALUATION_DIGEST.to_owned(),
        assigned_at: DecimalU64(17),
        bindings: vec![binding.clone()],
    };
    let records = vec![
        usage(
            &scope.project_id,
            &binding,
            "release-cost-a",
            ObservedAmount::Known {
                amount: DecimalU128(200),
            },
        )?,
        usage(
            &scope.project_id,
            &binding,
            "release-cost-b",
            ObservedAmount::Known {
                amount: DecimalU128(300),
            },
        )?,
    ];
    let first = summarize_cohort(&cohort, &records, &[], 0)?;
    let replay = vec![records[1].clone(), records[0].clone(), records[0].clone()];
    assert_eq!(first, summarize_cohort(&cohort, &replay, &[], 0)?);
    let usd_total = first
        .units
        .iter()
        .find(|unit| unit.unit.as_str() == "USD:2");
    assert!(matches!(
        usd_total,
        Some(total)
            if total.coverage == Coverage::Complete
                && total.amount.as_ref().is_some_and(|amount| amount.as_u128() == 500)
    ));

    // Quota settlement and the independently summarized cost envelope agree
    // exactly in scope and currency, including replay and remaining capacity.
    let mut ledger = ReservationLedger::new();
    ledger.limits.insert(
        scope.account_id.as_str().to_owned(),
        ScopeLimits {
            unit: scope.unit.clone(),
            account: Some(MinorUnits::new(750)),
            project: Some(MinorUnits::new(750)),
            root: Some(MinorUnits::new(750)),
        },
    );
    let reserved = reservations::reserve_budget(
        &ledger,
        "round17-reserve",
        "round17-reservation",
        scope.clone(),
        "round17-cost-envelope",
        MinorUnits::new(500),
    )?;
    assert_eq!(
        reservations::reserve_budget(
            &ledger,
            "round17-reserve",
            "round17-reservation",
            scope.clone(),
            "round17-cost-envelope",
            MinorUnits::new(500),
        )?,
        reserved
    );
    let settled = reservations::settle_usage(
        &reserved,
        "round17-settle",
        "round17-reservation",
        MinorUnits::new(500),
        Some(final_evidence(&scope)?),
    )?;
    assert_eq!(settled.settled.get(&scope).map(MinorUnits::get), Some(500));
    assert_eq!(
        usd_total
            .and_then(|total| total.amount.as_ref())
            .map(DecimalU128::as_u128),
        settled
            .settled
            .get(&scope)
            .map(MinorUnits::get)
            .map(u128::from),
        "P26 measured cost and P31 settled quota ledger must reconcile"
    );
    assert_eq!(
        settled
            .unresolved_liabilities
            .get(&scope)
            .map(MinorUnits::get),
        Some(0)
    );

    // Incomplete usage cannot be presented as complete release evidence, and
    // the settled amount continues to consume quota after reservation closure.
    let mut incomplete = records.clone();
    incomplete.push(usage(
        &scope.project_id,
        &binding,
        "release-cost-unknown",
        ObservedAmount::Unknown {
            reason: "missing provider interval".to_owned(),
        },
    )?);
    let partial = summarize_cohort(&cohort, &incomplete, &[], 0)?;
    assert!(partial.units.iter().any(|unit| {
        unit.unit.as_str() == "USD:2"
            && unit.coverage == Coverage::Partial
            && unit
                .amount
                .as_ref()
                .is_some_and(|amount| amount.as_u128() == 500)
    }));
    let at_quota = reservations::reserve_budget(
        &settled,
        "round17-reserve-remaining",
        "round17-reservation-remaining",
        scope.clone(),
        "round17-remaining-envelope",
        MinorUnits::new(250),
    )?;
    assert!(matches!(
        reservations::reserve_budget(
            &at_quota,
            "round17-reserve-over-limit",
            "round17-reservation-over-limit",
            scope.clone(),
            "round17-over-limit-envelope",
            MinorUnits::new(1),
        ),
        Err(ReservationError::LimitExceeded("account"))
    ));

    let mut foreign = records.clone();
    foreign[0].project_id = ProjectId::new("foreign-project")?;
    assert!(summarize_cohort(&cohort, &foreign, &[], 0).is_err());
    Ok(())
}
