//! Opt-in process-local admission epoch/rule coherence through a Store commit.
//!
//! Only the concrete paired cohort's own immutable rules and epoch participate.
//! A generic fence cannot stabilize unrelated Boolean policies, credentials,
//! physical allocations, or later dispatch/start. No authenticated catalog
//! loader, production issuer, persistent authority, or default activation exists.
use crate::{
    LifecycleEffect, StoreAuthorities,
    admission_reservations::{
        self, ReservationAuthority, ReservationBinding, ReservationCommand, ReservationProof,
    },
    inference_slots::{
        self, InferenceSlotAuthority, SlotBinding, SlotCommand, SlotEvent, SlotProof,
    },
};
use admiralty_core::limits::bounded_json_size;
use admiralty_core::{DomainEvent, SchedulingState, SourceIdentity, TaskView, content_digest};
use admiralty_runtime::reservations::{ReservationEvent, ReservationLedger, ReservationScope};
use std::{
    any::Any,
    collections::BTreeMap,
    sync::{Arc, RwLock, RwLockReadGuard, TryLockError},
};

const MAX_RULES: usize = 1024;
const MAX_EVENT_BYTES: usize = 1024 * 1024;
const MAX_EFFECT_BYTES: usize = 64 * 1024;

pub struct AdmissionFenceBinding<'a> {
    pub source: &'a SourceIdentity,
    pub source_seq: i64,
    pub event: &'a DomainEvent,
    pub event_digest: &'a str,
    pub observed_utc: &'a str,
    pub effect: Option<LifecycleEffect<'a>>,
    pub budget: &'a ReservationCommand,
    pub slot: &'a SlotCommand,
}

/// A writer-local retained lease, not a grant. No Send or serialization bound.
pub trait AdmissionCommitGuard {}

/// Startup-installed synchronization, additional to both existing authorities.
/// Implementations must be bounded/nonblocking, perform no I/O or Store reentry,
/// and retain their lease until drop. Generic implementations do not establish
/// that arbitrary independently mutable callbacks belong to a common cohort.
pub trait AdmissionCommitFence: Send + Sync {
    fn try_acquire<'a>(
        &'a self,
        binding: &AdmissionFenceBinding<'_>,
        budget_proof: &(dyn Any + Send + Sync),
        slot_proof: &(dyn Any + Send + Sync),
    ) -> Result<Box<dyn AdmissionCommitGuard + 'a>, FenceError>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum FenceError {
    #[error("admission fence binding or catalog is invalid or exceeds bounds")]
    Invalid,
    #[error("admission fence proof or rule is not authorized")]
    Denied,
    #[error("admission fence is busy; no policy change was applied")]
    Busy,
    #[error("admission fence state is poisoned")]
    Poisoned,
    #[error("admission fence epoch changed")]
    Stale,
    #[error("admission fence epoch is exhausted")]
    Exhausted,
}

/// An opaque already-installed bounded rule catalog. Deliberately has no public
/// constructor, Default, Clone, Deserialize, or request/config ingestion path.
/// Authenticated startup issuance is a separate unimplemented trust boundary.
pub struct InstalledAdmissionCatalog {
    rules: BTreeMap<String, InstalledRule>,
    allow_reads: bool,
}

#[derive(Clone, PartialEq, Eq)]
struct InstalledRule {
    fingerprint: String,
    budget_command_digest: String,
    slot_command_digest: String,
}

impl InstalledAdmissionCatalog {
    fn validate(&self) -> Result<(), FenceError> {
        if self.rules.len() > MAX_RULES
            || self.rules.iter().any(|(key, rule)| {
                key != &rule.fingerprint
                    || !digest_valid(key)
                    || !digest_valid(&rule.budget_command_digest)
                    || !digest_valid(&rule.slot_command_digest)
            })
        {
            return Err(FenceError::Invalid);
        }
        Ok(())
    }
}

struct CohortIdentity;
struct CohortState {
    epoch: u64,
    revoked: bool,
    catalog: InstalledAdmissionCatalog,
}
struct Cohort {
    identity: Arc<CohortIdentity>,
    state: RwLock<CohortState>,
}

/// Owns ALL mutable policy-relevant state used by the concrete paired adapters.
/// Rules contain only owned bounded values, never external callbacks or mutable
/// registry references. Mutation occurs only under exclusive nonblocking writes.
pub struct PairedAdmissionCohort {
    cohort: Arc<Cohort>,
}
impl PairedAdmissionCohort {
    pub fn from_installed(catalog: InstalledAdmissionCatalog) -> Result<Self, FenceError> {
        catalog.validate()?;
        Ok(Self {
            cohort: Arc::new(Cohort {
                identity: Arc::new(CohortIdentity),
                state: RwLock::new(CohortState {
                    epoch: 1,
                    revoked: false,
                    catalog,
                }),
            }),
        })
    }

    /// These three adapters share the exact same private Arc and state owner.
    pub fn adapters(&self) -> PairedAdmissionAdapters {
        PairedAdmissionAdapters {
            budget: Arc::new(PairedBudgetAuthority {
                cohort: Arc::clone(&self.cohort),
            }),
            slots: Arc::new(PairedSlotAuthority {
                cohort: Arc::clone(&self.cohort),
            }),
            fence: Arc::new(PairedFence {
                cohort: Arc::clone(&self.cohort),
            }),
        }
    }

    /// Looks up an exact PREINSTALLED rule; this cannot add a requested rule.
    pub fn proofs_for(
        &self,
        binding: &AdmissionFenceBinding<'_>,
    ) -> Result<(ReservationProof, SlotProof), FenceError> {
        let rule = fingerprint(binding)?;
        let state = read_state(&self.cohort)?;
        if state.revoked || state.catalog.rules.get(&rule.fingerprint) != Some(&rule) {
            return Err(FenceError::Denied);
        }
        let make = || WriteProof {
            identity: Arc::clone(&self.cohort.identity),
            epoch: state.epoch,
            rule: rule.clone(),
        };
        Ok((
            Arc::new(BudgetWriteProof(make())),
            Arc::new(SlotWriteProof(make())),
        ))
    }

    pub fn read_proofs(&self) -> Result<(ReservationProof, SlotProof), FenceError> {
        let state = read_state(&self.cohort)?;
        if state.revoked || !state.catalog.allow_reads {
            return Err(FenceError::Denied);
        }
        let make = || ReadProof {
            identity: Arc::clone(&self.cohort.identity),
            epoch: state.epoch,
        };
        Ok((
            Arc::new(BudgetReadProof(make())),
            Arc::new(SlotReadProof(make())),
        ))
    }

    pub fn epoch(&self) -> Result<u64, FenceError> {
        Ok(read_state(&self.cohort)?.epoch)
    }

    /// Busy/Stale/Poisoned/Exhausted changes nothing; callers explicitly retry.
    pub fn try_revoke(&self, expected_epoch: u64) -> Result<u64, FenceError> {
        let mut state = self.cohort.state.try_write().map_err(lock_error)?;
        if state.epoch != expected_epoch {
            return Err(FenceError::Stale);
        }
        let next = state.epoch.checked_add(1).ok_or(FenceError::Exhausted)?;
        state.revoked = true;
        state.epoch = next;
        Ok(next)
    }

    pub fn try_rotate(
        &self,
        expected_epoch: u64,
        catalog: InstalledAdmissionCatalog,
    ) -> Result<u64, FenceError> {
        catalog.validate()?;
        let mut state = self.cohort.state.try_write().map_err(lock_error)?;
        if state.epoch != expected_epoch {
            return Err(FenceError::Stale);
        }
        let next = state.epoch.checked_add(1).ok_or(FenceError::Exhausted)?;
        state.catalog = catalog;
        state.revoked = false;
        state.epoch = next;
        Ok(next)
    }
}

/// Opaque coupled startup installation. Other Store authority fields survive.
pub struct PairedAdmissionAdapters {
    budget: Arc<dyn ReservationAuthority>,
    slots: Arc<dyn InferenceSlotAuthority>,
    fence: Arc<dyn AdmissionCommitFence>,
}
impl PairedAdmissionAdapters {
    pub fn apply_to(self, authorities: &mut StoreAuthorities) {
        authorities.reservations = Some(self.budget);
        authorities.inference_slots = Some(self.slots);
        authorities.admission_fence = Some(self.fence);
    }
}

struct WriteProof {
    identity: Arc<CohortIdentity>,
    epoch: u64,
    rule: InstalledRule,
}
struct ReadProof {
    identity: Arc<CohortIdentity>,
    epoch: u64,
}
// Distinct private Any types prevent using a budget/slot/read role as another.
struct BudgetWriteProof(WriteProof);
struct SlotWriteProof(WriteProof);
struct BudgetReadProof(ReadProof);
struct SlotReadProof(ReadProof);

fn lock_error<T>(error: TryLockError<T>) -> FenceError {
    match error {
        TryLockError::WouldBlock => FenceError::Busy,
        TryLockError::Poisoned(_) => FenceError::Poisoned,
    }
}
fn read_state(cohort: &Cohort) -> Result<RwLockReadGuard<'_, CohortState>, FenceError> {
    cohort.state.try_read().map_err(lock_error)
}
fn write_proof_current(cohort: &Cohort, state: &CohortState, proof: &WriteProof) -> bool {
    !state.revoked
        && Arc::ptr_eq(&cohort.identity, &proof.identity)
        && proof.epoch == state.epoch
        && state.catalog.rules.get(&proof.rule.fingerprint) == Some(&proof.rule)
}
fn read_proof_current(cohort: &Cohort, state: &CohortState, proof: &ReadProof) -> bool {
    !state.revoked
        && state.catalog.allow_reads
        && Arc::ptr_eq(&cohort.identity, &proof.identity)
        && proof.epoch == state.epoch
}

struct PairedFence {
    cohort: Arc<Cohort>,
}
struct CohortCommitGuard<'a> {
    _state: RwLockReadGuard<'a, CohortState>,
}
impl AdmissionCommitGuard for CohortCommitGuard<'_> {}
impl AdmissionCommitFence for PairedFence {
    fn try_acquire<'a>(
        &'a self,
        binding: &AdmissionFenceBinding<'_>,
        budget_proof: &(dyn Any + Send + Sync),
        slot_proof: &(dyn Any + Send + Sync),
    ) -> Result<Box<dyn AdmissionCommitGuard + 'a>, FenceError> {
        let rule = fingerprint(binding)?;
        let budget = budget_proof
            .downcast_ref::<BudgetWriteProof>()
            .ok_or(FenceError::Denied)?;
        let slot = slot_proof
            .downcast_ref::<SlotWriteProof>()
            .ok_or(FenceError::Denied)?;
        let state = read_state(&self.cohort)?;
        if !write_proof_current(&self.cohort, &state, &budget.0)
            || !write_proof_current(&self.cohort, &state, &slot.0)
            || budget.0.rule != rule
            || slot.0.rule != rule
        {
            return Err(FenceError::Denied);
        }
        // The original shared guard survives all Store outcomes. No callback,
        // I/O, expiry extension, or drop-side durable release is performed here.
        Ok(Box::new(CohortCommitGuard { _state: state }))
    }
}

struct PairedBudgetAuthority {
    cohort: Arc<Cohort>,
}
impl ReservationAuthority for PairedBudgetAuthority {
    fn authorize_write(
        &self,
        binding: &ReservationBinding<'_>,
        proof: &(dyn Any + Send + Sync),
    ) -> bool {
        let Some(proof) = proof.downcast_ref::<BudgetWriteProof>() else {
            return false;
        };
        let Ok(state) = read_state(&self.cohort) else {
            return false;
        };
        let ReservationEvent::Reserve { scope, .. } = &binding.command.event else {
            return false;
        };
        write_proof_current(&self.cohort, &state, &proof.0)
            && admission_reservations::validate_command(binding.command).is_ok_and(|digest| {
                digest == binding.command_digest && digest == proof.0.rule.budget_command_digest
            })
            && task_matches(binding.task, scope, binding.historical_attempt)
    }
    fn authorize_read(&self, _ledger: &ReservationLedger, proof: &(dyn Any + Send + Sync)) -> bool {
        let Some(proof) = proof.downcast_ref::<BudgetReadProof>() else {
            return false;
        };
        read_state(&self.cohort)
            .is_ok_and(|state| read_proof_current(&self.cohort, &state, &proof.0))
    }
}

struct PairedSlotAuthority {
    cohort: Arc<Cohort>,
}
impl InferenceSlotAuthority for PairedSlotAuthority {
    fn authorize_write(&self, binding: &SlotBinding<'_>, proof: &(dyn Any + Send + Sync)) -> bool {
        let Some(proof) = proof.downcast_ref::<SlotWriteProof>() else {
            return false;
        };
        let Ok(state) = read_state(&self.cohort) else {
            return false;
        };
        let SlotEvent::Hold { claim, .. } = &binding.command.event else {
            return false;
        };
        write_proof_current(&self.cohort, &state, &proof.0)
            && inference_slots::validate_command(binding.command).is_ok_and(|digest| {
                digest == binding.command_digest && digest == proof.0.rule.slot_command_digest
            })
            && task_matches(binding.task, &claim.scope, binding.historical_attempt)
    }
    fn authorize_read(
        &self,
        _ledger: &inference_slots::SlotLedger,
        proof: &(dyn Any + Send + Sync),
    ) -> bool {
        let Some(proof) = proof.downcast_ref::<SlotReadProof>() else {
            return false;
        };
        read_state(&self.cohort)
            .is_ok_and(|state| read_proof_current(&self.cohort, &state, &proof.0))
    }
}

fn task_matches(task: Option<&TaskView>, scope: &ReservationScope, historical: bool) -> bool {
    let Some(task) = task else {
        return false;
    };
    let contract = &task.contract;
    if bounded_json_size(contract, MAX_EVENT_BYTES).is_err()
        || contract.project_id != scope.project_id
        || contract.root_id != scope.root_id
        || contract.task_id != scope.task_id
        || contract.revision != scope.contract_revision
        || contract.authority_ref != scope.authority_ref
        || !content_digest(contract).is_ok_and(|digest| digest == scope.contract_digest)
    {
        return false;
    }
    if historical {
        return task.lifecycle.as_ref().is_some_and(|lifecycle| {
            lifecycle.attempts.get(&scope.attempt_id) == Some(&scope.owner)
        });
    }
    (task.attempt_id.as_ref() == Some(&scope.attempt_id)
        && task.owner.as_ref() == Some(&scope.owner))
        || (task.state == SchedulingState::Ready
            && task.attempt_id.is_none()
            && task.owner.is_none())
}

fn digest_valid(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}
fn bounded_text(value: &str, limit: usize) -> bool {
    !value.is_empty() && value.len() <= limit && !value.chars().any(char::is_control)
}

fn fingerprint(binding: &AdmissionFenceBinding<'_>) -> Result<InstalledRule, FenceError> {
    // Bound all borrowed data BEFORE allocating command/event/effect identities.
    if !bounded_text(&binding.source.producer_id, 128)
        || !bounded_text(&binding.source.incarnation, 128)
        || binding.source.sequence.len() > 20
        || !bounded_text(binding.observed_utc, 128)
        || !digest_valid(binding.event_digest)
    {
        return Err(FenceError::Invalid);
    }
    bounded_json_size(binding.event, MAX_EVENT_BYTES).map_err(|_| FenceError::Invalid)?;
    bounded_json_size(binding.budget, admission_reservations::MAX_COMMAND_BYTES)
        .map_err(|_| FenceError::Invalid)?;
    bounded_json_size(binding.slot, inference_slots::MAX_COMMAND_BYTES)
        .map_err(|_| FenceError::Invalid)?;
    let effect = binding.effect.as_ref().ok_or(FenceError::Invalid)?;
    if !bounded_text(effect.idempotency_key, 256)
        || effect.payload.len() > crate::outbox::MAX_PAYLOAD_BYTES
        || !digest_valid(effect.payload_digest)
        || !digest_valid(effect.intent_digest)
        || effect.not_before < 0
    {
        return Err(FenceError::Invalid);
    }
    let effect_fields = (
        effect.idempotency_key,
        effect.payload,
        effect.payload_digest,
        effect.intent_digest,
        effect.not_before,
    );
    bounded_json_size(&effect_fields, MAX_EFFECT_BYTES).map_err(|_| FenceError::Invalid)?;
    let sequence = binding.source.validate().map_err(|_| FenceError::Invalid)?;
    if i64::try_from(sequence).ok() != Some(binding.source_seq) || binding.source_seq <= 0 {
        return Err(FenceError::Invalid);
    }
    let event_json = serde_json::to_string(binding.event).map_err(|_| FenceError::Invalid)?;
    // Exactly Store::prepare_append's raw bounded JSON-byte digest, with no
    // invented event-domain prefix or reconstructed outbox payload.
    if crate::digest(&event_json) != binding.event_digest
        || crate::digest(effect.payload) != effect.payload_digest
        || crate::outbox::prepared_intent_digest(
            effect.idempotency_key,
            effect.payload,
            effect.not_before,
        ) != effect.intent_digest
    {
        return Err(FenceError::Invalid);
    }
    let budget_command_digest = admission_reservations::validate_command(binding.budget)
        .map_err(|_| FenceError::Invalid)?;
    let slot_command_digest =
        inference_slots::validate_command(binding.slot).map_err(|_| FenceError::Invalid)?;
    let ReservationEvent::Reserve {
        reservation_id,
        scope,
        amount,
        ..
    } = &binding.budget.event
    else {
        return Err(FenceError::Invalid);
    };
    let SlotEvent::Hold { claim, .. } = &binding.slot.event else {
        return Err(FenceError::Invalid);
    };
    let DomainEvent::TaskClaimed {
        task_id,
        revision,
        attempt_id,
        owner,
    } = binding.event
    else {
        return Err(FenceError::Invalid);
    };
    if binding.source != &binding.budget.source
        || binding.source != &binding.slot.source
        || task_id != &scope.task_id
        || revision != &scope.contract_revision
        || attempt_id != &scope.attempt_id
        || owner != &scope.owner
        || scope != &claim.scope
        || amount.get() == 0
        || claim.budget_reservation_id != *reservation_id
        || claim.budget_command_digest != budget_command_digest
        || claim.kernel_claims_digest != scope.resource_claims_digest
        || binding.budget.launch_intent_digest.as_deref() != Some(effect.intent_digest)
        || claim.launch_intent_digest != effect.intent_digest
        || binding
            .budget
            .expected_ledger_revision
            .checked_add(1)
            .is_none()
        || binding
            .slot
            .expected_ledger_revision
            .checked_add(1)
            .is_none()
    {
        return Err(FenceError::Invalid);
    }
    let fingerprint = content_digest(&(
        "admiralty:paired-admission-binding:v1",
        binding.source,
        binding.source_seq,
        binding.event_digest,
        binding.observed_utc,
        effect_fields,
        &budget_command_digest,
        &slot_command_digest,
    ))
    .map_err(|_| FenceError::Invalid)?;
    Ok(InstalledRule {
        fingerprint,
        budget_command_digest,
        slot_command_digest,
    })
}

/// Test-only installation, deliberately absent from non-test production builds.
#[cfg(test)]
fn catalog_for_tests(
    bindings: &[AdmissionFenceBinding<'_>],
    allow_reads: bool,
) -> Result<InstalledAdmissionCatalog, FenceError> {
    if bindings.len() > MAX_RULES {
        return Err(FenceError::Invalid);
    }
    let mut rules = BTreeMap::new();
    for binding in bindings {
        let rule = fingerprint(binding)?;
        rules.insert(rule.fingerprint.clone(), rule);
    }
    let catalog = InstalledAdmissionCatalog { rules, allow_reads };
    catalog.validate()?;
    Ok(catalog)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn empty(allow_reads: bool) -> InstalledAdmissionCatalog {
        InstalledAdmissionCatalog {
            rules: BTreeMap::new(),
            allow_reads,
        }
    }

    #[test]
    fn read_proofs_require_own_cohort_role_and_current_epoch() {
        let cohort = PairedAdmissionCohort::from_installed(empty(true)).unwrap();
        let other = PairedAdmissionCohort::from_installed(empty(true)).unwrap();
        let adapters = cohort.adapters();
        let foreign = other.adapters();
        let (budget, slots) = cohort.read_proofs().unwrap();
        let budget_ledger = ReservationLedger::default();
        let slot_ledger = inference_slots::SlotLedger::default();
        assert!(
            adapters
                .budget
                .authorize_read(&budget_ledger, budget.as_ref())
        );
        assert!(adapters.slots.authorize_read(&slot_ledger, slots.as_ref()));
        assert!(
            !adapters
                .budget
                .authorize_read(&budget_ledger, slots.as_ref())
        );
        assert!(!adapters.slots.authorize_read(&slot_ledger, budget.as_ref()));
        assert!(
            !foreign
                .budget
                .authorize_read(&budget_ledger, budget.as_ref())
        );
        assert_eq!(cohort.try_revoke(1), Ok(2));
        assert!(
            !adapters
                .budget
                .authorize_read(&budget_ledger, budget.as_ref())
        );
        assert!(!adapters.slots.authorize_read(&slot_ledger, slots.as_ref()));
        assert!(matches!(cohort.read_proofs(), Err(FenceError::Denied)));
        assert_eq!(cohort.try_revoke(1), Err(FenceError::Stale));
        assert_eq!(cohort.epoch(), Ok(2));
    }

    #[test]
    fn retained_shared_guard_blocks_rotation_and_revocation() {
        let cohort = PairedAdmissionCohort::from_installed(empty(true)).unwrap();
        let guard = CohortCommitGuard {
            _state: read_state(&cohort.cohort).unwrap(),
        };
        assert_eq!(cohort.try_revoke(1), Err(FenceError::Busy));
        assert_eq!(cohort.try_rotate(1, empty(false)), Err(FenceError::Busy));
        assert_eq!(cohort.epoch(), Ok(1));
        assert!(cohort.read_proofs().is_ok());
        drop(guard);
        assert_eq!(cohort.try_rotate(1, empty(false)), Ok(2));
        assert!(matches!(cohort.read_proofs(), Err(FenceError::Denied)));
    }

    #[test]
    fn epoch_overflow_preserves_current_rules_and_revocation_state() {
        let cohort = PairedAdmissionCohort::from_installed(empty(true)).unwrap();
        cohort.cohort.state.try_write().unwrap().epoch = u64::MAX;
        assert_eq!(cohort.try_revoke(u64::MAX), Err(FenceError::Exhausted));
        assert_eq!(
            cohort.try_rotate(u64::MAX, empty(false)),
            Err(FenceError::Exhausted)
        );
        assert_eq!(cohort.epoch(), Ok(u64::MAX));
        assert!(cohort.read_proofs().is_ok());
    }

    #[test]
    fn poison_and_catalog_bounds_fail_closed() {
        let cohort = PairedAdmissionCohort::from_installed(empty(true)).unwrap();
        let failed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = cohort.cohort.state.try_write().unwrap();
            panic!("owned test poisons only its private cohort");
        }));
        assert!(failed.is_err());
        assert_eq!(cohort.epoch(), Err(FenceError::Poisoned));
        assert_eq!(cohort.try_revoke(1), Err(FenceError::Poisoned));
        assert!(matches!(cohort.read_proofs(), Err(FenceError::Poisoned)));
        let mut catalog = empty(false);
        for index in 0..=MAX_RULES {
            let fingerprint = format!("sha256:{index:064x}");
            catalog.rules.insert(
                fingerprint.clone(),
                InstalledRule {
                    fingerprint,
                    budget_command_digest: format!("sha256:{}", "a".repeat(64)),
                    slot_command_digest: format!("sha256:{}", "b".repeat(64)),
                },
            );
        }
        assert!(matches!(
            PairedAdmissionCohort::from_installed(catalog),
            Err(FenceError::Invalid)
        ));
        let mut malformed = empty(false);
        malformed.rules.insert(
            "foreign".into(),
            InstalledRule {
                fingerprint: "foreign".into(),
                budget_command_digest: "not-a-digest".into(),
                slot_command_digest: "not-a-digest".into(),
            },
        );
        assert!(matches!(
            PairedAdmissionCohort::from_installed(malformed),
            Err(FenceError::Invalid)
        ));
    }
}

#[cfg(test)]
#[path = "admission_fence_store_tests.rs"]
mod store_tests;
