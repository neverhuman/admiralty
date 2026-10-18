//! Pure budget reservation and settlement proposals. This module performs no I/O.
use admiralty_core::{
    AttemptId, PrincipalId, ProjectId, RootId, TaskId, content_digest, limits::bounded_json_size,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use thiserror::Error;

pub const MAX_JSON_BYTES: usize = 1024 * 1024;
pub const MAX_ITEMS: usize = 1024;
pub const MAX_TEXT_BYTES: usize = 4096;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
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
impl TryFrom<String> for AccountId {
    type Error = ReservationError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}
impl From<AccountId> for String {
    fn from(value: AccountId) -> Self {
        value.0
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
        if value.len() > 20 {
            return Err(ReservationError::InvalidDecimal);
        }
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
    pub attempt_id: AttemptId,
    pub owner: PrincipalId,
    pub incarnation: String,
    pub contract_revision: u32,
    pub contract_digest: String,
    pub authority_ref: String,
    pub resource_claims_digest: String,
    pub account_id: AccountId,
    pub unit: CurrencyUnit,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScopeLimits {
    pub unit: CurrencyUnit,
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
    #[serde(with = "decimal_revision")]
    pub revision: u64,
    pub original: MinorUnits,
    pub settled: MinorUnits,
    pub stop_requested: bool,
    pub outstanding: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReservationBinding {
    pub scope: ReservationScope,
    pub correlation_id: String,
    #[serde(with = "decimal_revision")]
    pub expected_revision: u64,
}
impl Reservation {
    pub fn binding(&self) -> ReservationBinding {
        ReservationBinding {
            scope: self.scope.clone(),
            correlation_id: self.correlation_id.clone(),
            expected_revision: self.revision,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReconciliationEvidence {
    pub scope_digest: String,
    pub resource_claims_digest: String,
    pub usage_complete: Option<bool>,
    pub effects_reconciled: Option<bool>,
    pub resource_claims_reconciled: Option<bool>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReservationLedger {
    #[serde(with = "decimal_revision")]
    pub revision: u64,
    pub limits: BTreeMap<String, ScopeLimits>,
    #[serde(with = "scope_amounts")]
    pub settled: BTreeMap<ReservationScope, MinorUnits>,
    #[serde(with = "scope_amounts")]
    pub unresolved_liabilities: BTreeMap<ReservationScope, MinorUnits>,
    pub reservations: BTreeMap<String, Reservation>,
    pub processed_events: BTreeMap<String, String>,
}

mod decimal_revision {
    use super::MinorUnits;
    use serde::{Deserialize, Serialize};
    pub fn serialize<S: serde::Serializer>(value: &u64, serializer: S) -> Result<S::Ok, S::Error> {
        MinorUnits::new(*value).serialize(serializer)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
        MinorUnits::deserialize(deserializer).map(|value| value.get())
    }
}
mod scope_amounts {
    use super::{MAX_ITEMS, MinorUnits, ReservationScope};
    use serde::Deserialize;
    use std::collections::BTreeMap;
    pub fn serialize<S: serde::Serializer>(
        value: &BTreeMap<ReservationScope, MinorUnits>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(value.iter())
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<BTreeMap<ReservationScope, MinorUnits>, D::Error> {
        let entries = Vec::<(ReservationScope, MinorUnits)>::deserialize(deserializer)?;
        if entries.len() > MAX_ITEMS {
            return Err(serde::de::Error::custom(
                "scope amount collection exceeds bound",
            ));
        }
        let mut result = BTreeMap::new();
        for (scope, amount) in entries {
            if result.insert(scope, amount).is_some() {
                return Err(serde::de::Error::custom("duplicate scope amount"));
            }
        }
        Ok(result)
    }
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
        binding: ReservationBinding,
        reason: StopReason,
    },
    Settle {
        event_id: String,
        reservation_id: String,
        binding: ReservationBinding,
        amount: MinorUnits,
        final_evidence: Option<FinalEvidence>,
    },
    CorrectSettlement {
        event_id: String,
        reservation_id: String,
        binding: ReservationBinding,
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
    SettledAndTerminated {
        evidence_ref: String,
        reconciliation: ReconciliationEvidence,
    },
    ConfirmedNoUsage {
        evidence_ref: String,
        reconciliation: ReconciliationEvidence,
    },
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
    #[error("scope, attempt, owner, correlation or revision mismatch")]
    BindingMismatch,
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
fn digest_valid(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    })
}
fn validate_scope(scope: &ReservationScope) -> Result<(), ReservationError> {
    AccountId::new(scope.account_id.as_str())?;
    CurrencyUnit::new(&scope.unit.code, scope.unit.scale)?;
    if scope.contract_revision == 0
        || PrincipalId::new(&scope.incarnation).is_err()
        || !digest_valid(&scope.contract_digest)
        || !digest_valid(&scope.resource_claims_digest)
        || scope.authority_ref.len() > 512
        || scope.authority_ref.is_empty()
        || scope
            .authority_ref
            .chars()
            .any(|ch| ch.is_control() || ch.is_whitespace())
    {
        return Err(ReservationError::BindingMismatch);
    }
    Ok(())
}
fn validate_ledger(state: &ReservationLedger) -> Result<(), ReservationError> {
    if [
        state.reservations.len(),
        state.processed_events.len(),
        state.settled.len(),
        state.unresolved_liabilities.len(),
        state.limits.len(),
    ]
    .iter()
    .any(|count| *count > MAX_ITEMS)
    {
        return Err(ReservationError::TooLarge);
    }
    bounded_json_size(state, MAX_JSON_BYTES).map_err(|_| ReservationError::TooLarge)?;
    for (account, limits) in &state.limits {
        AccountId::new(account)?;
        CurrencyUnit::new(&limits.unit.code, limits.unit.scale)?;
    }
    for scope in state
        .settled
        .keys()
        .chain(state.unresolved_liabilities.keys())
    {
        validate_scope(scope)?;
    }
    let mut settled = BTreeMap::<ReservationScope, u64>::new();
    let mut held = BTreeMap::<ReservationScope, u64>::new();
    for (id, item) in &state.reservations {
        validate_text("reservation_id", id, 128)?;
        validate_text("correlation_id", &item.correlation_id, 128)?;
        validate_scope(&item.scope)?;
        if id != &item.id || item.revision == 0 || item.revision > state.revision {
            return Err(ReservationError::BindingMismatch);
        }
        let total = settled.entry(item.scope.clone()).or_default();
        *total = checked_add(*total, item.settled.get())?;
        if item.outstanding {
            let total = held.entry(item.scope.clone()).or_default();
            *total = checked_add(
                *total,
                item.original.get().saturating_sub(item.settled.get()),
            )?;
        }
    }
    if settled
        .iter()
        .any(|(scope, minimum)| state.settled.get(scope).map_or(0, MinorUnits::get) < *minimum)
        || held.iter().any(|(scope, minimum)| {
            state
                .unresolved_liabilities
                .get(scope)
                .map_or(0, MinorUnits::get)
                < *minimum
        })
    {
        return Err(ReservationError::UnknownExposure);
    }
    for (id, digest) in &state.processed_events {
        validate_text("event_id", id, 128)?;
        if !digest_valid(digest) {
            return Err(ReservationError::EventCollision);
        }
    }
    Ok(())
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
    bounded_json_size(event, MAX_JSON_BYTES).map_err(|_| ReservationError::TooLarge)?;
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
            validate_scope(scope)?;
        }
        ReservationEvent::Settle {
            amount,
            final_evidence: Some(evidence),
            ..
        } => {
            let evidence_ref = match evidence {
                FinalEvidence::SettledAndTerminated { evidence_ref, .. }
                | FinalEvidence::ConfirmedNoUsage { evidence_ref, .. } => evidence_ref,
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
    validate_ledger(state)?;
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
    if let ReservationEvent::RequestStop {
        reservation_id,
        binding,
        ..
    }
    | ReservationEvent::Settle {
        reservation_id,
        binding,
        ..
    }
    | ReservationEvent::CorrectSettlement {
        reservation_id,
        binding,
        ..
    } = &event
    {
        validate_text("reservation_id", reservation_id, 128)?;
        let item = state
            .reservations
            .get(reservation_id)
            .ok_or_else(|| ReservationError::UnknownReservation(reservation_id.clone()))?;
        if binding != &item.binding() {
            return Err(ReservationError::BindingMismatch);
        }
    }
    let changed_reservation = match &event {
        ReservationEvent::Reserve { reservation_id, .. }
        | ReservationEvent::RequestStop { reservation_id, .. }
        | ReservationEvent::Settle { reservation_id, .. }
        | ReservationEvent::CorrectSettlement { reservation_id, .. } => {
            Some(reservation_id.clone())
        }
        ReservationEvent::SetLimits { .. } => None,
    };
    let mut next = state.clone();
    match event {
        ReservationEvent::SetLimits {
            account_id, limits, ..
        } => {
            AccountId::new(account_id.as_str())?;
            CurrencyUnit::new(&limits.unit.code, limits.unit.scale)?;
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
            if limits.unit != scope.unit {
                return Err(ReservationError::WrongUnit);
            }
            if next.reservations.values().any(|item| {
                item.scope.account_id == scope.account_id
                    && item.outstanding
                    && item.original.get() > 0
                    && item.settled.get() >= item.original.get()
            }) {
                return Err(ReservationError::UnknownExposure);
            }
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
            if !item.outstanding {
                return Err(ReservationError::InvalidSettlement);
            }
            if let Some(evidence) = &final_evidence {
                let reconciliation = match evidence {
                    FinalEvidence::SettledAndTerminated { reconciliation, .. }
                    | FinalEvidence::ConfirmedNoUsage { reconciliation, .. } => reconciliation,
                };
                let scope_digest =
                    content_digest(&item.scope).map_err(|_| ReservationError::TooLarge)?;
                if reconciliation.scope_digest != scope_digest
                    || reconciliation.resource_claims_digest != item.scope.resource_claims_digest
                    || reconciliation.usage_complete != Some(true)
                    || reconciliation.effects_reconciled != Some(true)
                    || reconciliation.resource_claims_reconciled != Some(true)
                    || (matches!(evidence, FinalEvidence::ConfirmedNoUsage { .. })
                        && item.settled.get() != 0)
                {
                    return Err(ReservationError::InvalidSettlement);
                }
            }
            let scope = item.scope.clone();
            let prior_remaining = item.original.get().saturating_sub(item.settled.get());
            let new_settled = checked_add(item.settled.get(), amount.get())?;
            item.settled = MinorUnits::new(new_settled);
            let settled = checked_add(
                next.settled.get(&scope).map_or(0, MinorUnits::get),
                amount.get(),
            )?;
            next.settled.insert(scope.clone(), MinorUnits::new(settled));
            let remaining = item.original.get().saturating_sub(new_settled);
            let prior_liability = next
                .unresolved_liabilities
                .get(&scope)
                .map_or(0, MinorUnits::get);
            let liability = prior_liability
                .checked_sub(amount.get().min(prior_remaining))
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
            if item.outstanding {
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
    if let Some(id) = changed_reservation {
        next.reservations
            .get_mut(&id)
            .expect("admitted reservation exists")
            .revision = next.revision;
    }
    next.processed_events.insert(event_id, digest);
    validate_ledger(&next)?;
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
    let reservation_id = reservation_id.into();
    let binding = state
        .reservations
        .get(&reservation_id)
        .ok_or_else(|| ReservationError::UnknownReservation(reservation_id.clone()))?
        .binding();
    reduce_reservation(
        state,
        ReservationEvent::Settle {
            event_id: event_id.into(),
            reservation_id,
            binding,
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
            attempt_id: AttemptId::new("attempt").expect("fixture attempt"),
            owner: PrincipalId::new("owner").expect("fixture owner"),
            incarnation: "boot-1".into(),
            contract_revision: 1,
            contract_digest: format!("sha256:{}", "a".repeat(64)),
            authority_ref: "grant:budget".into(),
            resource_claims_digest: format!("sha256:{}", "b".repeat(64)),
            account_id: AccountId::new("acct").expect("valid account fixture"),
            unit: CurrencyUnit::new("USD", 2).expect("valid unit fixture"),
        }
    }
    fn reconciled() -> ReconciliationEvidence {
        ReconciliationEvidence {
            scope_digest: content_digest(&scope()).expect("fixture scope digest"),
            resource_claims_digest: scope().resource_claims_digest,
            usage_complete: Some(true),
            effects_reconciled: Some(true),
            resource_claims_reconciled: Some(true),
        }
    }
    fn limited() -> ReservationLedger {
        let mut l = ReservationLedger::new();
        l.limits.insert(
            "acct".into(),
            ScopeLimits {
                unit: CurrencyUnit::new("USD", 2).expect("fixture limit unit"),
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
                binding: l.reservations["a"].binding(),
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
                reconciliation: reconciled(),
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
                binding: l.reservations["late"].binding(),
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
                reconciliation: reconciled(),
            }),
        )
        .expect("final settlement");
        let corrected = reduce_reservation(
            &settled,
            ReservationEvent::CorrectSettlement {
                event_id: "correction".into(),
                reservation_id: "late".into(),
                binding: settled.reservations["late"].binding(),
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
    #[test]
    fn stale_or_cross_attempt_bindings_and_first_unit_substitution_are_rejected() {
        let state = reserve(&limited(), "bound", 20);
        for mutation in ["attempt", "owner", "revision", "incarnation"] {
            let mut binding = state.reservations["bound"].binding();
            match mutation {
                "attempt" => {
                    binding.scope.attempt_id = AttemptId::new("other").expect("fixture attempt")
                }
                "owner" => binding.scope.owner = PrincipalId::new("other").expect("fixture owner"),
                "revision" => binding.expected_revision += 1,
                _ => binding.scope.incarnation = "other-boot".into(),
            }
            let event = ReservationEvent::RequestStop {
                event_id: format!("bad-{mutation}"),
                reservation_id: "bound".into(),
                binding,
                reason: StopReason::Cancellation,
            };
            assert_eq!(
                reduce_reservation(&state, event),
                Err(ReservationError::BindingMismatch)
            );
        }
        let mut other = scope();
        other.unit = CurrencyUnit::new("TOKENS", 0).expect("native unit");
        assert_eq!(
            reserve_budget(
                &limited(),
                "wrong-unit",
                "r",
                other,
                "c",
                MinorUnits::new(1)
            ),
            Err(ReservationError::WrongUnit)
        );
        assert!(serde_json::from_str::<AccountId>("\"\" ").is_err());
    }
    #[test]
    fn measured_overrun_is_retained_and_ambiguous_resource_effects_never_free_capacity() {
        let mut initial = limited();
        initial
            .limits
            .get_mut("acct")
            .expect("fixture limits")
            .account = Some(MinorUnits::new(1000));
        initial
            .limits
            .get_mut("acct")
            .expect("fixture limits")
            .project = Some(MinorUnits::new(1000));
        initial.limits.get_mut("acct").expect("fixture limits").root = Some(MinorUnits::new(1000));
        let state = reserve(&initial, "overrun", 40);
        let measured = settle_usage(&state, "measured", "overrun", MinorUnits::new(60), None)
            .expect("actual usage may exceed forecast");
        assert_eq!(measured.settled[&scope()].get(), 60);
        assert!(measured.reservations["overrun"].outstanding);
        assert_eq!(
            reserve_budget(
                &measured,
                "next",
                "next",
                scope(),
                "next",
                MinorUnits::new(1)
            ),
            Err(ReservationError::UnknownExposure)
        );
        for field in ["usage", "effects", "resources"] {
            let mut evidence = reconciled();
            match field {
                "usage" => evidence.usage_complete = None,
                "effects" => evidence.effects_reconciled = Some(false),
                _ => evidence.resource_claims_reconciled = None,
            }
            assert!(
                settle_usage(
                    &measured,
                    format!("bad-{field}"),
                    "overrun",
                    MinorUnits::new(0),
                    Some(FinalEvidence::SettledAndTerminated {
                        evidence_ref: "claim".into(),
                        reconciliation: evidence
                    })
                )
                .is_err()
            );
        }
        let closed = settle_usage(
            &measured,
            "closed",
            "overrun",
            MinorUnits::new(0),
            Some(FinalEvidence::SettledAndTerminated {
                evidence_ref: "qualified-reconciliation-required".into(),
                reconciliation: reconciled(),
            }),
        )
        .expect("complete scoped evidence proposal");
        assert!(!closed.reservations["overrun"].outstanding);
        assert!(
            reserve_budget(&closed, "next", "next", scope(), "next", MinorUnits::new(1)).is_ok()
        );
        assert!(
            settle_usage(
                &measured,
                "no-usage",
                "overrun",
                MinorUnits::new(0),
                Some(FinalEvidence::ConfirmedNoUsage {
                    evidence_ref: "contradiction".into(),
                    reconciliation: reconciled()
                })
            )
            .is_err()
        );
    }
    #[test]
    fn understated_liabilities_and_full_history_fail_without_silent_release() {
        let mut understated = reserve(&limited(), "held", 30);
        understated
            .unresolved_liabilities
            .insert(scope(), MinorUnits::new(0));
        assert_eq!(
            reserve_budget(&understated, "new", "new", scope(), "c", MinorUnits::new(1)),
            Err(ReservationError::UnknownExposure)
        );
        let mut full = limited();
        for index in 0..MAX_ITEMS {
            full.processed_events
                .insert(format!("old-{index}"), format!("sha256:{}", "a".repeat(64)));
        }
        assert_eq!(
            reserve_budget(&full, "new", "new", scope(), "c", MinorUnits::new(1)),
            Err(ReservationError::TooLarge)
        );
        assert_eq!(full.processed_events.len(), MAX_ITEMS);
    }
    #[test]
    fn ledger_scope_maps_round_trip_and_revisions_preserve_javascript_unsafe_integers() {
        let mut ledger = reserve(&limited(), "exact", 30);
        ledger.revision = 9_007_199_254_740_993;
        ledger
            .reservations
            .get_mut("exact")
            .expect("fixture reservation")
            .revision = ledger.revision;
        let value = serde_json::to_value(&ledger)
            .expect("non-string scope maps have a portable tuple representation");
        assert_eq!(value["revision"], serde_json::json!("9007199254740993"));
        assert_eq!(
            serde_json::from_value::<ReservationLedger>(value.clone())
                .expect("exact bounded projection round trip"),
            ledger
        );
        let mut duplicate = value;
        let entries = duplicate["unresolved_liabilities"]
            .as_array_mut()
            .expect("scope tuples");
        entries.push(entries[0].clone());
        assert!(serde_json::from_value::<ReservationLedger>(duplicate).is_err());
    }
}
