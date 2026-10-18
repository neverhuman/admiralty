//! Pure budget reservation and settlement proposals. This module performs no I/O.
use admiralty_core::{ProjectId, RootId, TaskId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use thiserror::Error;

pub const MAX_JSON_BYTES: usize = 1024 * 1024;
pub const MAX_ITEMS: usize = 1024;
pub const MAX_TEXT_BYTES: usize = 4096;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountId(String);

impl AccountId {
    pub fn new(value: impl Into<String>) -> Result<Self, ReservationError> {
        let value = value.into();
        validate_text("account_id", &value, 128)?;
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CurrencyUnit {
    pub code: String,
    pub scale: u8,
}

impl CurrencyUnit {
    pub fn new(code: impl Into<String>, scale: u8) -> Result<Self, ReservationError> {
        let code = code.into();
        validate_text("currency code", &code, 16)?;
        if !code.bytes().all(|b| b.is_ascii_uppercase()) || scale > 9 {
            return Err(ReservationError::InvalidUnit);
        }
        Ok(Self { code, scale })
    }
}

/// Canonical decimal string protects exact values across JSON/JavaScript boundaries.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct MinorUnits(u64);

impl MinorUnits {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }
    pub const fn get(&self) -> u64 {
        self.0
    }
    pub fn decimal(&self) -> String {
        self.0.to_string()
    }
}
impl TryFrom<String> for MinorUnits {
    type Error = ReservationError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        let parsed = value
            .parse::<u64>()
            .map_err(|_| ReservationError::InvalidDecimal)?;
        if parsed.to_string() != value {
            return Err(ReservationError::InvalidDecimal);
        }
        Ok(Self(parsed))
    }
}
impl From<MinorUnits> for String {
    fn from(value: MinorUnits) -> Self {
        value.decimal()
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReservationScope {
    pub project_id: ProjectId,
    pub root_id: RootId,
    pub task_id: TaskId,
    pub account_id: AccountId,
    pub unit: CurrencyUnit,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScopeLimits {
    pub account: Option<MinorUnits>,
    pub project: Option<MinorUnits>,
    pub root: Option<MinorUnits>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reservation {
    pub id: String,
    pub scope: ReservationScope,
    pub correlation_id: String,
    pub revision: u64,
    pub original: MinorUnits,
    pub settled: MinorUnits,
    pub stop_requested: bool,
    pub outstanding: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReservationLedger {
    pub revision: u64,
    pub limits: BTreeMap<String, ScopeLimits>,
    pub settled: BTreeMap<ReservationScope, MinorUnits>,
    pub unresolved_liabilities: BTreeMap<ReservationScope, MinorUnits>,
    pub reservations: BTreeMap<String, Reservation>,
    pub processed_events: BTreeMap<String, String>,
}

impl ReservationLedger {
    pub fn new() -> Self {
        Self {
            revision: 0,
            limits: BTreeMap::new(),
            settled: BTreeMap::new(),
            unresolved_liabilities: BTreeMap::new(),
            reservations: BTreeMap::new(),
            processed_events: BTreeMap::new(),
        }
    }
}
impl Default for ReservationLedger {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReservationEvent {
    Reserve {
        event_id: String,
        reservation_id: String,
        scope: ReservationScope,
        correlation_id: String,
        amount: MinorUnits,
    },
    RequestStop {
        event_id: String,
        reservation_id: String,
        reason: StopReason,
    },
    Settle {
        event_id: String,
        reservation_id: String,
        amount: MinorUnits,
        final_evidence: Option<FinalEvidence>,
    },
    CorrectSettlement {
        event_id: String,
        reservation_id: String,
        corrected_total: MinorUnits,
        evidence_ref: String,
    },
    SetLimits {
        event_id: String,
        account_id: AccountId,
        limits: ScopeLimits,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    Cancellation,
    LeaseExpiry,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum FinalEvidence {
    SettledAndTerminated { evidence_ref: String },
    ConfirmedNoUsage { evidence_ref: String },
}

#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub enum ReservationError {
    #[error("invalid or oversized {0}")]
    InvalidText(&'static str),
    #[error("invalid currency/unit")]
    InvalidUnit,
    #[error("counter must be a canonical decimal u64 string")]
    InvalidDecimal,
    #[error("event ID was reused with a different payload")]
    EventCollision,
    #[error("reservation already exists with different data")]
    ReservationCollision,
    #[error("unknown reservation {0}")]
    UnknownReservation(String),
    #[error("unknown limit or exposure prevents safe admission")]
    UnknownExposure,
    #[error("reservation exceeds {0} budget limit")]
    LimitExceeded(&'static str),
    #[error("currency or quota units cannot be combined")]
    WrongUnit,
    #[error("checked arithmetic overflow")]
    Overflow,
    #[error("invalid settlement or evidence")]
    InvalidSettlement,
    #[error("revision overflow")]
    RevisionOverflow,
    #[error("input exceeds the bounded size or collection limit")]
    TooLarge,
}

fn validate_text(name: &'static str, value: &str, max: usize) -> Result<(), ReservationError> {
    if value.trim().is_empty() || value.len() > max || value.chars().any(char::is_control) {
        return Err(ReservationError::InvalidText(name));
    }
    Ok(())
}
fn checked_add(a: u64, b: u64) -> Result<u64, ReservationError> {
    a.checked_add(b).ok_or(ReservationError::Overflow)
}
fn validate_event(event: &ReservationEvent) -> Result<(), ReservationError> {
    let id = match event {
        ReservationEvent::Reserve { event_id, .. }
        | ReservationEvent::RequestStop { event_id, .. }
        | ReservationEvent::Settle { event_id, .. }
        | ReservationEvent::SetLimits { event_id, .. }
        | ReservationEvent::CorrectSettlement { event_id, .. } => event_id,
    };
    validate_text("event_id", id, 128)?;
    let json = serde_json::to_vec(event).map_err(|_| ReservationError::TooLarge)?;
    if json.len() > MAX_JSON_BYTES {
        return Err(ReservationError::TooLarge);
    }
    match event {
        ReservationEvent::Reserve {
            reservation_id,
            correlation_id,
            scope,
            ..
        } => {
            validate_text("reservation_id", reservation_id, 128)?;
            validate_text("correlation_id", correlation_id, 128)?;
            validate_text("account_id", scope.account_id.as_str(), 128)?;
        }
        ReservationEvent::Settle {
            amount,
            final_evidence: Some(evidence),
            ..
        } => {
            let evidence_ref = match evidence {
                FinalEvidence::SettledAndTerminated { evidence_ref }
                | FinalEvidence::ConfirmedNoUsage { evidence_ref } => evidence_ref,
            };
            validate_text("evidence_ref", evidence_ref, MAX_TEXT_BYTES)?;
            if matches!(evidence, FinalEvidence::ConfirmedNoUsage { .. }) && amount.get() != 0 {
                return Err(ReservationError::InvalidSettlement);
            }
        }
        ReservationEvent::CorrectSettlement { evidence_ref, .. } => {
            validate_text("evidence_ref", evidence_ref, MAX_TEXT_BYTES)?
        }
        _ => {}
    }
    Ok(())
}

/// Reduce one event atomically. Duplicate bytes return the unchanged ledger.
pub fn reduce_reservation(
    state: &ReservationLedger,
    event: ReservationEvent,
) -> Result<ReservationLedger, ReservationError> {
    validate_event(&event)?;
    if state.reservations.len() > MAX_ITEMS
        || state.processed_events.len() > MAX_ITEMS
        || state.settled.len() > MAX_ITEMS
        || state.unresolved_liabilities.len() > MAX_ITEMS
        || state.limits.len() > MAX_ITEMS
    {
        return Err(ReservationError::TooLarge);
    }
    if serde_json::to_vec(&(
        state.revision,
        &state.limits,
        state.settled.iter().collect::<Vec<_>>(),
        state.unresolved_liabilities.iter().collect::<Vec<_>>(),
        &state.reservations,
        &state.processed_events,
    ))
    .map_err(|_| ReservationError::TooLarge)?
    .len()
        > MAX_JSON_BYTES
    {
        return Err(ReservationError::TooLarge);
    }
    let event_id = match &event {
        ReservationEvent::Reserve { event_id, .. }
        | ReservationEvent::RequestStop { event_id, .. }
        | ReservationEvent::Settle { event_id, .. }
        | ReservationEvent::SetLimits { event_id, .. }
        | ReservationEvent::CorrectSettlement { event_id, .. } => event_id.clone(),
    };
    let digest = admiralty_core::content_digest(&event).map_err(|_| ReservationError::TooLarge)?;
    if let Some(prior) = state.processed_events.get(&event_id) {
        return if prior == &digest {
            Ok(state.clone())
        } else {
            Err(ReservationError::EventCollision)
        };
    }
    let mut next = state.clone();
    match event {
        ReservationEvent::SetLimits {
            account_id, limits, ..
        } => {
            next.limits.insert(account_id.0, limits);
        }
        ReservationEvent::Reserve {
            reservation_id,
            scope,
            correlation_id,
            amount,
            ..
        } => {
            if next.reservations.contains_key(&reservation_id) {
                return Err(ReservationError::ReservationCollision);
            }
            let limits = next
                .limits
                .get(scope.account_id.as_str())
                .ok_or(ReservationError::UnknownExposure)?;
            let account_total = total_for(&next, &scope, |s| s.account_id == scope.account_id)?;
            let project_total = total_for(&next, &scope, |s| {
                s.account_id == scope.account_id && s.project_id == scope.project_id
            })?;
            let root_total = total_for(&next, &scope, |s| {
                s.account_id == scope.account_id
                    && s.project_id == scope.project_id
                    && s.root_id == scope.root_id
            })?;
            check_limit(
                "account",
                limits.account.as_ref(),
                account_total,
                amount.get(),
            )?;
            check_limit(
                "project",
                limits.project.as_ref(),
                project_total,
                amount.get(),
            )?;
            check_limit("root", limits.root.as_ref(), root_total, amount.get())?;
            next.unresolved_liabilities.insert(
                scope.clone(),
                MinorUnits::new(checked_add(
                    next.unresolved_liabilities
                        .get(&scope)
                        .map_or(0, MinorUnits::get),
                    amount.get(),
                )?),
            );
            next.reservations.insert(
                reservation_id.clone(),
                Reservation {
                    id: reservation_id,
                    scope,
                    correlation_id,
                    revision: next
                        .revision
                        .checked_add(1)
                        .ok_or(ReservationError::RevisionOverflow)?,
                    original: amount,
                    settled: MinorUnits::new(0),
                    stop_requested: false,
                    outstanding: true,
                },
            );
        }
        ReservationEvent::RequestStop { reservation_id, .. } => {
            let item = next
                .reservations
                .get_mut(&reservation_id)
                .ok_or_else(|| ReservationError::UnknownReservation(reservation_id.clone()))?;
            item.stop_requested = true;
        }
        ReservationEvent::Settle {
            reservation_id,
            amount,
            final_evidence,
            ..
        } => {
            let item = next
                .reservations
                .get_mut(&reservation_id)
                .ok_or_else(|| ReservationError::UnknownReservation(reservation_id.clone()))?;
            if !item.outstanding
                || amount.get()
                    > item
                        .original
                        .get()
                        .checked_sub(item.settled.get())
                        .ok_or(ReservationError::Overflow)?
            {
                return Err(ReservationError::InvalidSettlement);
            }
            let scope = item.scope.clone();
            let new_settled = checked_add(item.settled.get(), amount.get())?;
            item.settled = MinorUnits::new(new_settled);
            let settled = checked_add(
                next.settled.get(&scope).map_or(0, MinorUnits::get),
                amount.get(),
            )?;
            next.settled.insert(scope.clone(), MinorUnits::new(settled));
            let remaining = item
                .original
                .get()
                .checked_sub(new_settled)
                .ok_or(ReservationError::Overflow)?;
            let prior_liability = next
                .unresolved_liabilities
                .get(&scope)
                .map_or(0, MinorUnits::get);
            let liability = prior_liability
                .checked_sub(amount.get())
                .ok_or(ReservationError::Overflow)?;
            if final_evidence.is_some() {
                item.outstanding = false;
                next.unresolved_liabilities.insert(
                    scope,
                    MinorUnits::new(
                        liability
                            .checked_sub(remaining)
                            .ok_or(ReservationError::Overflow)?,
                    ),
                );
            } else {
                next.unresolved_liabilities
                    .insert(scope, MinorUnits::new(liability));
            }
        }
        ReservationEvent::CorrectSettlement {
            reservation_id,
            corrected_total,
            ..
        } => {
            let item = next
                .reservations
                .get_mut(&reservation_id)
                .ok_or_else(|| ReservationError::UnknownReservation(reservation_id.clone()))?;
            if item.outstanding || corrected_total.get() > item.original.get() {
                return Err(ReservationError::InvalidSettlement);
            }
            let scope = item.scope.clone();
            let aggregate = next
                .settled
                .get(&scope)
                .map_or(0, MinorUnits::get)
                .checked_sub(item.settled.get())
                .ok_or(ReservationError::Overflow)?;
            next.settled.insert(
                scope,
                MinorUnits::new(checked_add(aggregate, corrected_total.get())?),
            );
            item.settled = corrected_total;
        }
    }
    next.revision = next
        .revision
        .checked_add(1)
        .ok_or(ReservationError::RevisionOverflow)?;
    next.processed_events.insert(event_id, digest);
    Ok(next)
}

fn total_for<F>(
    ledger: &ReservationLedger,
    scope: &ReservationScope,
    matches: F,
) -> Result<u64, ReservationError>
where
    F: Fn(&ReservationScope) -> bool,
{
    let mut total = 0_u64;
    for (item_scope, amount) in &ledger.settled {
        if matches(item_scope) {
            if item_scope.unit != scope.unit {
                return Err(ReservationError::WrongUnit);
            }
            total = checked_add(total, amount.get())?;
        }
    }
    for (item_scope, amount) in &ledger.unresolved_liabilities {
        if matches(item_scope) {
            if item_scope.unit != scope.unit {
                return Err(ReservationError::WrongUnit);
            }
            total = checked_add(total, amount.get())?;
        }
    }
    Ok(total)
}
fn check_limit(
    name: &'static str,
    limit: Option<&MinorUnits>,
    current: u64,
    request: u64,
) -> Result<(), ReservationError> {
    let cap = limit.ok_or(ReservationError::UnknownExposure)?;
    if checked_add(current, request)? > cap.get() {
        return Err(ReservationError::LimitExceeded(name));
    }
    Ok(())
}

/// Convenience proposal builder; callers still apply it with their expected ledger revision.
pub fn reserve_budget(
    state: &ReservationLedger,
    event_id: impl Into<String>,
    reservation_id: impl Into<String>,
    scope: ReservationScope,
    correlation_id: impl Into<String>,
    amount: MinorUnits,
) -> Result<ReservationLedger, ReservationError> {
    reduce_reservation(
        state,
        ReservationEvent::Reserve {
            event_id: event_id.into(),
            reservation_id: reservation_id.into(),
            scope,
            correlation_id: correlation_id.into(),
            amount,
        },
    )
}

/// Convenience settlement proposal builder.
pub fn settle_usage(
    state: &ReservationLedger,
    event_id: impl Into<String>,
    reservation_id: impl Into<String>,
    amount: MinorUnits,
    final_evidence: Option<FinalEvidence>,
) -> Result<ReservationLedger, ReservationError> {
    reduce_reservation(
        state,
        ReservationEvent::Settle {
            event_id: event_id.into(),
            reservation_id: reservation_id.into(),
            amount,
            final_evidence,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn scope() -> ReservationScope {
        ReservationScope {
            project_id: ProjectId::new("p").expect("valid project fixture"),
            root_id: RootId::new("r").expect("valid root fixture"),
            task_id: TaskId::new("t").expect("valid task fixture"),
            account_id: AccountId::new("acct").expect("valid account fixture"),
            unit: CurrencyUnit::new("USD", 2).expect("valid unit fixture"),
        }
    }
    fn limited() -> ReservationLedger {
        let mut l = ReservationLedger::new();
        l.limits.insert(
            "acct".into(),
            ScopeLimits {
                account: Some(MinorUnits::new(100)),
                project: Some(MinorUnits::new(100)),
                root: Some(MinorUnits::new(100)),
            },
        );
        l
    }
    fn reserve(l: &ReservationLedger, id: &str, amount: u64) -> ReservationLedger {
        reserve_budget(
            l,
            format!("ev-{id}"),
            id,
            scope(),
            format!("corr-{id}"),
            MinorUnits::new(amount),
        )
        .expect("reservation fixture succeeds")
    }
    #[test]
    fn scoped_limits_and_serialized_cas_proposals() {
        let l = limited();
        let a = reserve(&l, "a", 60);
        assert!(matches!(
            reserve_budget(&a, "ev-b", "b", scope(), "cb", MinorUnits::new(50)),
            Err(ReservationError::LimitExceeded("account"))
        ));
        assert_eq!(a.revision, 1);
        assert_eq!(reserve(&l, "b", 50).revision, 1);
    }
    #[test]
    fn replay_collision_stop_and_partial_final_conserve_liability() {
        let l = limited();
        let l = reserve(&l, "a", 50);
        let replay = reduce_reservation(
            &l,
            ReservationEvent::Reserve {
                event_id: "ev-a".into(),
                reservation_id: "a".into(),
                scope: scope(),
                correlation_id: "corr-a".into(),
                amount: MinorUnits::new(50),
            },
        )
        .expect("exact replay");
        assert_eq!(l, replay);
        assert!(matches!(
            reserve_budget(&l, "ev-a", "a", scope(), "different", MinorUnits::new(50)),
            Err(ReservationError::EventCollision)
        ));
        let stopped = reduce_reservation(
            &l,
            ReservationEvent::RequestStop {
                event_id: "cancel".into(),
                reservation_id: "a".into(),
                reason: StopReason::Cancellation,
            },
        )
        .expect("stop request");
        assert_eq!(
            stopped
                .unresolved_liabilities
                .get(&scope())
                .expect("liability retained")
                .get(),
            50
        );
        let partial = settle_usage(&stopped, "partial", "a", MinorUnits::new(20), None)
            .expect("partial settlement");
        assert_eq!(
            partial
                .unresolved_liabilities
                .get(&scope())
                .expect("remaining liability")
                .get(),
            30
        );
        let final_state = settle_usage(
            &partial,
            "final",
            "a",
            MinorUnits::new(10),
            Some(FinalEvidence::SettledAndTerminated {
                evidence_ref: "receipt".into(),
            }),
        )
        .expect("final settlement");
        assert_eq!(
            final_state
                .unresolved_liabilities
                .get(&scope())
                .expect("remaining unsettled amount")
                .get(),
            0
        );
        assert_eq!(
            final_state
                .settled
                .get(&scope())
                .expect("settled amount")
                .get(),
            30
        );
    }
    #[test]
    fn canonical_large_count_and_bad_decimal() {
        let n =
            MinorUnits::try_from(String::from("9007199254740993")).expect("canonical large count");
        assert_eq!(n.decimal(), "9007199254740993");
        assert!(MinorUnits::try_from(String::from("01")).is_err());
    }
    #[test]
    fn unknown_limit_wrong_unit_and_overflow_block() {
        let unknown = ReservationLedger::new();
        assert!(matches!(
            reserve_budget(&unknown, "e", "r", scope(), "c", MinorUnits::new(1)),
            Err(ReservationError::UnknownExposure)
        ));
        let mut l = limited();
        l.settled.insert(scope(), MinorUnits::new(u64::MAX));
        assert!(matches!(
            reserve_budget(&l, "e", "r", scope(), "c", MinorUnits::new(1)),
            Err(ReservationError::Overflow)
        ));
    }

    #[test]
    fn expiry_keeps_unknown_exposure_and_late_correction_is_new_revision() {
        let l = reserve(&limited(), "late", 40);
        let expired = reduce_reservation(
            &l,
            ReservationEvent::RequestStop {
                event_id: "expiry".into(),
                reservation_id: "late".into(),
                reason: StopReason::LeaseExpiry,
            },
        )
        .expect("expiry requests stop");
        assert_eq!(
            expired
                .unresolved_liabilities
                .get(&scope())
                .expect("held exposure")
                .get(),
            40
        );
        let settled = settle_usage(
            &expired,
            "settle",
            "late",
            MinorUnits::new(10),
            Some(FinalEvidence::SettledAndTerminated {
                evidence_ref: "independent".into(),
            }),
        )
        .expect("final settlement");
        let corrected = reduce_reservation(
            &settled,
            ReservationEvent::CorrectSettlement {
                event_id: "correction".into(),
                reservation_id: "late".into(),
                corrected_total: MinorUnits::new(12),
                evidence_ref: "provider-correction".into(),
            },
        )
        .expect("late correction is appended");
        assert_eq!(corrected.revision, settled.revision + 1);
        assert_eq!(
            corrected
                .settled
                .get(&scope())
                .expect("corrected settled total")
                .get(),
            12
        );
    }

    #[test]
    fn zero_and_tiny_amounts_are_exact_and_units_never_mix() {
        let l = reserve(&limited(), "zero", 0);
        let l = reserve(&l, "tiny", 1);
        assert_eq!(
            l.unresolved_liabilities
                .get(&scope())
                .expect("tiny exposure")
                .get(),
            1
        );
        let mut other = scope();
        other.unit = CurrencyUnit::new("EUR", 2).expect("different currency fixture");
        let mut ledger = limited();
        ledger.settled.insert(other, MinorUnits::new(1));
        assert!(matches!(
            reserve_budget(&ledger, "unit", "u", scope(), "c", MinorUnits::new(1)),
            Err(ReservationError::WrongUnit)
        ));
    }
}
