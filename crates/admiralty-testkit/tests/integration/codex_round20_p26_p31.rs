#![allow(clippy::unwrap_used, clippy::expect_used)]
use admiralty_core::{
    AttemptId, PrincipalId, ProjectId, RootId, TaskId, content_digest,
    economics::{
        BoundedId, CohortSpec, Coverage, DecimalU64, DecimalU128, EconomicsError, ObservedAmount,
        ParentInclusion, RootBinding, UsageRecord, summarize_cohort,
    },
};
use admiralty_runtime::reservations::{
    self, AccountId, CurrencyUnit, FinalEvidence, MinorUnits, ReconciliationEvidence,
    ReservationLedger, ReservationScope, ScopeLimits,
};
use serde::{Deserialize, Serialize};
use std::error::Error;

const CONTRACT_DIGEST: &str =
    "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const ORACLE_DIGEST: &str =
    "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const VALUATION_DIGEST: &str =
    "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ReleaseBundle {
    schema_version: u32,
    cohort: CohortSpec,
    usage_records: Vec<UsageRecord>,
    reservation_ledger: ReservationLedger,
}

impl ReleaseBundle {
    fn validate(&self) -> Result<(), EconomicsError> {
        if self.schema_version != 1 {
            return Err(EconomicsError::Invalid(
                "unsupported release bundle schema version".to_owned(),
            ));
        }
        self.cohort.validate()?;
        for record in &self.usage_records {
            record.validate()?;
            if record.project_id != self.cohort.project_id
                || record.root_scope != record.binding.root_id
                || !self.cohort.bindings.contains(&record.binding)
            {
                return Err(EconomicsError::Invalid(
                    "release evidence is outside the bound cohort".to_owned(),
                ));
            }
        }
        Ok(())
    }
}

fn scope() -> Result<ReservationScope, Box<dyn Error>> {
    Ok(ReservationScope {
        project_id: ProjectId::new("round20-project")?,
        root_id: RootId::new("round20-root")?,
        task_id: TaskId::new("round20-task")?,
        attempt_id: AttemptId::new("round20-attempt")?,
        owner: PrincipalId::new("round20-owner")?,
        incarnation: "round20-boot".to_owned(),
        contract_revision: 1,
        contract_digest: CONTRACT_DIGEST.to_owned(),
        authority_ref: "fixture:round20-release".to_owned(),
        resource_claims_digest: ORACLE_DIGEST.to_owned(),
        account_id: AccountId::new("round20-account")?,
        unit: CurrencyUnit::new("USD", 2)?,
    })
}

fn usage(
    project: &ProjectId,
    binding: &RootBinding,
    id: &str,
    amount: u128,
) -> Result<UsageRecord, Box<dyn Error>> {
    Ok(UsageRecord {
        project_id: project.clone(),
        binding: binding.clone(),
        observation_id: BoundedId::new(id)?,
        revision: DecimalU64(1),
        root_scope: binding.root_id.clone(),
        role: BoundedId::new("release-worker")?,
        purpose: BoundedId::new("bundle-validation")?,
        provider: BoundedId::new("provider")?,
        unit: BoundedId::new("USD:2")?,
        amount: ObservedAmount::Known {
            amount: DecimalU128(amount),
        },
        parent_inclusion: ParentInclusion::Standalone,
    })
}

#[test]
fn p26_p31_round20_post_pr57_release_qualification() -> Result<(), Box<dyn Error>> {
    let scope = scope()?;
    let binding = RootBinding {
        root_id: scope.root_id.clone(),
        task_id: scope.task_id.clone(),
        contract_revision: scope.contract_revision,
        contract_digest: scope.contract_digest.clone(),
        protected_oracle_digest: ORACLE_DIGEST.to_owned(),
    };
    let cohort = CohortSpec {
        cohort_id: BoundedId::new("round20-release-cohort")?,
        project_id: scope.project_id.clone(),
        fixed_assignment_roots: vec![scope.root_id.clone()],
        required_delivery_stage: BoundedId::new("release")?,
        valuation_digest: VALUATION_DIGEST.to_owned(),
        assigned_at: DecimalU64(18),
        bindings: vec![binding.clone()],
    };
    let usage_records = vec![
        usage(&scope.project_id, &binding, "bundle-cost-1", 275)?,
        usage(&scope.project_id, &binding, "bundle-cost-2", 425)?,
    ];
    let mut ledger = ReservationLedger::new();
    ledger.limits.insert(
        scope.account_id.as_str().to_owned(),
        ScopeLimits {
            unit: scope.unit.clone(),
            account: Some(MinorUnits::new(1_000)),
            project: Some(MinorUnits::new(1_000)),
            root: Some(MinorUnits::new(1_000)),
        },
    );
    // A request beyond the account envelope is rejected without mutating the
    // source ledger, so a rejected cutover cannot consume quota.
    assert!(
        reservations::reserve_budget(
            &ledger,
            "round20-over-limit",
            "round20-rejected-reservation",
            scope.clone(),
            "round20-release-envelope",
            MinorUnits::new(1_001),
        )
        .is_err()
    );
    assert!(ledger.reservations.is_empty());

    let reserved = reservations::reserve_budget(
        &ledger,
        "round20-reserve",
        "round20-reservation",
        scope.clone(),
        "round20-release-envelope",
        MinorUnits::new(700),
    )?;
    let final_evidence = FinalEvidence::SettledAndTerminated {
        evidence_ref: "fixture:round20-release-audit".to_owned(),
        reconciliation: ReconciliationEvidence {
            scope_digest: content_digest(&scope)?,
            resource_claims_digest: scope.resource_claims_digest.clone(),
            usage_complete: Some(true),
            effects_reconciled: Some(true),
            resource_claims_reconciled: Some(true),
        },
    };
    ledger = reservations::settle_usage(
        &reserved,
        "round20-settle",
        "round20-reservation",
        MinorUnits::new(700),
        Some(final_evidence),
    )?;
    assert_eq!(ledger.settled.get(&scope).map(MinorUnits::get), Some(700));

    let bundle = ReleaseBundle {
        schema_version: 1,
        cohort: cohort.clone(),
        usage_records: usage_records.clone(),
        reservation_ledger: ledger.clone(),
    };
    bundle.validate()?;
    let encoded = serde_json::to_vec(&bundle)?;
    let decoded: ReleaseBundle = serde_json::from_slice(&encoded)?;
    decoded.validate()?;
    assert_eq!(
        bundle, decoded,
        "all packet schemas round-trip without loss"
    );
    assert_eq!(
        encoded,
        serde_json::to_vec(&decoded)?,
        "canonical bundle bytes are stable"
    );
    assert_eq!(content_digest(&bundle)?, content_digest(&decoded)?);

    // Both protocols encode large counters as canonical decimal strings, and
    // the independently aggregated P26 cost equals the settled P31 amount.
    let wire: serde_json::Value = serde_json::from_slice(&encoded)?;
    assert_eq!(wire["usage_records"][0]["amount"]["amount"], "275");
    assert_eq!(wire["usage_records"][1]["amount"]["amount"], "425");
    assert_eq!(wire["reservation_ledger"]["settled"][0][1], "700");
    let summary = summarize_cohort(&cohort, &usage_records, &[], 0)?;
    assert!(summary.units.iter().any(|unit| {
        unit.unit.as_str() == "USD:2"
            && unit.coverage == Coverage::Complete
            && unit
                .amount
                .as_ref()
                .is_some_and(|amount| amount.as_u128() == 700)
    }));

    // Unknown fields and unsupported schema versions fail closed instead of
    // silently changing the meaning of a signed or replayed release bundle.
    let mut tampered = wire.clone();
    tampered["usage_records"][0]["trust_override"] = serde_json::Value::Bool(true);
    assert!(serde_json::from_value::<ReleaseBundle>(tampered).is_err());
    let mut unsupported = bundle;
    unsupported.schema_version = 2;
    assert!(unsupported.validate().is_err());
    Ok(())
}
