use admiralty_core::{
    AttemptId, PrincipalId, ProjectId, RootId, TaskId,
    economics::{
        self, BoundedId, CohortSpec, DecimalU64, DecimalU128, ObservedAmount, ParentInclusion,
        RootBinding, RootOutcome, RootStatus, UsageRecord,
    },
};
use admiralty_runtime::reservations::{
    self, AccountId, CurrencyUnit, FinalEvidence, MinorUnits, ReconciliationEvidence,
    ReservationEvent, ReservationLedger, ReservationScope, ScopeLimits, StopReason,
};

fn id(value: &str) -> BoundedId {
    BoundedId::new(value).expect("valid economics fixture identifier")
}

fn root_binding(root_id: &RootId) -> RootBinding {
    RootBinding {
        root_id: root_id.clone(),
        task_id: TaskId::new(format!("task-{}", root_id.as_str())).expect("valid task fixture"),
        contract_revision: 1,
        contract_digest: format!("sha256:{}", "a".repeat(64)),
        protected_oracle_digest: format!("sha256:{}", "b".repeat(64)),
    }
}

fn cohort(roots: &[RootId]) -> CohortSpec {
    CohortSpec {
        cohort_id: id("fixed-cohort"),
        project_id: ProjectId::new("project-1").expect("valid project fixture"),
        fixed_assignment_roots: roots.to_vec(),
        required_delivery_stage: id("accepted"),
        valuation_digest: format!("sha256:{}", "c".repeat(64)),
        assigned_at: DecimalU64(1),
        bindings: roots.iter().map(root_binding).collect(),
    }
}

fn usage(
    root: &RootId,
    observation_id: &str,
    revision: u64,
    unit: &str,
    amount: ObservedAmount,
) -> UsageRecord {
    UsageRecord {
        project_id: ProjectId::new("project-1").expect("valid project fixture"),
        binding: root_binding(root),
        observation_id: id(observation_id),
        revision: DecimalU64(revision),
        root_scope: root.clone(),
        role: id("worker"),
        purpose: id("implementation"),
        provider: id("provider-a"),
        unit: id(unit),
        amount,
        parent_inclusion: ParentInclusion::Standalone,
    }
}

fn outcome(root: &RootId, status: RootStatus, stage: Option<&str>) -> RootOutcome {
    let accepted = matches!(status, RootStatus::Succeeded);
    RootOutcome {
        project_id: ProjectId::new("project-1").expect("valid project fixture"),
        binding: root_binding(root),
        acceptance_receipt_digest: accepted.then(|| format!("sha256:{}", "d".repeat(64))),
        root_id: root.clone(),
        status,
        accepted_stage: stage.map(id),
        maturity: DecimalU64(10),
    }
}

#[test]
fn cohort_revisions_duplicates_and_unknown_usage_preserve_coverage() {
    let accepted_root = RootId::new("root-accepted").expect("valid root fixture");
    let failed_root = RootId::new("root-failed").expect("valid root fixture");
    let cohort = cohort(&[accepted_root.clone(), failed_root.clone()]);
    let revision_one = usage(
        &accepted_root,
        "usage-accepted",
        1,
        "tokens",
        ObservedAmount::Known {
            amount: DecimalU128(100),
        },
    );
    let revision_two = usage(
        &accepted_root,
        "usage-accepted",
        2,
        "tokens",
        ObservedAmount::Known {
            amount: DecimalU128(125),
        },
    );
    let unknown = usage(
        &failed_root,
        "usage-unknown",
        1,
        "tokens",
        ObservedAmount::Unknown {
            reason: "provider reported an incomplete interval".into(),
        },
    );
    let summary = economics::summarize_cohort(
        &cohort,
        &[revision_one, revision_two.clone(), revision_two, unknown],
        &[
            outcome(&accepted_root, RootStatus::Succeeded, Some("accepted")),
            outcome(&failed_root, RootStatus::Failed, None),
        ],
        10,
    )
    .expect("summarize fixed assignment cohort");

    assert_eq!(summary.assigned_roots, DecimalU64(2));
    assert_eq!(summary.accepted_roots, Some(DecimalU64(1)));
    assert_eq!(summary.failed_roots, DecimalU64(1));
    assert_eq!(
        summary
            .accepted_yield
            .as_ref()
            .map(|r| (r.numerator.as_u128(), r.denominator.as_u128())),
        Some((1, 2))
    );
    let tokens = summary
        .units
        .iter()
        .find(|unit| unit.unit.as_str() == "tokens")
        .expect("tokens subtotal exists");
    assert_eq!(tokens.amount.as_ref().map(DecimalU128::as_u128), Some(125));
    assert_eq!(tokens.coverage, economics::Coverage::Partial);
    assert_eq!(tokens.included_records, DecimalU64(1));
    assert!(
        summary
            .limitations
            .iter()
            .any(|limitation| limitation.contains("usage is unknown"))
    );
}

#[test]
fn unknown_only_cost_is_null_and_zero_success_is_not_a_savings_claim() {
    let root = RootId::new("root-zero-success").expect("valid root fixture");
    let cohort = cohort(std::slice::from_ref(&root));
    let summary = economics::summarize_cohort(
        &cohort,
        &[usage(
            &root,
            "usage-unpriced",
            1,
            "usd_micros",
            ObservedAmount::Unknown {
                reason: "no exact valuation available".into(),
            },
        )],
        &[outcome(&root, RootStatus::Failed, None)],
        10,
    )
    .expect("summarize zero-success cohort with unknown valuation");
    assert_eq!(summary.accepted_roots, Some(DecimalU64(0)));
    assert_eq!(
        summary
            .accepted_yield
            .as_ref()
            .map(|r| (r.numerator.as_u128(), r.denominator.as_u128())),
        Some((0, 1))
    );
    let cost = &summary.units[0];
    assert_eq!(cost.amount, None);
    assert_eq!(cost.coverage, economics::Coverage::Unknown);
    assert_eq!(summary.legacy_yield_heuristic, None);
}

fn reservation_scope() -> ReservationScope {
    ReservationScope {
        project_id: ProjectId::new("project-1").expect("valid project fixture"),
        root_id: RootId::new("root-1").expect("valid root fixture"),
        task_id: TaskId::new("task-1").expect("valid task fixture"),
        attempt_id: AttemptId::new("attempt-1").expect("valid attempt fixture"),
        owner: PrincipalId::new("worker-1").expect("valid owner fixture"),
        incarnation: "incarnation-1".into(),
        contract_revision: 1,
        contract_digest: format!("sha256:{}", "a".repeat(64)),
        authority_ref: "budget:fixture".into(),
        resource_claims_digest: format!("sha256:{}", "b".repeat(64)),
        account_id: AccountId::new("account-1").expect("valid account fixture"),
        unit: CurrencyUnit::new("USD", 2).expect("valid currency fixture"),
    }
}

fn limited_ledger() -> ReservationLedger {
    let mut ledger = ReservationLedger::new();
    ledger.limits.insert(
        "account-1".into(),
        ScopeLimits {
            unit: CurrencyUnit::new("USD", 2).expect("valid limit unit"),
            account: Some(MinorUnits::new(1_000)),
            project: Some(MinorUnits::new(1_000)),
            root: Some(MinorUnits::new(1_000)),
        },
    );
    ledger
}

fn reconciliation(scope: &ReservationScope) -> ReconciliationEvidence {
    ReconciliationEvidence {
        scope_digest: admiralty_core::content_digest(scope).expect("digest reservation scope"),
        resource_claims_digest: scope.resource_claims_digest.clone(),
        usage_complete: Some(true),
        effects_reconciled: Some(true),
        resource_claims_reconciled: Some(true),
    }
}

#[test]
fn reservations_replay_hold_unknown_liability_settle_and_correct_exact_minor_units() {
    let scope = reservation_scope();
    let empty = limited_ledger();
    let reserved = reservations::reserve_budget(
        &empty,
        "reserve-1",
        "reservation-1",
        scope.clone(),
        "correlation-1",
        MinorUnits::new(1),
    )
    .expect("reserve one exact minor unit");
    assert_eq!(
        reservations::reserve_budget(
            &reserved,
            "reserve-1",
            "reservation-1",
            scope.clone(),
            "correlation-1",
            MinorUnits::new(1),
        )
        .expect("replaying an applied reserve returns the unchanged ledger"),
        reserved
    );
    assert_eq!(
        reservations::reserve_budget(
            &empty,
            "reserve-1",
            "reservation-1",
            scope.clone(),
            "correlation-1",
            MinorUnits::new(1),
        )
        .expect("exact event replay is idempotent"),
        reserved
    );
    assert_eq!(reserved.unresolved_liabilities[&scope].get(), 1);

    let stopped = reservations::reduce_reservation(
        &reserved,
        ReservationEvent::RequestStop {
            event_id: "stop-1".into(),
            reservation_id: "reservation-1".into(),
            binding: reserved.reservations["reservation-1"].binding(),
            reason: StopReason::LeaseExpiry,
        },
    )
    .expect("stop request preserves unresolved exposure");
    assert_eq!(stopped.unresolved_liabilities[&scope].get(), 1);

    let final_state = reservations::settle_usage(
        &stopped,
        "settle-1",
        "reservation-1",
        MinorUnits::new(1),
        Some(FinalEvidence::SettledAndTerminated {
            evidence_ref: "provider-reconciliation-1".into(),
            reconciliation: reconciliation(&scope),
        }),
    )
    .expect("settle with complete reconciliation evidence");
    assert_eq!(final_state.unresolved_liabilities[&scope].get(), 0);
    assert_eq!(final_state.settled[&scope].get(), 1);

    let corrected = reservations::reduce_reservation(
        &final_state,
        ReservationEvent::CorrectSettlement {
            event_id: "late-correction-1".into(),
            reservation_id: "reservation-1".into(),
            binding: final_state.reservations["reservation-1"].binding(),
            corrected_total: MinorUnits::new(2),
            evidence_ref: "provider-late-correction".into(),
        },
    )
    .expect("append late settlement correction");
    assert_eq!(corrected.settled[&scope].get(), 2);
    assert_eq!(corrected.unresolved_liabilities[&scope].get(), 0);
    assert_eq!(corrected.revision, final_state.revision + 1);
}

#[test]
fn reservation_over_limit_is_rejected_without_changing_liabilities() {
    let scope = reservation_scope();
    let empty = limited_ledger();
    let within_limit = reservations::reserve_budget(
        &empty,
        "reserve-near-limit",
        "reservation-near-limit",
        scope.clone(),
        "correlation-near-limit",
        MinorUnits::new(999),
    )
    .expect("reserve amount below the configured limit");

    let result = reservations::reserve_budget(
        &within_limit,
        "reserve-over-limit",
        "reservation-over-limit",
        scope.clone(),
        "correlation-over-limit",
        MinorUnits::new(2),
    );
    assert!(matches!(
        result,
        Err(reservations::ReservationError::LimitExceeded("account"))
    ));
    assert!(matches!(
        reservations::reserve_budget(
            &within_limit,
            "reserve-over-limit",
            "reservation-over-limit",
            scope.clone(),
            "correlation-over-limit",
            MinorUnits::new(2),
        ),
        Err(reservations::ReservationError::LimitExceeded("account"))
    ));
    assert_eq!(within_limit.unresolved_liabilities[&scope].get(), 999);
    assert!(
        !within_limit
            .reservations
            .contains_key("reservation-over-limit")
    );
    assert_eq!(
        within_limit.revision, 1,
        "rejected reservation must not advance the ledger"
    );
    assert!(
        serde_json::from_str::<MinorUnits>("\"18446744073709551616\"").is_err(),
        "minor-unit values one above u64::MAX must be rejected"
    );
    assert!(
        serde_json::from_str::<MinorUnits>("\"01\"").is_err(),
        "minor-unit values with noncanonical leading zeros must be rejected"
    );
}
