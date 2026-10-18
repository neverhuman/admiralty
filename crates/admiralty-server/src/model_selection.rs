//! Own-model selection, bounded attempt escalation, strategy candidate comparison,
//! and inference slot + budget reservation proposal wiring.
//!
//! This module connects `admiralty_runtime::models` (own-model configuration and
//! Ornith -> Luna fallback policy), `admiralty_runtime::strategies` (parallel
//! variants and synthesis fallback), and `admiralty_runtime::reservations`
//! (conservative budget reservation and settlement) to read-only `admiralty_core`
//! task/lifecycle contracts. It performs no provider launches, network calls, or
//! mutations to store/reducer state.

use crate::models::ModelsApiError;
use admiralty_core::{
    AttemptId, CandidateDecisionEvidence, PrincipalId, ProjectId, RootId, SchedulingState,
    TaskContract, TaskId, TaskView, content_digest,
};
use admiralty_runtime::{
    models::{
        CapacitySnapshot, FailureReason, ModelRegistry, ModelSpec, Provider, RegistryError,
        RegistryFile, SelectionOutcome, SelectionRequest, UsageCost, WaitReason,
    },
    reservations::{
        AccountId, CurrencyUnit, FinalEvidence, MinorUnits, ReconciliationEvidence, Reservation,
        ReservationBinding, ReservationError, ReservationEvent, ReservationLedger,
        ReservationScope, reduce_reservation,
    },
    strategies::{
        ArtifactRef, BestOfConfig, BestOfPlan, CandidateRef, PlanDecision,
        QualifiedSynthesisFailure, StrategyCapacity, StrategyError, StrategyWait, StrategyWorkUnit,
        SynthesisDecision, VariantOutcome, VariantReport, plan_synthesis, plan_synthesis_fallback,
        plan_variants,
    },
};
use axum::{Json, http::StatusCode};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

pub const MAX_ATTEMPT_HISTORY: usize = 64;

/// Verified digest of a `ModelSpec`'s routing and concurrency configuration.
/// Ensures slot reservations bind the exact observed model/provider/node tuple.
pub fn model_config_digest(spec: &ModelSpec) -> Result<String, ModelSelectionError> {
    content_digest(&(
        &spec.id,
        &spec.provider,
        &spec.provider_model,
        spec.tier,
        spec.enabled,
        &spec.node_id,
        &spec.endpoint,
        &spec.credential_file_ref,
        spec.max_concurrency,
        &spec.capabilities,
        spec.task_max_attempts,
        &spec.escalate_to,
    ))
    .map_err(|e| ModelSelectionError::Invalid(format!("model digest failed: {e}")))
}

/// Observed model slot utilization and configuration attestation at a bounded tick.
/// Missing or `None` slot counts remain explicitly unknown and never default to zero.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ObservedModelSlotState {
    pub registry_revision: String,
    pub snapshot_revision: String,
    pub observed_tick: u64,
    pub valid_through_tick: u64,
    #[serde(default)]
    pub in_use: BTreeMap<String, Option<u16>>,
    #[serde(default)]
    pub unavailable: BTreeSet<String>,
    /// Maps `model_id` to `model_config_digest(spec)` observed on the target host/runner.
    #[serde(default)]
    pub observed_model_digests: BTreeMap<String, String>,
}

impl ObservedModelSlotState {
    fn validate_structure(&self, registry: &ModelRegistry) -> Result<(), ModelSelectionError> {
        if self.snapshot_revision.trim().is_empty()
            || self.snapshot_revision.len() > 128
            || self.snapshot_revision.chars().any(char::is_control)
        {
            return Err(ModelSelectionError::Invalid(
                "snapshot_revision must be a non-empty bounded identifier".into(),
            ));
        }
        if self.valid_through_tick < self.observed_tick {
            return Err(ModelSelectionError::Invalid(
                "valid_through_tick cannot precede observed_tick".into(),
            ));
        }
        let known_ids: BTreeSet<&str> = registry.models.iter().map(|m| m.id.as_str()).collect();
        for id in self
            .in_use
            .keys()
            .chain(self.unavailable.iter())
            .chain(self.observed_model_digests.keys())
        {
            if !known_ids.contains(id.as_str()) {
                return Err(ModelSelectionError::Invalid(format!(
                    "observation references unknown model {id}"
                )));
            }
        }
        Ok(())
    }

    fn validate_revision_and_freshness(
        &self,
        registry: &ModelRegistry,
        now_tick: u64,
    ) -> Result<(), ModelSelectionError> {
        if self.registry_revision != registry.revision {
            return Err(ModelSelectionError::RegistryRevisionMismatch {
                expected: registry.revision.clone(),
                observed: self.registry_revision.clone(),
            });
        }
        if self.observed_tick > now_tick
            || self.valid_through_tick < now_tick
            || self.valid_through_tick < self.observed_tick
        {
            return Err(ModelSelectionError::StaleObservation {
                observed_tick: self.observed_tick,
                valid_through_tick: self.valid_through_tick,
                now_tick,
            });
        }
        Ok(())
    }

    /// Validate observation freshness and convert to a conservative `CapacitySnapshot`
    /// for single-model selection. Any model lacking a known `in_use` count or a matching
    /// `observed_model_digests` entry is marked unavailable so unverified hosts/models
    /// cannot be silently selected.
    pub fn to_verified_capacity(
        &self,
        registry: &ModelRegistry,
        now_tick: u64,
    ) -> Result<CapacitySnapshot, ModelSelectionError> {
        registry.validate()?;
        self.validate_revision_and_freshness(registry, now_tick)?;
        self.validate_structure(registry)?;

        let mut snapshot = CapacitySnapshot {
            in_use: BTreeMap::new(),
            unavailable: self.unavailable.clone(),
        };

        for spec in &registry.models {
            let expected_digest = model_config_digest(spec)?;
            let digest_matches = self
                .observed_model_digests
                .get(&spec.id)
                .is_some_and(|observed| observed == &expected_digest);
            if !digest_matches {
                snapshot.unavailable.insert(spec.id.clone());
                continue;
            }
            match self.in_use.get(&spec.id) {
                Some(Some(used)) => {
                    if *used > spec.max_concurrency {
                        return Err(ModelSelectionError::Invalid(format!(
                            "observed in_use ({used}) exceeds max_concurrency ({}) for model {}",
                            spec.max_concurrency, spec.id
                        )));
                    }
                    snapshot.in_use.insert(spec.id.clone(), *used);
                }
                Some(None) | None => {
                    snapshot.unavailable.insert(spec.id.clone());
                }
            }
        }
        Ok(snapshot)
    }

    /// Convert to `StrategyCapacity` for parallel variant + synthesis planning.
    /// Validates `snapshot_revision`, tick ordering, and known model IDs.
    /// Models that are unavailable, have unknown `in_use`, or lack a verified config
    /// digest receive `None` (unknown free slots) rather than `Some(0)`.
    pub fn to_strategy_capacity(
        &self,
        registry: &ModelRegistry,
    ) -> Result<StrategyCapacity, ModelSelectionError> {
        registry.validate()?;
        self.validate_structure(registry)?;
        let mut free_slots = BTreeMap::new();
        for spec in &registry.models {
            let expected_digest = model_config_digest(spec)?;
            let digest_matches = self
                .observed_model_digests
                .get(&spec.id)
                .is_some_and(|observed| observed == &expected_digest);
            if !digest_matches || self.unavailable.contains(&spec.id) {
                free_slots.insert(spec.id.clone(), None);
                continue;
            }
            match self.in_use.get(&spec.id) {
                Some(Some(used)) => {
                    if *used > spec.max_concurrency {
                        return Err(ModelSelectionError::Invalid(format!(
                            "observed in_use ({used}) exceeds max_concurrency ({}) for model {}",
                            spec.max_concurrency, spec.id
                        )));
                    }
                    free_slots.insert(spec.id.clone(), Some(spec.max_concurrency - *used));
                }
                Some(None) | None => {
                    free_slots.insert(spec.id.clone(), None);
                }
            }
        }
        Ok(StrategyCapacity {
            registry_revision: self.registry_revision.clone(),
            snapshot_revision: self.snapshot_revision.clone(),
            observed_tick: self.observed_tick,
            valid_through_tick: self.valid_through_tick,
            free_slots,
        })
    }

    /// Convert to `StrategyCapacity` after strictly enforcing both `registry_revision`
    /// match and `now_tick` freshness window for direct callers outside `admiralty_runtime::strategies`.
    pub fn to_verified_strategy_capacity(
        &self,
        registry: &ModelRegistry,
        now_tick: u64,
    ) -> Result<StrategyCapacity, ModelSelectionError> {
        registry.validate()?;
        self.validate_revision_and_freshness(registry, now_tick)?;
        self.to_strategy_capacity(registry)
    }
}

/// Classified execution/provider failure categories.
/// Setup, authentication, unknown-model, and transport failures require operator/environment
/// repair (`FailureReason::SetupError`) and never authorize stronger-spend model escalation.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskExecutionFailureKind {
    TaskExecutionFailed,
    TaskCheckFailed,
    ContextWindowExhausted,
    ProviderUnavailable,
    SetupError,
    AuthenticationError,
    UnknownModelError,
    TransportError,
    BlockedOnOperatorInput,
}

impl TaskExecutionFailureKind {
    pub const fn to_registry_failure_reason(self) -> FailureReason {
        match self {
            Self::TaskExecutionFailed => FailureReason::Failure,
            Self::TaskCheckFailed => FailureReason::TestFail,
            Self::ContextWindowExhausted => FailureReason::ContextExhausted,
            Self::ProviderUnavailable => FailureReason::ProviderUnavailable,
            Self::SetupError
            | Self::AuthenticationError
            | Self::UnknownModelError
            | Self::TransportError => FailureReason::SetupError,
            Self::BlockedOnOperatorInput => FailureReason::BlockedInput,
        }
    }
}

/// Qualified failure evidence from a prior attempt required before retrying or escalating.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AttemptFailureEvidence {
    pub reason: FailureReason,
    pub qualified: bool,
    pub diagnostic_artifact: ArtifactRef,
    pub closure_digest: String,
}

impl AttemptFailureEvidence {
    pub fn from_classified(
        kind: TaskExecutionFailureKind,
        qualified: bool,
        diagnostic_artifact: ArtifactRef,
        closure_digest: String,
    ) -> Self {
        Self {
            reason: kind.to_registry_failure_reason(),
            qualified,
            diagnostic_artifact,
            closure_digest,
        }
    }
}

/// Immutable record of a prior model attempt on a task.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ModelAttemptRecord {
    pub attempt_id: AttemptId,
    pub model_id: String,
    pub tier: i32,
    pub reservation_id: String,
    pub contract_revision: u32,
    pub contract_digest: String,
    pub failure: Option<AttemptFailureEvidence>,
}

/// Complete per-task model attempt lineage for a specific contract revision.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TaskModelAttemptHistory {
    pub attempts: Vec<ModelAttemptRecord>,
}

/// Policy and budget parameters for proposing a model slot + budget reservation.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ModelEscalationPolicy {
    pub required_capabilities: BTreeSet<String>,
    pub pinned_model: Option<String>,
    pub allow_escalation: bool,
    pub allow_capacity_fallback: bool,
    pub account_id: AccountId,
    pub unit: CurrencyUnit,
    /// Per-attempt budget reservation required for each model ID.
    pub model_attempt_budgets: BTreeMap<String, MinorUnits>,
    pub owner: PrincipalId,
    pub incarnation: String,
    pub next_attempt_id: AttemptId,
    pub reservation_id: String,
    pub reservation_event_id: String,
    pub correlation_id: String,
}

/// Bound inference slot claim whose digest becomes `ReservationScope::resource_claims_digest`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InferenceSlotClaim {
    pub project_id: ProjectId,
    pub root_id: RootId,
    pub task_id: TaskId,
    pub attempt_id: AttemptId,
    pub contract_revision: u32,
    pub contract_digest: String,
    pub model_id: String,
    pub provider: Provider,
    pub provider_model: String,
    pub tier: i32,
    pub node_id: Option<String>,
    pub slots_reserved: u16,
    pub registry_revision: String,
    pub snapshot_revision: String,
    pub model_config_digest: String,
}

/// Complete proposal binding a selected model, its inference slot claim, and an atomic
/// budget reservation on the `ReservationLedger`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelSlotReservationProposal {
    pub selected_model_id: String,
    pub selected_model_spec: ModelSpec,
    pub provider: Provider,
    pub provider_model: String,
    pub tier: i32,
    pub node_id: Option<String>,
    pub attempt_number: u16,
    pub tier_attempt_number: u16,
    pub escalated_from: Option<String>,
    pub usage_cost: UsageCost,
    pub selection_request: SelectionRequest,
    pub slot_claim: InferenceSlotClaim,
    pub resource_claims_digest: String,
    pub reservation_scope: ReservationScope,
    pub reservation_event: ReservationEvent,
    pub next_ledger: ReservationLedger,
    pub pre_reservation_capacity: CapacitySnapshot,
    pub updated_capacity: CapacitySnapshot,
    /// Pure proposal state; single-writer serialization and atomic commit are required
    /// for real admission.
    pub proposal_only: bool,
    pub writer_atomicity_required: bool,
    /// Proposals never self-authorize execution; callers must commit the reservation
    /// and pass `ExecutionAdmissionValidator` before launching a session.
    pub execution_granted: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModelSelectionWait {
    RegistryWait(WaitReason),
    TaskAttemptLimitReached {
        used: u16,
        max_attempts: u16,
    },
    UnresolvedPriorAttempt {
        attempt_id: AttemptId,
        reservation_id: String,
    },
    UnqualifiedFailureEvidence {
        attempt_id: AttemptId,
    },
    TaskNotSchedulable {
        state: SchedulingState,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ModelSlotDecision {
    Ready {
        proposal: Box<ModelSlotReservationProposal>,
    },
    Waiting {
        model_id: Option<String>,
        reason: ModelSelectionWait,
    },
}

#[derive(Debug)]
pub enum ModelSelectionError {
    Invalid(String),
    Registry(RegistryError),
    Reservation(ReservationError),
    Strategy(StrategyError),
    RegistryRevisionMismatch {
        expected: String,
        observed: String,
    },
    StaleObservation {
        observed_tick: u64,
        valid_through_tick: u64,
        now_tick: u64,
    },
}

impl fmt::Display for ModelSelectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(msg) => write!(f, "invalid model selection input: {msg}"),
            Self::Registry(err) => write!(f, "model registry error: {err}"),
            Self::Reservation(err) => write!(f, "budget reservation error: {err}"),
            Self::Strategy(err) => write!(f, "strategy error: {err}"),
            Self::RegistryRevisionMismatch { expected, observed } => write!(
                f,
                "registry revision mismatch: expected {expected}, observed {observed}"
            ),
            Self::StaleObservation {
                observed_tick,
                valid_through_tick,
                now_tick,
            } => write!(
                f,
                "stale model slot observation: observed_tick={observed_tick}, valid_through_tick={valid_through_tick}, now_tick={now_tick}"
            ),
        }
    }
}

impl std::error::Error for ModelSelectionError {}

impl From<RegistryError> for ModelSelectionError {
    fn from(value: RegistryError) -> Self {
        Self::Registry(value)
    }
}

impl From<ReservationError> for ModelSelectionError {
    fn from(value: ReservationError) -> Self {
        Self::Reservation(value)
    }
}

impl From<StrategyError> for ModelSelectionError {
    fn from(value: StrategyError) -> Self {
        Self::Strategy(value)
    }
}

fn valid_sha256_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    })
}

/// Validate task state, derive verified attempt counts per tier, and propose an
/// own-model or escalated model selection wired to an inference slot claim and an
/// atomic budget reservation on `ReservationLedger`.
pub fn propose_model_slot_reservation(
    registry: &ModelRegistry,
    task: &TaskView,
    history: &TaskModelAttemptHistory,
    policy: &ModelEscalationPolicy,
    observed: &ObservedModelSlotState,
    ledger: &ReservationLedger,
    now_tick: u64,
) -> Result<ModelSlotDecision, ModelSelectionError> {
    registry.validate()?;
    task.contract
        .validate()
        .map_err(|e| ModelSelectionError::Invalid(format!("invalid task contract: {e}")))?;

    match &task.state {
        SchedulingState::Ready => {
            if task.attempt_id.is_some() || task.owner.is_some() {
                return Err(ModelSelectionError::Invalid(
                    "ready task must not carry an active attempt_id or owner claim".into(),
                ));
            }
        }
        SchedulingState::Claimed => {
            if task.attempt_id.as_ref() != Some(&policy.next_attempt_id) {
                return Err(ModelSelectionError::Invalid(
                    "claimed task attempt_id does not match policy.next_attempt_id".into(),
                ));
            }
            if task.owner.as_ref() != Some(&policy.owner) {
                return Err(ModelSelectionError::Invalid(
                    "claimed task owner does not match policy.owner".into(),
                ));
            }
        }
        other => {
            return Ok(ModelSlotDecision::Waiting {
                model_id: None,
                reason: ModelSelectionWait::TaskNotSchedulable {
                    state: other.clone(),
                },
            });
        }
    }

    if history.attempts.len() > MAX_ATTEMPT_HISTORY {
        return Err(ModelSelectionError::Invalid(
            "attempt history exceeds MAX_ATTEMPT_HISTORY".into(),
        ));
    }

    let contract_digest = content_digest(&task.contract)
        .map_err(|e| ModelSelectionError::Invalid(format!("contract digest failed: {e}")))?;

    let by_id: BTreeMap<&str, &ModelSpec> =
        registry.models.iter().map(|m| (m.id.as_str(), m)).collect();

    let mut seen_attempts = BTreeSet::new();
    let mut seen_reservations = BTreeSet::new();
    let mut attempts_by_tier: BTreeMap<i32, u16> = BTreeMap::new();

    for record in &history.attempts {
        if !seen_attempts.insert(record.attempt_id.clone()) {
            return Err(ModelSelectionError::Invalid(format!(
                "duplicate attempt_id {} in history",
                record.attempt_id
            )));
        }
        if !seen_reservations.insert(record.reservation_id.clone()) {
            return Err(ModelSelectionError::Invalid(format!(
                "duplicate reservation_id {} in history",
                record.reservation_id
            )));
        }
        if record.contract_revision != task.contract.revision
            || record.contract_digest != contract_digest
        {
            return Err(ModelSelectionError::Invalid(
                "attempt history record does not bind current task contract revision and digest"
                    .into(),
            ));
        }
        let spec = by_id.get(record.model_id.as_str()).ok_or_else(|| {
            ModelSelectionError::Invalid(format!(
                "attempt history references unknown model {}",
                record.model_id
            ))
        })?;
        if spec.tier != record.tier {
            return Err(ModelSelectionError::Invalid(format!(
                "attempt history tier {} does not match registry tier {} for model {}",
                record.tier, spec.tier, record.model_id
            )));
        }

        // Prior reservation must exist in ledger and be settled (not outstanding).
        let prior_res = ledger
            .reservations
            .get(&record.reservation_id)
            .ok_or_else(|| {
                ModelSelectionError::Invalid(format!(
                    "prior attempt reservation {} missing from ledger",
                    record.reservation_id
                ))
            })?;
        if prior_res.scope.project_id != task.contract.project_id
            || prior_res.scope.root_id != task.contract.root_id
            || prior_res.scope.task_id != task.contract.task_id
            || prior_res.scope.attempt_id != record.attempt_id
            || prior_res.scope.contract_revision != task.contract.revision
            || prior_res.scope.contract_digest != contract_digest
        {
            return Err(ModelSelectionError::Invalid(
                "prior reservation scope does not match task attempt record".into(),
            ));
        }
        if let Some(lifecycle) = &task.lifecycle {
            let Some(lifecycle_owner) = lifecycle.attempts.get(&record.attempt_id) else {
                return Err(ModelSelectionError::Invalid(format!(
                    "task lifecycle missing historical attempt_id {}",
                    record.attempt_id
                )));
            };
            if lifecycle_owner != &prior_res.scope.owner {
                return Err(ModelSelectionError::Invalid(format!(
                    "task lifecycle owner for attempt {} does not match reservation owner",
                    record.attempt_id
                )));
            }
        }
        if prior_res.outstanding {
            return Ok(ModelSlotDecision::Waiting {
                model_id: Some(record.model_id.clone()),
                reason: ModelSelectionWait::UnresolvedPriorAttempt {
                    attempt_id: record.attempt_id.clone(),
                    reservation_id: record.reservation_id.clone(),
                },
            });
        }

        let Some(failure) = &record.failure else {
            return Ok(ModelSlotDecision::Waiting {
                model_id: Some(record.model_id.clone()),
                reason: ModelSelectionWait::UnresolvedPriorAttempt {
                    attempt_id: record.attempt_id.clone(),
                    reservation_id: record.reservation_id.clone(),
                },
            });
        };
        if !valid_sha256_digest(&failure.diagnostic_artifact.sha256)
            || !valid_sha256_digest(&failure.closure_digest)
        {
            return Err(ModelSelectionError::Invalid(
                "prior attempt failure requires valid sha256 diagnostic and closure digests".into(),
            ));
        }
        if !failure.qualified {
            return Ok(ModelSlotDecision::Waiting {
                model_id: Some(record.model_id.clone()),
                reason: ModelSelectionWait::UnqualifiedFailureEvidence {
                    attempt_id: record.attempt_id.clone(),
                },
            });
        }

        let count = attempts_by_tier.entry(record.tier).or_insert(0);
        *count = count
            .checked_add(1)
            .ok_or_else(|| ModelSelectionError::Invalid("tier attempt counter overflow".into()))?;
    }

    if seen_attempts.contains(&policy.next_attempt_id) {
        return Err(ModelSelectionError::Invalid(
            "next_attempt_id collides with a prior attempt in history".into(),
        ));
    }
    if let Some(lifecycle) = &task.lifecycle {
        match task.state {
            SchedulingState::Ready if lifecycle.attempts.len() != history.attempts.len() => {
                return Err(ModelSelectionError::Invalid(
                    "ready task lifecycle attempts do not match supplied model attempt history"
                        .into(),
                ));
            }
            SchedulingState::Claimed
                if lifecycle.attempts.get(&policy.next_attempt_id) != Some(&policy.owner)
                    || lifecycle.attempts.len() != history.attempts.len() + 1 =>
            {
                return Err(ModelSelectionError::Invalid(
                    "claimed task lifecycle attempts must bind next_attempt_id to policy.owner and match history".into(),
                ));
            }
            _ => {}
        }
    }

    let used_attempts = u16::try_from(history.attempts.len())
        .map_err(|_| ModelSelectionError::Invalid("attempt count overflow".into()))?;
    if used_attempts >= task.contract.max_attempts {
        return Ok(ModelSlotDecision::Waiting {
            model_id: history.attempts.last().map(|a| a.model_id.clone()),
            reason: ModelSelectionWait::TaskAttemptLimitReached {
                used: used_attempts,
                max_attempts: task.contract.max_attempts,
            },
        });
    }

    let capacity = observed.to_verified_capacity(registry, now_tick)?;
    let last_attempt = history.attempts.last();
    let current_model = last_attempt.map(|a| a.model_id.clone());
    let last_failure = last_attempt.and_then(|a| a.failure.as_ref().map(|f| f.reason));

    let selection_request = SelectionRequest {
        required_capabilities: policy.required_capabilities.clone(),
        pinned_model: policy.pinned_model.clone(),
        current_model: current_model.clone(),
        attempts_by_tier: attempts_by_tier.clone(),
        last_failure,
        allow_escalation: policy.allow_escalation,
        allow_capacity_fallback: policy.allow_capacity_fallback,
    };

    match registry.select(&selection_request, &capacity)? {
        SelectionOutcome::Waiting { model_id, reason } => Ok(ModelSlotDecision::Waiting {
            model_id,
            reason: ModelSelectionWait::RegistryWait(reason),
        }),
        SelectionOutcome::Selected {
            model_id,
            usage_cost,
        } => {
            let spec = by_id.get(model_id.as_str()).ok_or_else(|| {
                ModelSelectionError::Invalid(format!("selected unknown model {model_id}"))
            })?;
            let amount = policy
                .model_attempt_budgets
                .get(&model_id)
                .cloned()
                .ok_or_else(|| {
                    ModelSelectionError::Invalid(format!(
                        "missing per-attempt budget configuration for selected model {model_id}"
                    ))
                })?;

            let spec_digest = model_config_digest(spec)?;
            let slot_claim = InferenceSlotClaim {
                project_id: task.contract.project_id.clone(),
                root_id: task.contract.root_id.clone(),
                task_id: task.contract.task_id.clone(),
                attempt_id: policy.next_attempt_id.clone(),
                contract_revision: task.contract.revision,
                contract_digest: contract_digest.clone(),
                model_id: spec.id.clone(),
                provider: spec.provider.clone(),
                provider_model: spec.provider_model.clone(),
                tier: spec.tier,
                node_id: spec.node_id.clone(),
                slots_reserved: 1,
                registry_revision: registry.revision.clone(),
                snapshot_revision: observed.snapshot_revision.clone(),
                model_config_digest: spec_digest,
            };
            let resource_claims_digest = content_digest(&slot_claim).map_err(|e| {
                ModelSelectionError::Invalid(format!("slot claim digest failed: {e}"))
            })?;

            let reservation_scope = ReservationScope {
                project_id: task.contract.project_id.clone(),
                root_id: task.contract.root_id.clone(),
                task_id: task.contract.task_id.clone(),
                attempt_id: policy.next_attempt_id.clone(),
                owner: policy.owner.clone(),
                incarnation: policy.incarnation.clone(),
                contract_revision: task.contract.revision,
                contract_digest,
                authority_ref: task.contract.authority_ref.clone(),
                resource_claims_digest: resource_claims_digest.clone(),
                account_id: policy.account_id.clone(),
                unit: policy.unit.clone(),
            };

            let reservation_event = ReservationEvent::Reserve {
                event_id: policy.reservation_event_id.clone(),
                reservation_id: policy.reservation_id.clone(),
                scope: reservation_scope.clone(),
                correlation_id: policy.correlation_id.clone(),
                amount,
            };

            let next_ledger = reduce_reservation(ledger, reservation_event.clone())?;
            let pre_reservation_capacity = capacity.clone();
            let mut updated_capacity = capacity;
            let in_use = updated_capacity.in_use.entry(spec.id.clone()).or_insert(0);
            *in_use = in_use.checked_add(1).ok_or_else(|| {
                ModelSelectionError::Invalid("in_use slot counter overflow".into())
            })?;

            let tier_attempt_number = attempts_by_tier
                .get(&spec.tier)
                .copied()
                .unwrap_or(0)
                .checked_add(1)
                .ok_or_else(|| ModelSelectionError::Invalid("tier attempt overflow".into()))?;
            let escalated_from = current_model.filter(|prev| prev != &spec.id);

            Ok(ModelSlotDecision::Ready {
                proposal: Box::new(ModelSlotReservationProposal {
                    selected_model_id: spec.id.clone(),
                    selected_model_spec: (*spec).clone(),
                    provider: spec.provider.clone(),
                    provider_model: spec.provider_model.clone(),
                    tier: spec.tier,
                    node_id: spec.node_id.clone(),
                    attempt_number: used_attempts + 1,
                    tier_attempt_number,
                    escalated_from,
                    usage_cost,
                    selection_request,
                    slot_claim,
                    resource_claims_digest,
                    reservation_scope,
                    reservation_event,
                    next_ledger,
                    pre_reservation_capacity,
                    updated_capacity,
                    proposal_only: true,
                    writer_atomicity_required: true,
                    execution_granted: false,
                }),
            })
        }
    }
}

/// Explicit disposition reported for an attempt's resource/budget usage.
/// Zero measured spend is NEVER automatically converted to `ConfirmedNoUsage`;
/// a trusted closure verifier must explicitly attest `ConfirmedNoUsage` vs `SettledAndTerminated`.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UsageDisposition {
    ConfirmedNoUsage,
    SettledAndTerminated,
    IncompleteOrUnknown,
}

/// Raw usage and reconciliation report submitted for settlement evaluation.
/// Deserializing this struct grants no settlement or slot-release authority; it must be
/// validated by a [`UsageClosureVerifier`] to produce a non-deserializable [`VerifiedUsageClosure`].
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct UsageReconciliationReport {
    pub reservation_id: String,
    pub attempt_id: AttemptId,
    pub model_id: String,
    pub measured_amount: Option<MinorUnits>,
    pub disposition: UsageDisposition,
    pub usage_complete: Option<bool>,
    pub effects_reconciled: Option<bool>,
    pub resource_claims_reconciled: Option<bool>,
    pub scope_digest: String,
    pub resource_claims_digest: String,
    pub evidence_ref: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VerifiedUsageOutcome {
    /// All usage, effects, and resource claims were verified complete by a trusted adapter;
    /// terminal `FinalEvidence` may close the reservation and release the slot on first settlement.
    Terminal {
        settled_amount: MinorUnits,
        final_evidence: FinalEvidence,
    },
    /// Usage or reconciliation is missing, incomplete, or unauthenticated; optional interim
    /// spend may be recorded, but the reservation and inference slot MUST remain held.
    HeldIncomplete {
        interim_amount: Option<MinorUnits>,
        reason: String,
    },
}

/// Non-deserializable token proving that a [`UsageClosureVerifier`] adapter authenticated
/// the usage and closure reconciliation for a specific reservation and `InferenceSlotClaim`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedUsageClosure {
    reservation_id: String,
    attempt_id: AttemptId,
    model_id: String,
    scope_digest: String,
    resource_claims_digest: String,
    trusted_closure_verified: bool,
    outcome: VerifiedUsageOutcome,
}

impl VerifiedUsageClosure {
    /// Construct a `VerifiedUsageClosure` after checking exact reservation/claim bindings
    /// and explicit reconciliation flags. Never fabricates `true` flags or `ConfirmedNoUsage`.
    pub fn authenticate(
        reservation: &Reservation,
        slot_claim: &InferenceSlotClaim,
        report: &UsageReconciliationReport,
        closure_authenticated: bool,
    ) -> Result<Self, ModelSelectionError> {
        let expected_claims_digest = content_digest(slot_claim)
            .map_err(|e| ModelSelectionError::Invalid(format!("slot claim digest failed: {e}")))?;
        let expected_scope_digest = content_digest(&reservation.scope)
            .map_err(|e| ModelSelectionError::Invalid(format!("scope digest failed: {e}")))?;

        if reservation.id != report.reservation_id
            || slot_claim.attempt_id != report.attempt_id
            || slot_claim.model_id != report.model_id
            || reservation.scope.resource_claims_digest != expected_claims_digest
            || reservation.scope.project_id != slot_claim.project_id
            || reservation.scope.root_id != slot_claim.root_id
            || reservation.scope.task_id != slot_claim.task_id
            || reservation.scope.attempt_id != slot_claim.attempt_id
            || reservation.scope.contract_revision != slot_claim.contract_revision
            || reservation.scope.contract_digest != slot_claim.contract_digest
            || report.resource_claims_digest != expected_claims_digest
            || report.scope_digest != expected_scope_digest
        {
            return Err(ModelSelectionError::Reservation(
                ReservationError::BindingMismatch,
            ));
        }

        if !closure_authenticated || report.evidence_ref.trim().is_empty() {
            return Ok(Self {
                reservation_id: reservation.id.clone(),
                attempt_id: slot_claim.attempt_id.clone(),
                model_id: slot_claim.model_id.clone(),
                scope_digest: expected_scope_digest,
                resource_claims_digest: expected_claims_digest,
                trusted_closure_verified: false,
                outcome: VerifiedUsageOutcome::HeldIncomplete {
                    interim_amount: None,
                    reason: "untrusted_or_unauthenticated_closure".into(),
                },
            });
        }

        let Some(measured_amount) = report.measured_amount.clone() else {
            return Ok(Self {
                reservation_id: reservation.id.clone(),
                attempt_id: slot_claim.attempt_id.clone(),
                model_id: slot_claim.model_id.clone(),
                scope_digest: expected_scope_digest,
                resource_claims_digest: expected_claims_digest,
                trusted_closure_verified: true,
                outcome: VerifiedUsageOutcome::HeldIncomplete {
                    interim_amount: None,
                    reason: "missing_or_unknown_usage_amount".into(),
                },
            });
        };

        if report.disposition == UsageDisposition::IncompleteOrUnknown
            || report.usage_complete != Some(true)
            || report.effects_reconciled != Some(true)
            || report.resource_claims_reconciled != Some(true)
        {
            return Ok(Self {
                reservation_id: reservation.id.clone(),
                attempt_id: slot_claim.attempt_id.clone(),
                model_id: slot_claim.model_id.clone(),
                scope_digest: expected_scope_digest,
                resource_claims_digest: expected_claims_digest,
                trusted_closure_verified: true,
                outcome: VerifiedUsageOutcome::HeldIncomplete {
                    interim_amount: Some(measured_amount),
                    reason: "incomplete_usage_or_reconciliation_flags".into(),
                },
            });
        }

        let reconciliation = ReconciliationEvidence {
            scope_digest: expected_scope_digest.clone(),
            resource_claims_digest: expected_claims_digest.clone(),
            usage_complete: report.usage_complete,
            effects_reconciled: report.effects_reconciled,
            resource_claims_reconciled: report.resource_claims_reconciled,
        };

        let final_evidence = match report.disposition {
            UsageDisposition::ConfirmedNoUsage => {
                if measured_amount.get() != 0 || reservation.settled.get() != 0 {
                    return Err(ModelSelectionError::Reservation(
                        ReservationError::InvalidSettlement,
                    ));
                }
                FinalEvidence::ConfirmedNoUsage {
                    evidence_ref: report.evidence_ref.clone(),
                    reconciliation,
                }
            }
            UsageDisposition::SettledAndTerminated => FinalEvidence::SettledAndTerminated {
                evidence_ref: report.evidence_ref.clone(),
                reconciliation,
            },
            UsageDisposition::IncompleteOrUnknown => unreachable!(),
        };

        Ok(Self {
            reservation_id: reservation.id.clone(),
            attempt_id: slot_claim.attempt_id.clone(),
            model_id: slot_claim.model_id.clone(),
            scope_digest: expected_scope_digest,
            resource_claims_digest: expected_claims_digest,
            trusted_closure_verified: true,
            outcome: VerifiedUsageOutcome::Terminal {
                settled_amount: measured_amount,
                final_evidence,
            },
        })
    }
}

/// Adapter trait required before terminal slot/budget settlement can close a reservation
/// or release an occupied inference slot.
pub trait UsageClosureVerifier {
    fn verify_usage_closure(
        &self,
        reservation: &Reservation,
        slot_claim: &InferenceSlotClaim,
        report: &UsageReconciliationReport,
    ) -> Result<VerifiedUsageClosure, ModelSelectionError>;
}

/// Authenticated closure receipt registered with [`AuthenticatedUsageVerifier`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthenticatedClosureReceipt {
    pub reservation_id: String,
    pub attempt_id: AttemptId,
    pub model_id: String,
    pub measured_amount: MinorUnits,
    pub disposition: UsageDisposition,
    pub evidence_ref: String,
}

/// Concrete trusted [`UsageClosureVerifier`] backed by authenticated runtime/verifier receipts.
#[derive(Clone, Debug, Default)]
pub struct AuthenticatedUsageVerifier {
    receipts: BTreeMap<String, AuthenticatedClosureReceipt>,
}

impl AuthenticatedUsageVerifier {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_receipt(mut self, receipt: AuthenticatedClosureReceipt) -> Self {
        self.receipts
            .insert(receipt.reservation_id.clone(), receipt);
        self
    }

    pub fn insert_receipt(&mut self, receipt: AuthenticatedClosureReceipt) {
        self.receipts
            .insert(receipt.reservation_id.clone(), receipt);
    }
}

impl UsageClosureVerifier for AuthenticatedUsageVerifier {
    fn verify_usage_closure(
        &self,
        reservation: &Reservation,
        slot_claim: &InferenceSlotClaim,
        report: &UsageReconciliationReport,
    ) -> Result<VerifiedUsageClosure, ModelSelectionError> {
        let authenticated = self.receipts.get(&reservation.id).is_some_and(|receipt| {
            receipt.reservation_id == report.reservation_id
                && receipt.attempt_id == report.attempt_id
                && receipt.model_id == report.model_id
                && Some(&receipt.measured_amount) == report.measured_amount.as_ref()
                && receipt.disposition == report.disposition
                && receipt.evidence_ref == report.evidence_ref
        });
        VerifiedUsageClosure::authenticate(reservation, slot_claim, report, authenticated)
    }
}

/// Explicit untrusted adapter that never authenticates closure reports; produces
/// held proposals (`trusted_closure_verified: false`, `slot_released: false`) without
/// mutating the ledger or releasing inference slots.
#[derive(Clone, Copy, Debug, Default)]
pub struct UntrustedProposalUsageAdapter;

impl UsageClosureVerifier for UntrustedProposalUsageAdapter {
    fn verify_usage_closure(
        &self,
        reservation: &Reservation,
        slot_claim: &InferenceSlotClaim,
        report: &UsageReconciliationReport,
    ) -> Result<VerifiedUsageClosure, ModelSelectionError> {
        VerifiedUsageClosure::authenticate(reservation, slot_claim, report, false)
    }
}

/// Settlement proposal state returned by [`settle_model_slot_attempt`].
/// Single-writer serialization and atomic commit are required before real admission.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelSlotSettlementProposal {
    pub reservation_id: String,
    pub attempt_id: AttemptId,
    pub model_id: String,
    pub settlement_event: Option<ReservationEvent>,
    pub proposed_ledger: ReservationLedger,
    pub proposed_capacity: CapacitySnapshot,
    /// True only on the first legitimate terminal transition (`outstanding: true -> false`).
    /// Always `false` on idempotent replay or when usage/reconciliation is incomplete or untrusted.
    pub slot_released: bool,
    /// True when `settlement_event_id` was already recorded in `processed_events` with
    /// an identical event digest.
    pub idempotent_replay: bool,
    pub trusted_closure_verified: bool,
    pub proposal_only: bool,
    pub writer_atomicity_required: bool,
}

fn resolve_settlement_event_for_ledger(
    ledger: &ReservationLedger,
    existing: &Reservation,
    event_id: String,
    reservation_id: String,
    amount: MinorUnits,
    final_evidence: Option<FinalEvidence>,
) -> Result<(ReservationEvent, bool), ModelSelectionError> {
    if let Some(prior_digest) = ledger.processed_events.get(&event_id) {
        for rev in (1..=existing.revision).rev() {
            let candidate = ReservationEvent::Settle {
                event_id: event_id.clone(),
                reservation_id: reservation_id.clone(),
                binding: ReservationBinding {
                    scope: existing.scope.clone(),
                    correlation_id: existing.correlation_id.clone(),
                    expected_revision: rev,
                },
                amount: amount.clone(),
                final_evidence: final_evidence.clone(),
            };
            let digest = content_digest(&candidate).map_err(|_| ReservationError::TooLarge)?;
            if &digest == prior_digest {
                return Ok((candidate, true));
            }
        }
        return Err(ModelSelectionError::Reservation(
            ReservationError::EventCollision,
        ));
    }

    Ok((
        ReservationEvent::Settle {
            event_id,
            reservation_id,
            binding: existing.binding(),
            amount,
            final_evidence,
        },
        false,
    ))
}

/// Propose settlement of an inference slot and budget reservation after an attempt,
/// requiring a [`UsageClosureVerifier`] adapter.
///
/// - Never fabricates `usage_complete`, `effects_reconciled`, `resource_claims_reconciled`,
///   or `ConfirmedNoUsage` from raw caller strings/amounts.
/// - Wrong, missing, unknown, or untrusted usage keeps the reservation and slot held.
/// - Mutates `proposed_capacity` ONLY on the first legitimate terminal settlement
///   (`outstanding: true -> false`), never on idempotent replay of `settlement_event_id`.
pub fn settle_model_slot_attempt<V: UsageClosureVerifier + ?Sized>(
    ledger: &ReservationLedger,
    capacity: &CapacitySnapshot,
    slot_claim: &InferenceSlotClaim,
    reservation_id: &str,
    settlement_event_id: impl Into<String>,
    report: &UsageReconciliationReport,
    verifier: &V,
) -> Result<ModelSlotSettlementProposal, ModelSelectionError> {
    if slot_claim.slots_reserved == 0 {
        return Err(ModelSelectionError::Invalid(
            "slot_claim.slots_reserved must be at least 1".into(),
        ));
    }
    let existing = ledger
        .reservations
        .get(reservation_id)
        .ok_or_else(|| ReservationError::UnknownReservation(reservation_id.to_owned()))?;

    let closure = verifier.verify_usage_closure(existing, slot_claim, report)?;
    let event_id = settlement_event_id.into();

    match closure.outcome {
        VerifiedUsageOutcome::HeldIncomplete { interim_amount, .. } => {
            if !closure.trusted_closure_verified || interim_amount.is_none() {
                return Ok(ModelSlotSettlementProposal {
                    reservation_id: existing.id.clone(),
                    attempt_id: slot_claim.attempt_id.clone(),
                    model_id: slot_claim.model_id.clone(),
                    settlement_event: None,
                    proposed_ledger: ledger.clone(),
                    proposed_capacity: capacity.clone(),
                    slot_released: false,
                    idempotent_replay: false,
                    trusted_closure_verified: closure.trusted_closure_verified,
                    proposal_only: true,
                    writer_atomicity_required: true,
                });
            }

            let amount = interim_amount.expect("checked Some above");
            let (event, is_replay) = resolve_settlement_event_for_ledger(
                ledger,
                existing,
                event_id,
                reservation_id.to_owned(),
                amount,
                None,
            )?;
            let next_ledger = reduce_reservation(ledger, event.clone())?;
            Ok(ModelSlotSettlementProposal {
                reservation_id: existing.id.clone(),
                attempt_id: slot_claim.attempt_id.clone(),
                model_id: slot_claim.model_id.clone(),
                settlement_event: Some(event),
                proposed_ledger: next_ledger,
                proposed_capacity: capacity.clone(),
                slot_released: false,
                idempotent_replay: is_replay,
                trusted_closure_verified: true,
                proposal_only: true,
                writer_atomicity_required: true,
            })
        }
        VerifiedUsageOutcome::Terminal {
            settled_amount,
            final_evidence,
        } => {
            let was_outstanding = existing.outstanding;
            let (event, is_replay) = resolve_settlement_event_for_ledger(
                ledger,
                existing,
                event_id,
                reservation_id.to_owned(),
                settled_amount,
                Some(final_evidence),
            )?;
            let next_ledger = reduce_reservation(ledger, event.clone())?;
            let settled_now = next_ledger
                .reservations
                .get(reservation_id)
                .is_some_and(|r| !r.outstanding);
            let first_legitimate_settlement = !is_replay
                && was_outstanding
                && settled_now
                && next_ledger.revision > ledger.revision;

            let mut proposed_capacity = capacity.clone();
            if first_legitimate_settlement {
                if let Some(in_use) = proposed_capacity.in_use.get_mut(&slot_claim.model_id) {
                    *in_use = in_use
                        .checked_sub(slot_claim.slots_reserved)
                        .ok_or_else(|| {
                            ModelSelectionError::Invalid(
                                "cannot release more inference slots than currently in_use".into(),
                            )
                        })?;
                } else {
                    return Err(ModelSelectionError::Invalid(format!(
                        "cannot release slot for model {} not tracked in CapacitySnapshot",
                        slot_claim.model_id
                    )));
                }
            }

            Ok(ModelSlotSettlementProposal {
                reservation_id: existing.id.clone(),
                attempt_id: slot_claim.attempt_id.clone(),
                model_id: slot_claim.model_id.clone(),
                settlement_event: Some(event),
                proposed_ledger: next_ledger,
                proposed_capacity,
                slot_released: first_legitimate_settlement,
                idempotent_replay: is_replay,
                trusted_closure_verified: true,
                proposal_only: true,
                writer_atomicity_required: true,
            })
        }
    }
}

/// Budget and identity parameters for reserving all slots of a parallel `BestOfConfig`
/// strategy plan (variants + synthesis reserve).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StrategyReservationRequest {
    pub run_id: String,
    pub contract_digest: String,
    pub baseline_sha: String,
    pub oracle_digest: String,
    pub owner: PrincipalId,
    pub incarnation: String,
    pub attempt_id: AttemptId,
    pub account_id: AccountId,
    pub unit: CurrencyUnit,
    pub per_unit_budgets: BTreeMap<String, MinorUnits>,
    pub reservation_id: String,
    pub reservation_event_id: String,
    pub correlation_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StrategyReservationProposal {
    pub plan: BestOfPlan,
    pub resource_claims_digest: String,
    pub reservation_scope: ReservationScope,
    pub reservation_event: ReservationEvent,
    pub next_ledger: ReservationLedger,
    pub proposal_only: bool,
    pub writer_atomicity_required: bool,
    pub execution_granted: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum StrategySlotDecision {
    Ready {
        proposal: Box<StrategyReservationProposal>,
    },
    Waiting {
        reason: StrategyWait,
    },
}

/// Wire parallel-candidate (`BestOfConfig`) planning to verified `ObservedModelSlotState`
/// and atomic budget reservation across all reserved variant + synthesis slots.
pub fn propose_strategy_slot_reservations(
    config: &BestOfConfig,
    contract: &TaskContract,
    request: &StrategyReservationRequest,
    registry: &ModelRegistry,
    observed: &ObservedModelSlotState,
    ledger: &ReservationLedger,
    now_tick: u64,
) -> Result<StrategySlotDecision, ModelSelectionError> {
    let strategy_capacity = observed.to_strategy_capacity(registry)?;
    let decision = plan_variants(
        config,
        contract,
        &request.contract_digest,
        &request.baseline_sha,
        &request.oracle_digest,
        &request.run_id,
        registry,
        &strategy_capacity,
        now_tick,
    )?;

    match decision {
        PlanDecision::Waiting { reason } => Ok(StrategySlotDecision::Waiting { reason }),
        PlanDecision::Ready { plan } => {
            let mut total_budget = 0_u64;
            for (model_id, slots) in &plan.reserved_slots {
                let unit_budget = request.per_unit_budgets.get(model_id).ok_or_else(|| {
                    ModelSelectionError::Invalid(format!(
                        "missing per-unit budget for strategy model {model_id}"
                    ))
                })?;
                let model_total = unit_budget
                    .get()
                    .checked_mul(u64::from(*slots))
                    .ok_or(ReservationError::Overflow)?;
                total_budget = total_budget
                    .checked_add(model_total)
                    .ok_or(ReservationError::Overflow)?;
            }

            let resource_claims_digest = content_digest(&(
                &plan.run_digest,
                &plan.registry_revision,
                &observed.snapshot_revision,
                &plan.reserved_slots,
            ))
            .map_err(|e| {
                ModelSelectionError::Invalid(format!("strategy claims digest failed: {e}"))
            })?;

            let reservation_scope = ReservationScope {
                project_id: contract.project_id.clone(),
                root_id: contract.root_id.clone(),
                task_id: contract.task_id.clone(),
                attempt_id: request.attempt_id.clone(),
                owner: request.owner.clone(),
                incarnation: request.incarnation.clone(),
                contract_revision: contract.revision,
                contract_digest: request.contract_digest.clone(),
                authority_ref: contract.authority_ref.clone(),
                resource_claims_digest: resource_claims_digest.clone(),
                account_id: request.account_id.clone(),
                unit: request.unit.clone(),
            };

            let reservation_event = ReservationEvent::Reserve {
                event_id: request.reservation_event_id.clone(),
                reservation_id: request.reservation_id.clone(),
                scope: reservation_scope.clone(),
                correlation_id: request.correlation_id.clone(),
                amount: MinorUnits::new(total_budget),
            };
            let next_ledger = reduce_reservation(ledger, reservation_event.clone())?;

            Ok(StrategySlotDecision::Ready {
                proposal: Box::new(StrategyReservationProposal {
                    plan: *plan,
                    resource_claims_digest,
                    reservation_scope,
                    reservation_event,
                    next_ledger,
                    proposal_only: true,
                    writer_atomicity_required: true,
                    execution_granted: false,
                }),
            })
        }
    }
}

/// Raw, unverified variant candidate submission for comparison prior to synthesis.
/// Deserializing this struct never confers verification trust; a [`CandidateCustodyVerifier`]
/// must authenticate custody and independent verification before any candidate can enter synthesis.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VariantCandidateSubmission {
    pub unit_id: String,
    pub worker_principal: PrincipalId,
    pub report: VariantReport,
    pub claimed_verification: Option<CandidateDecisionEvidence>,
    pub claimed_checks_passed: bool,
}

/// Non-deserializable token proving that a [`CandidateCustodyVerifier`] adapter
/// authenticated independent verifier custody and check receipts for a variant candidate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttestedCandidateCustody {
    unit_id: String,
    commit_sha: String,
    tree_sha: String,
    verifier: PrincipalId,
}

impl AttestedCandidateCustody {
    pub fn authenticate(
        plan: &BestOfPlan,
        submission: &VariantCandidateSubmission,
        candidate: &CandidateRef,
        custody_authenticated: bool,
    ) -> Option<Self> {
        if !custody_authenticated || !submission.claimed_checks_passed {
            return None;
        }
        let ev = submission.claimed_verification.as_ref()?;
        if ev.verifier == submission.worker_principal
            || ev.contract_digest != plan.original.contract_digest
            || ev.source_commit_sha != candidate.commit_sha
            || ev.source_tree_sha != candidate.tree_sha
            || !valid_sha256_digest(&ev.verification_policy_digest)
            || !valid_sha256_digest(&ev.receipt_digest)
            || !valid_sha256_digest(&ev.custody_digest)
            || !valid_sha256_digest(&ev.closure_digest)
        {
            return None;
        }
        Some(Self {
            unit_id: submission.unit_id.clone(),
            commit_sha: candidate.commit_sha.clone(),
            tree_sha: candidate.tree_sha.clone(),
            verifier: ev.verifier.clone(),
        })
    }
}

/// Non-deserializable verifier/custody adapter required to attest candidate outputs
/// before synthesis planning.
pub trait CandidateCustodyVerifier {
    fn verify_candidate_custody(
        &self,
        plan: &BestOfPlan,
        submission: &VariantCandidateSubmission,
        candidate: &CandidateRef,
    ) -> Option<AttestedCandidateCustody>;
}

/// Authenticated custody & oracle receipt registered with [`TrustedCustodyVerifierAdapter`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthenticatedCandidateReceipt {
    pub unit_id: String,
    pub contract_digest: String,
    pub commit_sha: String,
    pub tree_sha: String,
    pub receipt_digest: String,
    pub custody_digest: String,
    pub closure_digest: String,
    pub verifier: PrincipalId,
}

/// Concrete [`CandidateCustodyVerifier`] that requires an explicit authenticated receipt
/// matching the candidate's commit, tree, custody, receipt, closure digests, and independent
/// verifier identity. Arbitrary deserialized JSON without a registered receipt is rejected.
#[derive(Clone, Debug, Default)]
pub struct TrustedCustodyVerifierAdapter {
    receipts: BTreeMap<String, AuthenticatedCandidateReceipt>,
}

impl TrustedCustodyVerifierAdapter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_receipt(mut self, receipt: AuthenticatedCandidateReceipt) -> Self {
        self.receipts.insert(receipt.unit_id.clone(), receipt);
        self
    }

    pub fn insert_receipt(&mut self, receipt: AuthenticatedCandidateReceipt) {
        self.receipts.insert(receipt.unit_id.clone(), receipt);
    }
}

impl CandidateCustodyVerifier for TrustedCustodyVerifierAdapter {
    fn verify_candidate_custody(
        &self,
        plan: &BestOfPlan,
        submission: &VariantCandidateSubmission,
        candidate: &CandidateRef,
    ) -> Option<AttestedCandidateCustody> {
        let ev = submission.claimed_verification.as_ref()?;
        let authenticated = self.receipts.get(&submission.unit_id).is_some_and(|r| {
            r.unit_id == submission.unit_id
                && r.contract_digest == plan.original.contract_digest
                && r.commit_sha == candidate.commit_sha
                && r.tree_sha == candidate.tree_sha
                && r.receipt_digest == ev.receipt_digest
                && r.custody_digest == ev.custody_digest
                && r.closure_digest == ev.closure_digest
                && r.verifier == ev.verifier
                && r.verifier != submission.worker_principal
        });
        AttestedCandidateCustody::authenticate(plan, submission, candidate, authenticated)
    }
}

/// Explicit untrusted comparison adapter that never attests candidate custody; all
/// candidate claims are downgraded to `ReconciledFailure` so untrusted JSON cannot
/// enter synthesis as a valid candidate.
#[derive(Clone, Copy, Debug, Default)]
pub struct UntrustedProposalCustodyAdapter;

impl CandidateCustodyVerifier for UntrustedProposalCustodyAdapter {
    fn verify_candidate_custody(
        &self,
        plan: &BestOfPlan,
        submission: &VariantCandidateSubmission,
        candidate: &CandidateRef,
    ) -> Option<AttestedCandidateCustody> {
        AttestedCandidateCustody::authenticate(plan, submission, candidate, false)
    }
}

/// Proposal-only comparison summary produced alongside `SynthesisDecision`.
/// Never grants candidate acceptance or execution authority.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CandidateComparisonProposal {
    pub custody_attested_units: Vec<String>,
    pub downgraded_to_failure_units: Vec<String>,
    pub synthesis_decision_digest: String,
    pub proposal_only: bool,
    pub acceptance_granted: bool,
    pub execution_granted: bool,
}

/// Compare parallel variant candidates through a non-deserializable [`CandidateCustodyVerifier`]
/// adapter (enforcing authenticated custody receipts, `verifier != worker_principal`, and
/// matching contract/commit/tree digests), downgrade unattested or failed candidates to
/// `ReconciledFailure`, and produce an explicit non-accepting synthesis proposal.
pub fn compare_candidates_and_plan_synthesis<V: CandidateCustodyVerifier + ?Sized>(
    plan: &BestOfPlan,
    entries: &[VariantCandidateSubmission],
    verifier: &V,
    registry: &ModelRegistry,
    observed: &ObservedModelSlotState,
    now_tick: u64,
) -> Result<(SynthesisDecision, CandidateComparisonProposal), ModelSelectionError> {
    let mut reconciled_reports = Vec::with_capacity(entries.len());
    let mut custody_attested_units = Vec::new();
    let mut downgraded_to_failure_units = Vec::new();

    for entry in entries {
        if entry.unit_id != entry.report.unit_id {
            return Err(ModelSelectionError::Invalid(
                "entry unit_id does not match VariantReport.unit_id".into(),
            ));
        }
        match &entry.report.outcome {
            VariantOutcome::Unresolved | VariantOutcome::ReconciledFailure { .. } => {
                reconciled_reports.push(entry.report.clone());
            }
            VariantOutcome::Candidate {
                candidate,
                diagnostic,
            } => {
                if verifier
                    .verify_candidate_custody(plan, entry, candidate)
                    .is_some()
                {
                    custody_attested_units.push(entry.unit_id.clone());
                    reconciled_reports.push(entry.report.clone());
                } else {
                    downgraded_to_failure_units.push(entry.unit_id.clone());
                    reconciled_reports.push(VariantReport {
                        unit_id: entry.report.unit_id.clone(),
                        original: entry.report.original.clone(),
                        outcome: VariantOutcome::ReconciledFailure {
                            diagnostic: diagnostic.clone(),
                        },
                    });
                }
            }
        }
    }

    custody_attested_units.sort();
    downgraded_to_failure_units.sort();

    let strategy_capacity = observed.to_strategy_capacity(registry)?;
    let decision = plan_synthesis(
        plan,
        &reconciled_reports,
        registry,
        &strategy_capacity,
        now_tick,
    )?;
    let synthesis_decision_digest = content_digest(&decision).map_err(|e| {
        ModelSelectionError::Invalid(format!("synthesis decision digest failed: {e}"))
    })?;

    Ok((
        decision,
        CandidateComparisonProposal {
            custody_attested_units,
            downgraded_to_failure_units,
            synthesis_decision_digest,
            proposal_only: true,
            acceptance_granted: false,
            execution_granted: false,
        },
    ))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SynthesisFallbackReservationProposal {
    pub unit: StrategyWorkUnit,
    pub slot_claim: InferenceSlotClaim,
    pub resource_claims_digest: String,
    pub reservation_scope: ReservationScope,
    pub reservation_event: ReservationEvent,
    pub next_ledger: ReservationLedger,
    pub updated_capacity: CapacitySnapshot,
    pub proposal_only: bool,
    pub writer_atomicity_required: bool,
    pub execution_granted: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SynthesisFallbackSlotDecision {
    Ready {
        proposal: Box<SynthesisFallbackReservationProposal>,
    },
    Waiting {
        reason: StrategyWait,
    },
}

/// Wire `plan_synthesis_fallback` (e.g., Ornith synthesis failure escalating to Luna)
/// to an explicit inference slot claim and atomic budget reservation on `ReservationLedger`.
#[allow(clippy::too_many_arguments)]
pub fn propose_synthesis_fallback_reservation(
    plan: &BestOfPlan,
    synthesis_unit: &StrategyWorkUnit,
    failure: &QualifiedSynthesisFailure,
    registry: &ModelRegistry,
    observed: &ObservedModelSlotState,
    ledger: &ReservationLedger,
    owner: PrincipalId,
    incarnation: String,
    attempt_id: AttemptId,
    account_id: AccountId,
    unit: CurrencyUnit,
    fallback_budget: MinorUnits,
    reservation_id: String,
    reservation_event_id: String,
    correlation_id: String,
    now_tick: u64,
) -> Result<SynthesisFallbackSlotDecision, ModelSelectionError> {
    let strategy_capacity = observed.to_strategy_capacity(registry)?;
    let decision = plan_synthesis_fallback(
        plan,
        synthesis_unit,
        failure,
        registry,
        &strategy_capacity,
        now_tick,
    )?;

    match decision {
        SynthesisDecision::Waiting { reason } => {
            Ok(SynthesisFallbackSlotDecision::Waiting { reason })
        }
        SynthesisDecision::DiagnosticProposal { .. } => {
            Ok(SynthesisFallbackSlotDecision::Waiting {
                reason: StrategyWait::RegistryRouteRequired,
            })
        }
        SynthesisDecision::Ready {
            unit: fallback_unit,
        } => {
            let capacity = observed.to_verified_capacity(registry, now_tick)?;
            let spec = registry
                .models
                .iter()
                .find(|m| m.id == fallback_unit.model_id)
                .ok_or_else(|| {
                    ModelSelectionError::Invalid(format!(
                        "fallback model {} missing from registry",
                        fallback_unit.model_id
                    ))
                })?;
            let spec_digest = model_config_digest(spec)?;
            let slot_claim = InferenceSlotClaim {
                project_id: plan.contract.project_id.clone(),
                root_id: plan.contract.root_id.clone(),
                task_id: plan.contract.task_id.clone(),
                attempt_id: attempt_id.clone(),
                contract_revision: plan.contract.revision,
                contract_digest: plan.original.contract_digest.clone(),
                model_id: spec.id.clone(),
                provider: spec.provider.clone(),
                provider_model: spec.provider_model.clone(),
                tier: spec.tier,
                node_id: spec.node_id.clone(),
                slots_reserved: 1,
                registry_revision: registry.revision.clone(),
                snapshot_revision: observed.snapshot_revision.clone(),
                model_config_digest: spec_digest,
            };
            let resource_claims_digest = content_digest(&slot_claim).map_err(|e| {
                ModelSelectionError::Invalid(format!("fallback slot claim digest failed: {e}"))
            })?;

            let reservation_scope = ReservationScope {
                project_id: plan.contract.project_id.clone(),
                root_id: plan.contract.root_id.clone(),
                task_id: plan.contract.task_id.clone(),
                attempt_id,
                owner,
                incarnation,
                contract_revision: plan.contract.revision,
                contract_digest: plan.original.contract_digest.clone(),
                authority_ref: plan.contract.authority_ref.clone(),
                resource_claims_digest: resource_claims_digest.clone(),
                account_id,
                unit,
            };

            let reservation_event = ReservationEvent::Reserve {
                event_id: reservation_event_id,
                reservation_id,
                scope: reservation_scope.clone(),
                correlation_id,
                amount: fallback_budget,
            };
            let next_ledger = reduce_reservation(ledger, reservation_event.clone())?;

            let mut updated_capacity = capacity;
            let in_use = updated_capacity.in_use.entry(spec.id.clone()).or_insert(0);
            *in_use = in_use.checked_add(1).ok_or_else(|| {
                ModelSelectionError::Invalid("fallback in_use slot counter overflow".into())
            })?;

            Ok(SynthesisFallbackSlotDecision::Ready {
                proposal: Box::new(SynthesisFallbackReservationProposal {
                    unit: *fallback_unit,
                    slot_claim,
                    resource_claims_digest,
                    reservation_scope,
                    reservation_event,
                    next_ledger,
                    updated_capacity,
                    proposal_only: true,
                    writer_atomicity_required: true,
                    execution_granted: false,
                }),
            })
        }
    }
}

/// Async server helper that reads the private `RegistryFile` and evaluates
/// `propose_model_slot_reservation` off the async runtime thread pool.
pub async fn select_and_propose_model_slot(
    registry_file: RegistryFile,
    task: TaskView,
    history: TaskModelAttemptHistory,
    policy: ModelEscalationPolicy,
    observed: ObservedModelSlotState,
    ledger: ReservationLedger,
    now_tick: u64,
) -> Result<Json<ModelSlotDecision>, ModelsApiError> {
    let result = tokio::task::spawn_blocking(move || {
        let registry = registry_file.read()?;
        propose_model_slot_reservation(
            &registry, &task, &history, &policy, &observed, &ledger, now_tick,
        )
    })
    .await
    .map_err(|_| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error":"model_selection_worker_unavailable"})),
        )
    })?;

    result.map(Json).map_err(|err| match err {
        ModelSelectionError::Registry(RegistryError::Busy) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error":"model_registry_busy","retry_after_s":1})),
        ),
        ModelSelectionError::Registry(RegistryError::Io(_) | RegistryError::Json(_)) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error":"model_registry_unavailable"})),
        ),
        other => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": "model_selection_rejected",
                "detail": other.to_string(),
            })),
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use admiralty_core::{AcceptanceCriterion, CheckSpec, TaskLifecycleView};
    use admiralty_runtime::reservations::ScopeLimits;
    use admiralty_runtime::strategies::{CandidateRef, SynthesisFallback};

    fn ornith_luna_registry() -> ModelRegistry {
        let mut reg: ModelRegistry =
            serde_json::from_str(include_str!("../../../configs/examples/models.json"))
                .expect("valid example models.json");
        reg.revision = "1".into();
        reg.validate().expect("registry validates");
        reg
    }

    fn sample_contract(max_attempts: u16) -> TaskContract {
        TaskContract {
            schema_version: 1,
            project_id: ProjectId::new("project-alpha").expect("valid project_id"),
            root_id: RootId::new("root-main").expect("valid root_id"),
            task_id: TaskId::new("task-gate4").expect("valid task_id"),
            revision: 1,
            title: "Wire own-model selection and escalation".into(),
            objective: "Connect Ornith->Luna fallback and slot reservation proposals".into(),
            non_goals: vec!["No direct launch".into()],
            owned_paths: vec!["crates/admiralty-server/src/model_selection.rs".into()],
            forbidden_paths: vec!["crates/admiralty-core/**".into()],
            read_first: vec!["docs/plans/admiralty-v2/REMAINING_AUDIT_2026-10-18.md".into()],
            dependencies: vec![],
            criteria: vec![AcceptanceCriterion {
                id: "bounded-fallback".into(),
                text: "Ornith retries within tier cap then escalates to Luna with slot+budget reservation".into(),
                check_ids: vec!["unit-test".into()],
            }],
            checks: vec![CheckSpec {
                id: "unit-test".into(),
                program: "cargo".into(),
                args: vec!["test".into()],
                cwd: ".".into(),
                timeout_secs: 60,
            }],
            authority_ref: "authority-gate4-v1".into(),
            max_attempts,
            simulated: true,
        }
    }

    fn ready_task(max_attempts: u16) -> TaskView {
        TaskView {
            contract: sample_contract(max_attempts),
            state: SchedulingState::Ready,
            attempt_id: None,
            owner: None,
            candidate_ref: None,
            lifecycle: None,
        }
    }

    fn observed_slots(
        registry: &ModelRegistry,
        ornith_in_use: Option<u16>,
        luna_in_use: Option<u16>,
    ) -> ObservedModelSlotState {
        let mut digests = BTreeMap::new();
        for spec in &registry.models {
            digests.insert(
                spec.id.clone(),
                model_config_digest(spec).expect("model digest"),
            );
        }
        ObservedModelSlotState {
            registry_revision: registry.revision.clone(),
            snapshot_revision: "obs-snap-1".into(),
            observed_tick: 100,
            valid_through_tick: 120,
            in_use: BTreeMap::from([
                ("ornith".into(), ornith_in_use),
                ("gpt-6-luna".into(), luna_in_use),
            ]),
            unavailable: BTreeSet::new(),
            observed_model_digests: digests,
        }
    }

    fn funded_ledger(limit: u64) -> ReservationLedger {
        let account_id = AccountId::new("acct-inference").expect("account_id");
        let unit = CurrencyUnit::new("USD", 4).expect("unit");
        reduce_reservation(
            &ReservationLedger::new(),
            ReservationEvent::SetLimits {
                event_id: "evt-limits-1".into(),
                account_id,
                limits: ScopeLimits {
                    unit,
                    account: Some(MinorUnits::new(limit)),
                    project: Some(MinorUnits::new(limit)),
                    root: Some(MinorUnits::new(limit)),
                },
            },
        )
        .expect("funded ledger")
    }

    fn base_policy(attempt_num: usize, allow_escalation: bool) -> ModelEscalationPolicy {
        ModelEscalationPolicy {
            required_capabilities: BTreeSet::from(["coding".into(), "tool_calls".into()]),
            pinned_model: None,
            allow_escalation,
            allow_capacity_fallback: false,
            account_id: AccountId::new("acct-inference").expect("account_id"),
            unit: CurrencyUnit::new("USD", 4).expect("unit"),
            model_attempt_budgets: BTreeMap::from([
                ("ornith".into(), MinorUnits::new(10)),
                ("gpt-6-luna".into(), MinorUnits::new(250)),
            ]),
            owner: PrincipalId::new("worker-1").expect("owner"),
            incarnation: "inc-1".into(),
            next_attempt_id: AttemptId::new(format!("attempt-{attempt_num}")).expect("attempt_id"),
            reservation_id: format!("res-{attempt_num}"),
            reservation_event_id: format!("evt-res-{attempt_num}"),
            correlation_id: format!("corr-{attempt_num}"),
        }
    }

    fn diagnostic_ref(tag: char) -> ArtifactRef {
        ArtifactRef {
            sha256: format!("sha256:{}", tag.to_string().repeat(64)),
        }
    }

    fn sha256_tag(tag: char) -> String {
        format!("sha256:{}", tag.to_string().repeat(64))
    }

    fn build_authenticated_settlement(
        ledger: &ReservationLedger,
        slot_claim: &InferenceSlotClaim,
        reservation_id: &str,
        amount: MinorUnits,
        disposition: UsageDisposition,
        evidence_ref: &str,
    ) -> (UsageReconciliationReport, AuthenticatedUsageVerifier) {
        let existing = ledger
            .reservations
            .get(reservation_id)
            .expect("existing reservation");
        let scope_digest = content_digest(&existing.scope).expect("scope digest");
        let resource_claims_digest = content_digest(slot_claim).expect("slot claim digest");
        let report = UsageReconciliationReport {
            reservation_id: reservation_id.into(),
            attempt_id: slot_claim.attempt_id.clone(),
            model_id: slot_claim.model_id.clone(),
            measured_amount: Some(amount.clone()),
            disposition,
            usage_complete: Some(true),
            effects_reconciled: Some(true),
            resource_claims_reconciled: Some(true),
            scope_digest,
            resource_claims_digest,
            evidence_ref: evidence_ref.into(),
        };
        let verifier =
            AuthenticatedUsageVerifier::new().with_receipt(AuthenticatedClosureReceipt {
                reservation_id: reservation_id.into(),
                attempt_id: slot_claim.attempt_id.clone(),
                model_id: slot_claim.model_id.clone(),
                measured_amount: amount,
                disposition,
                evidence_ref: evidence_ref.into(),
            });
        (report, verifier)
    }

    #[test]
    fn ornith_to_luna_bounded_attempts_settlement_and_escalation_lifecycle() {
        let registry = ornith_luna_registry();
        let task = ready_task(4);
        let observed = observed_slots(&registry, Some(2), Some(1));
        let mut ledger = funded_ledger(1_000);
        let mut history = TaskModelAttemptHistory::default();

        // Attempt 1: selects tier-0 own-model `ornith`, reserves 1 slot and 10 minor units.
        let decision1 = propose_model_slot_reservation(
            &registry,
            &task,
            &history,
            &base_policy(1, true),
            &observed,
            &ledger,
            105,
        )
        .expect("attempt 1 proposal");
        let proposal1 = match decision1 {
            ModelSlotDecision::Ready { proposal } => *proposal,
            other => panic!("expected Ready for attempt 1, got {other:?}"),
        };
        assert_eq!(proposal1.selected_model_id, "ornith");
        assert_eq!(proposal1.tier, 0);
        assert_eq!(proposal1.attempt_number, 1);
        assert_eq!(proposal1.tier_attempt_number, 1);
        assert_eq!(proposal1.escalated_from, None);
        assert!(proposal1.proposal_only);
        assert!(proposal1.writer_atomicity_required);
        assert!(!proposal1.execution_granted);
        assert_eq!(proposal1.updated_capacity.in_use["ornith"], 3);
        ledger = proposal1.next_ledger;

        // While attempt 1 reservation is still outstanding, proposing attempt 2 must wait!
        history.attempts.push(ModelAttemptRecord {
            attempt_id: AttemptId::new("attempt-1").expect("attempt_id"),
            model_id: "ornith".into(),
            tier: 0,
            reservation_id: "res-1".into(),
            contract_revision: task.contract.revision,
            contract_digest: proposal1.slot_claim.contract_digest.clone(),
            failure: Some(AttemptFailureEvidence {
                reason: FailureReason::TestFail,
                qualified: true,
                diagnostic_artifact: diagnostic_ref('a'),
                closure_digest: sha256_tag('b'),
            }),
        });
        let blocked_unsettled = propose_model_slot_reservation(
            &registry,
            &task,
            &history,
            &base_policy(2, true),
            &observed,
            &ledger,
            106,
        )
        .expect("unsettled check");
        assert!(matches!(
            blocked_unsettled,
            ModelSlotDecision::Waiting {
                reason: ModelSelectionWait::UnresolvedPriorAttempt { .. },
                ..
            }
        ));

        // Settle attempt 1 reservation with authenticated closure verifier and release the slot.
        let (report1, verifier1) = build_authenticated_settlement(
            &ledger,
            &proposal1.slot_claim,
            "res-1",
            MinorUnits::new(8),
            UsageDisposition::SettledAndTerminated,
            "receipt:attempt-1",
        );
        let settled1 = settle_model_slot_attempt(
            &ledger,
            &proposal1.updated_capacity,
            &proposal1.slot_claim,
            "res-1",
            "evt-settle-1",
            &report1,
            &verifier1,
        )
        .expect("settle attempt 1");
        assert!(settled1.slot_released);
        assert!(!settled1.idempotent_replay);
        assert!(settled1.trusted_closure_verified);
        assert!(settled1.proposal_only);
        assert!(settled1.writer_atomicity_required);
        assert_eq!(settled1.proposed_capacity.in_use["ornith"], 2);
        ledger = settled1.proposed_ledger;

        // Attempt 2: ornith has task_max_attempts = 2, so after 1 TestFail it retries ornith (tier 0, attempt 2).
        let decision2 = propose_model_slot_reservation(
            &registry,
            &task,
            &history,
            &base_policy(2, true),
            &observed,
            &ledger,
            107,
        )
        .expect("attempt 2 proposal");
        let proposal2 = match decision2 {
            ModelSlotDecision::Ready { proposal } => *proposal,
            other => panic!("expected Ready on ornith retry, got {other:?}"),
        };
        assert_eq!(proposal2.selected_model_id, "ornith");
        assert_eq!(proposal2.attempt_number, 2);
        assert_eq!(proposal2.tier_attempt_number, 2);
        assert_eq!(proposal2.escalated_from, None);
        ledger = proposal2.next_ledger;

        // Settle attempt 2 with another qualified TestFail.
        let (report2, verifier2) = build_authenticated_settlement(
            &ledger,
            &proposal2.slot_claim,
            "res-2",
            MinorUnits::new(10),
            UsageDisposition::SettledAndTerminated,
            "receipt:attempt-2",
        );
        let settled2 = settle_model_slot_attempt(
            &ledger,
            &proposal2.updated_capacity,
            &proposal2.slot_claim,
            "res-2",
            "evt-settle-2",
            &report2,
            &verifier2,
        )
        .expect("settle attempt 2");
        assert!(settled2.slot_released);
        ledger = settled2.proposed_ledger;
        history.attempts.push(ModelAttemptRecord {
            attempt_id: AttemptId::new("attempt-2").expect("attempt_id"),
            model_id: "ornith".into(),
            tier: 0,
            reservation_id: "res-2".into(),
            contract_revision: task.contract.revision,
            contract_digest: proposal2.slot_claim.contract_digest.clone(),
            failure: Some(AttemptFailureEvidence {
                reason: FailureReason::TestFail,
                qualified: true,
                diagnostic_artifact: diagnostic_ref('c'),
                closure_digest: sha256_tag('d'),
            }),
        });

        // Without escalation authorization, attempt 3 waits with EscalationNotAuthorized.
        let denied_escalation = propose_model_slot_reservation(
            &registry,
            &task,
            &history,
            &base_policy(3, false),
            &observed,
            &ledger,
            108,
        )
        .expect("denied escalation");
        assert!(matches!(
            denied_escalation,
            ModelSlotDecision::Waiting {
                model_id: Some(ref id),
                reason: ModelSelectionWait::RegistryWait(WaitReason::EscalationNotAuthorized),
            } if id == "ornith"
        ));

        // With allow_escalation = true, attempt 3 escalates from ornith (tier 0) to gpt-6-luna (tier 1)!
        let decision3 = propose_model_slot_reservation(
            &registry,
            &task,
            &history,
            &base_policy(3, true),
            &observed,
            &ledger,
            108,
        )
        .expect("attempt 3 escalation to Luna");
        let proposal3 = match decision3 {
            ModelSlotDecision::Ready { proposal } => *proposal,
            other => panic!("expected Ready on Luna escalation, got {other:?}"),
        };
        assert_eq!(proposal3.selected_model_id, "gpt-6-luna");
        assert_eq!(proposal3.provider, Provider::CodexCli);
        assert_eq!(proposal3.tier, 1);
        assert_eq!(proposal3.attempt_number, 3);
        assert_eq!(proposal3.tier_attempt_number, 1);
        assert_eq!(proposal3.escalated_from.as_deref(), Some("ornith"));
        assert_eq!(proposal3.updated_capacity.in_use["gpt-6-luna"], 2);
        assert_eq!(
            proposal3
                .next_ledger
                .reservations
                .get("res-3")
                .expect("res-3")
                .original,
            MinorUnits::new(250)
        );
    }

    #[test]
    fn settlement_replay_with_two_occupied_slots_never_releases_second_worker_slot_and_untrusted_or_incomplete_usage_stays_held()
     {
        let registry = ornith_luna_registry();
        let task1 = ready_task(4);
        let mut task2 = ready_task(4);
        task2.contract.task_id = TaskId::new("task-gate4-worker2").expect("task_id 2");

        let observed0 = observed_slots(&registry, Some(0), Some(0));
        let ledger0 = funded_ledger(1_000);

        // Worker 1 reserves slot 1 on ornith (in_use: 0 -> 1).
        let proposal1 = match propose_model_slot_reservation(
            &registry,
            &task1,
            &TaskModelAttemptHistory::default(),
            &base_policy(1, true),
            &observed0,
            &ledger0,
            100,
        )
        .expect("worker 1 reserve")
        {
            ModelSlotDecision::Ready { proposal } => *proposal,
            other => panic!("expected Ready for worker 1, got {other:?}"),
        };
        assert_eq!(proposal1.updated_capacity.in_use["ornith"], 1);

        // Worker 2 reserves slot 2 on ornith (in_use: 1 -> 2).
        let observed1 = observed_slots(&registry, Some(1), Some(0));
        let mut policy2 = base_policy(2, true);
        policy2.owner = PrincipalId::new("worker-2").expect("worker-2");
        let proposal2 = match propose_model_slot_reservation(
            &registry,
            &task2,
            &TaskModelAttemptHistory::default(),
            &policy2,
            &observed1,
            &proposal1.next_ledger,
            101,
        )
        .expect("worker 2 reserve")
        {
            ModelSlotDecision::Ready { proposal } => *proposal,
            other => panic!("expected Ready for worker 2, got {other:?}"),
        };
        let ledger_both = proposal2.next_ledger;
        let capacity_both = proposal2.updated_capacity;
        assert_eq!(capacity_both.in_use["ornith"], 2);

        // 1. Cross-attempt / wrong-claim binding is rejected and never releases any slot.
        let (mut report1, verifier1) = build_authenticated_settlement(
            &ledger_both,
            &proposal1.slot_claim,
            "res-1",
            MinorUnits::new(6),
            UsageDisposition::SettledAndTerminated,
            "receipt:worker-1-close",
        );
        let wrong_claim_err = settle_model_slot_attempt(
            &ledger_both,
            &capacity_both,
            &proposal2.slot_claim,
            "res-1",
            "evt-settle-w1",
            &report1,
            &verifier1,
        )
        .expect_err("wrong slot_claim for res-1 must fail");
        assert!(matches!(
            wrong_claim_err,
            ModelSelectionError::Reservation(ReservationError::BindingMismatch)
        ));

        // 2. Untrusted usage adapter keeps reservation outstanding and never releases a slot.
        let untrusted_proposal = settle_model_slot_attempt(
            &ledger_both,
            &capacity_both,
            &proposal1.slot_claim,
            "res-1",
            "evt-settle-w1-untrusted",
            &report1,
            &UntrustedProposalUsageAdapter,
        )
        .expect("untrusted adapter returns held proposal");
        assert!(!untrusted_proposal.trusted_closure_verified);
        assert!(!untrusted_proposal.slot_released);
        assert_eq!(untrusted_proposal.proposed_capacity.in_use["ornith"], 2);
        assert!(untrusted_proposal.proposed_ledger.reservations["res-1"].outstanding);

        // 3. Missing usage amount (None) or incomplete reconciliation flags stay held (in_use remains 2).
        let mut missing_amount_report = report1.clone();
        missing_amount_report.measured_amount = None;
        let held_missing = settle_model_slot_attempt(
            &ledger_both,
            &capacity_both,
            &proposal1.slot_claim,
            "res-1",
            "evt-settle-w1-missing",
            &missing_amount_report,
            &verifier1,
        )
        .expect("missing usage stays held");
        assert!(!held_missing.slot_released);
        assert_eq!(held_missing.proposed_capacity.in_use["ornith"], 2);
        assert!(held_missing.proposed_ledger.reservations["res-1"].outstanding);

        let mut unreconciled_report = report1.clone();
        unreconciled_report.effects_reconciled = Some(false);
        let held_unreconciled = settle_model_slot_attempt(
            &ledger_both,
            &capacity_both,
            &proposal1.slot_claim,
            "res-1",
            "evt-settle-w1-unreconciled",
            &unreconciled_report,
            &verifier1,
        )
        .expect("unreconciled effects stay held while recording interim spend");
        assert!(!held_unreconciled.slot_released);
        assert_eq!(held_unreconciled.proposed_capacity.in_use["ornith"], 2);
        assert!(held_unreconciled.proposed_ledger.reservations["res-1"].outstanding);

        // 4. Zero spend with SettledAndTerminated is NOT auto-converted into ConfirmedNoUsage,
        // and ConfirmedNoUsage with non-zero spend is rejected.
        report1.disposition = UsageDisposition::ConfirmedNoUsage;
        let bad_no_usage_verifier =
            AuthenticatedUsageVerifier::new().with_receipt(AuthenticatedClosureReceipt {
                reservation_id: "res-1".into(),
                attempt_id: proposal1.slot_claim.attempt_id.clone(),
                model_id: "ornith".into(),
                measured_amount: MinorUnits::new(6),
                disposition: UsageDisposition::ConfirmedNoUsage,
                evidence_ref: "receipt:worker-1-close".into(),
            });
        let bad_no_usage = settle_model_slot_attempt(
            &ledger_both,
            &capacity_both,
            &proposal1.slot_claim,
            "res-1",
            "evt-settle-bad-no-usage",
            &report1,
            &bad_no_usage_verifier,
        )
        .expect_err("non-zero ConfirmedNoUsage must be rejected");
        assert!(matches!(
            bad_no_usage,
            ModelSelectionError::Reservation(ReservationError::InvalidSettlement)
        ));
        report1.disposition = UsageDisposition::SettledAndTerminated;

        // 5. First legitimate terminal settlement of Worker 1 releases exactly 1 slot (in_use: 2 -> 1).
        let first_settle = settle_model_slot_attempt(
            &ledger_both,
            &capacity_both,
            &proposal1.slot_claim,
            "res-1",
            "evt-settle-w1",
            &report1,
            &verifier1,
        )
        .expect("first legitimate settlement of worker 1");
        assert!(first_settle.slot_released);
        assert!(!first_settle.idempotent_replay);
        assert!(!first_settle.proposed_ledger.reservations["res-1"].outstanding);
        assert!(first_settle.proposed_ledger.reservations["res-2"].outstanding);
        assert_eq!(first_settle.proposed_capacity.in_use["ornith"], 1);

        // 6. Idempotent replay of the exact same settlement_event_id ("evt-settle-w1")
        // MUST NOT decrement capacity again or release Worker 2's active slot!
        let replayed_settle = settle_model_slot_attempt(
            &first_settle.proposed_ledger,
            &first_settle.proposed_capacity,
            &proposal1.slot_claim,
            "res-1",
            "evt-settle-w1",
            &report1,
            &verifier1,
        )
        .expect("idempotent replay of worker 1 settlement");
        assert!(replayed_settle.idempotent_replay);
        assert!(!replayed_settle.slot_released);
        assert_eq!(
            replayed_settle.proposed_ledger,
            first_settle.proposed_ledger
        );
        assert_eq!(replayed_settle.proposed_capacity.in_use["ornith"], 1);

        // 7. Reusing "evt-settle-w1" with a mutated payload fails with EventCollision.
        let (tampered_report, tampered_verifier) = build_authenticated_settlement(
            &first_settle.proposed_ledger,
            &proposal1.slot_claim,
            "res-1",
            MinorUnits::new(9),
            UsageDisposition::SettledAndTerminated,
            "receipt:worker-1-tampered",
        );
        let collision_err = settle_model_slot_attempt(
            &first_settle.proposed_ledger,
            &first_settle.proposed_capacity,
            &proposal1.slot_claim,
            "res-1",
            "evt-settle-w1",
            &tampered_report,
            &tampered_verifier,
        )
        .expect_err("reusing event_id with different payload must fail");
        assert!(matches!(
            collision_err,
            ModelSelectionError::Reservation(ReservationError::EventCollision)
        ));

        // 8. Attempting a second distinct settlement event on already-settled res-1 fails with InvalidSettlement.
        let duplicate_err = settle_model_slot_attempt(
            &first_settle.proposed_ledger,
            &first_settle.proposed_capacity,
            &proposal1.slot_claim,
            "res-1",
            "evt-settle-w1-second",
            &report1,
            &verifier1,
        )
        .expect_err("settling an already-settled reservation with a new event_id must fail");
        assert!(matches!(
            duplicate_err,
            ModelSelectionError::Reservation(ReservationError::InvalidSettlement)
        ));
    }

    #[test]
    fn claimed_task_binds_next_attempt_id_and_owner_with_or_without_lifecycle_and_strategy_freshness_is_enforced()
     {
        let registry = ornith_luna_registry();
        let observed = observed_slots(&registry, Some(1), Some(0));
        let ledger = funded_ledger(1_000);
        let policy = base_policy(1, true);

        // 1. Claimed task with lifecycle = None must bind both task.attempt_id and task.owner to policy.
        let mut claimed_no_lifecycle = ready_task(3);
        claimed_no_lifecycle.state = SchedulingState::Claimed;
        claimed_no_lifecycle.attempt_id =
            Some(AttemptId::new("other-attempt").expect("attempt_id"));
        claimed_no_lifecycle.owner = Some(policy.owner.clone());
        let wrong_attempt_err = propose_model_slot_reservation(
            &registry,
            &claimed_no_lifecycle,
            &TaskModelAttemptHistory::default(),
            &policy,
            &observed,
            &ledger,
            105,
        )
        .expect_err("claimed task with mismatched attempt_id must fail");
        assert!(
            wrong_attempt_err
                .to_string()
                .contains("claimed task attempt_id does not match policy.next_attempt_id")
        );

        claimed_no_lifecycle.attempt_id = Some(policy.next_attempt_id.clone());
        claimed_no_lifecycle.owner = Some(PrincipalId::new("other-worker").expect("other-worker"));
        let wrong_owner_err = propose_model_slot_reservation(
            &registry,
            &claimed_no_lifecycle,
            &TaskModelAttemptHistory::default(),
            &policy,
            &observed,
            &ledger,
            105,
        )
        .expect_err("claimed task with mismatched owner must fail");
        assert!(
            wrong_owner_err
                .to_string()
                .contains("claimed task owner does not match policy.owner")
        );

        // Matching attempt_id and owner with lifecycle = None succeeds.
        claimed_no_lifecycle.owner = Some(policy.owner.clone());
        let valid_claimed = propose_model_slot_reservation(
            &registry,
            &claimed_no_lifecycle,
            &TaskModelAttemptHistory::default(),
            &policy,
            &observed,
            &ledger,
            105,
        )
        .expect("valid claimed task without lifecycle");
        assert!(matches!(valid_claimed, ModelSlotDecision::Ready { .. }));

        // 2. Claimed task with lifecycle = Some(...) must also bind (next_attempt_id -> owner) in lifecycle.attempts,
        // not merely match attempt count!
        let mut claimed_with_lifecycle = claimed_no_lifecycle.clone();
        claimed_with_lifecycle.lifecycle = Some(TaskLifecycleView {
            revision: 1,
            attempts: BTreeMap::from([(
                AttemptId::new("impostor-attempt").expect("impostor"),
                policy.owner.clone(),
            )]),
            closure_digest: None,
            decision: None,
            reason: None,
        });
        let impostor_lifecycle_err = propose_model_slot_reservation(
            &registry,
            &claimed_with_lifecycle,
            &TaskModelAttemptHistory::default(),
            &policy,
            &observed,
            &ledger,
            105,
        )
        .expect_err("count-matching lifecycle with wrong attempt key must fail");
        assert!(
            impostor_lifecycle_err
                .to_string()
                .contains("claimed task lifecycle attempts must bind next_attempt_id")
        );

        // 3. Direct and strategy callers enforce revision/freshness on StrategyCapacity.
        let mut wrong_rev_obs = observed.clone();
        wrong_rev_obs.registry_revision = "999".into();
        assert!(matches!(
            wrong_rev_obs.to_verified_strategy_capacity(&registry, 105),
            Err(ModelSelectionError::RegistryRevisionMismatch { .. })
        ));
        assert!(matches!(
            observed.to_verified_strategy_capacity(&registry, 150),
            Err(ModelSelectionError::StaleObservation { .. })
        ));
    }

    #[test]
    fn unqualified_failure_setup_error_and_drifted_model_digest_fail_closed() {
        let registry = ornith_luna_registry();
        let task = ready_task(4);
        let observed = observed_slots(&registry, Some(0), Some(0));
        let ledger = funded_ledger(1_000);

        // First reserve and settle attempt 1 with zero usage (explicitly attested ConfirmedNoUsage).
        let proposal1 = match propose_model_slot_reservation(
            &registry,
            &task,
            &TaskModelAttemptHistory::default(),
            &base_policy(1, true),
            &observed,
            &ledger,
            100,
        )
        .expect("attempt 1")
        {
            ModelSlotDecision::Ready { proposal } => *proposal,
            other => panic!("expected Ready, got {other:?}"),
        };
        let (report1, verifier1) = build_authenticated_settlement(
            &proposal1.next_ledger,
            &proposal1.slot_claim,
            "res-1",
            MinorUnits::new(0),
            UsageDisposition::ConfirmedNoUsage,
            "receipt:no-usage",
        );
        let settled1 = settle_model_slot_attempt(
            &proposal1.next_ledger,
            &proposal1.updated_capacity,
            &proposal1.slot_claim,
            "res-1",
            "evt-settle-1",
            &report1,
            &verifier1,
        )
        .expect("confirmed no usage settlement");
        let ledger = settled1.proposed_ledger;

        // 1. Unqualified failure evidence blocks retry/escalation.
        let unqualified_history = TaskModelAttemptHistory {
            attempts: vec![ModelAttemptRecord {
                attempt_id: AttemptId::new("attempt-1").expect("attempt_id"),
                model_id: "ornith".into(),
                tier: 0,
                reservation_id: "res-1".into(),
                contract_revision: 1,
                contract_digest: proposal1.slot_claim.contract_digest.clone(),
                failure: Some(AttemptFailureEvidence {
                    reason: FailureReason::ContextExhausted,
                    qualified: false,
                    diagnostic_artifact: diagnostic_ref('1'),
                    closure_digest: sha256_tag('2'),
                }),
            }],
        };
        let res = propose_model_slot_reservation(
            &registry,
            &task,
            &unqualified_history,
            &base_policy(2, true),
            &observed,
            &ledger,
            101,
        )
        .expect("unqualified failure check");
        assert!(matches!(
            res,
            ModelSlotDecision::Waiting {
                reason: ModelSelectionWait::UnqualifiedFailureEvidence { .. },
                ..
            }
        ));

        // 2. SetupError blocks selection and never escalates to Luna even when allow_escalation=true.
        let setup_error_history = TaskModelAttemptHistory {
            attempts: vec![ModelAttemptRecord {
                attempt_id: AttemptId::new("attempt-1").expect("attempt_id"),
                model_id: "ornith".into(),
                tier: 0,
                reservation_id: "res-1".into(),
                contract_revision: 1,
                contract_digest: proposal1.slot_claim.contract_digest.clone(),
                failure: Some(AttemptFailureEvidence {
                    reason: FailureReason::SetupError,
                    qualified: true,
                    diagnostic_artifact: diagnostic_ref('3'),
                    closure_digest: sha256_tag('4'),
                }),
            }],
        };
        let res = propose_model_slot_reservation(
            &registry,
            &task,
            &setup_error_history,
            &base_policy(2, true),
            &observed,
            &ledger,
            101,
        )
        .expect("setup error check");
        assert!(matches!(
            res,
            ModelSlotDecision::Waiting {
                model_id: Some(ref id),
                reason: ModelSelectionWait::RegistryWait(WaitReason::SetupRequired),
            } if id == "ornith"
        ));

        // 3. Drifted observed model digest marks ornith unavailable; without capacity fallback it waits.
        let mut drifted_obs = observed.clone();
        drifted_obs
            .observed_model_digests
            .insert("ornith".into(), sha256_tag('f'));
        let res = propose_model_slot_reservation(
            &registry,
            &task,
            &TaskModelAttemptHistory::default(),
            &base_policy(2, true),
            &drifted_obs,
            &ledger,
            101,
        )
        .expect("drifted model digest check");
        assert!(matches!(
            res,
            ModelSlotDecision::Waiting {
                model_id: Some(ref id),
                reason: ModelSelectionWait::RegistryWait(WaitReason::CapacityOrUnavailable),
            } if id == "ornith"
        ));
    }

    #[test]
    fn parallel_strategy_candidate_comparison_and_synthesis_fallback_to_luna() {
        let registry = ornith_luna_registry();
        let contract = sample_contract(6);
        let contract_digest = content_digest(&contract).expect("contract digest");
        let observed = observed_slots(&registry, Some(4), Some(1));
        let ledger = funded_ledger(2_000);

        let best_of_config = BestOfConfig {
            schema_version: 1,
            candidate_model_id: "ornith".into(),
            synthesis_model_id: "ornith".into(),
            candidates: 2,
            max_concurrent: 3,
            max_total_attempts: 4,
            required_capabilities: BTreeSet::from(["coding".into(), "tool_calls".into()]),
            synthesis_fallback: Some(SynthesisFallback {
                model_id: "gpt-6-luna".into(),
                on_failures: vec![FailureReason::TestFail, FailureReason::ContextExhausted],
            }),
        };

        let strat_req = StrategyReservationRequest {
            run_id: "strat-run-1".into(),
            contract_digest: contract_digest.clone(),
            baseline_sha: "a".repeat(40),
            oracle_digest: sha256_tag('9'),
            owner: PrincipalId::new("coordinator-1").expect("owner"),
            incarnation: "inc-strat-1".into(),
            attempt_id: AttemptId::new("attempt-strat-1").expect("attempt_id"),
            account_id: AccountId::new("acct-inference").expect("account_id"),
            unit: CurrencyUnit::new("USD", 4).expect("unit"),
            per_unit_budgets: BTreeMap::from([
                ("ornith".into(), MinorUnits::new(10)),
                ("gpt-6-luna".into(), MinorUnits::new(250)),
            ]),
            reservation_id: "res-strat-1".into(),
            reservation_event_id: "evt-strat-1".into(),
            correlation_id: "corr-strat-1".into(),
        };

        let strat_decision = propose_strategy_slot_reservations(
            &best_of_config,
            &contract,
            &strat_req,
            &registry,
            &observed,
            &ledger,
            105,
        )
        .expect("strategy slot reservation");
        let strat_proposal = match strat_decision {
            StrategySlotDecision::Ready { proposal } => *proposal,
            other => panic!("expected Ready strategy proposal, got {other:?}"),
        };
        assert!(strat_proposal.proposal_only);
        assert!(strat_proposal.writer_atomicity_required);
        assert!(!strat_proposal.execution_granted);
        // 2 variants + 1 synthesis on ornith = 3 slots * 10 = 30 minor units reserved.
        assert_eq!(strat_proposal.plan.reserved_slots["ornith"], 3);
        assert_eq!(
            strat_proposal
                .next_ledger
                .reservations
                .get("res-strat-1")
                .expect("res-strat-1")
                .original,
            MinorUnits::new(30)
        );

        // Compare two variant outcomes:
        // Variant 0 has an authenticated custody receipt in `TrustedCustodyVerifierAdapter` and independent verifier.
        // Variant 1 has valid-looking deserializable fields (`claimed_checks_passed: true` + shaped digests + distinct principal strings),
        // but NO authenticated receipt in `TrustedCustodyVerifierAdapter` -> must be downgraded to ReconciledFailure!
        let v0 = &strat_proposal.plan.variants[0];
        let v1 = &strat_proposal.plan.variants[1];
        let cand0 = CandidateRef {
            commit_sha: "b".repeat(40),
            tree_sha: "c".repeat(40),
            artifact: diagnostic_ref('5'),
        };
        let cand1 = CandidateRef {
            commit_sha: "d".repeat(40),
            tree_sha: "e".repeat(40),
            artifact: diagnostic_ref('6'),
        };

        let entries = vec![
            VariantCandidateSubmission {
                unit_id: v0.id.clone(),
                worker_principal: PrincipalId::new("worker-v0").expect("worker-v0"),
                report: VariantReport {
                    unit_id: v0.id.clone(),
                    original: strat_proposal.plan.original.clone(),
                    outcome: VariantOutcome::Candidate {
                        candidate: cand0.clone(),
                        diagnostic: diagnostic_ref('7'),
                    },
                },
                claimed_verification: Some(CandidateDecisionEvidence {
                    contract_digest: contract_digest.clone(),
                    verification_policy_digest: sha256_tag('1'),
                    receipt_digest: sha256_tag('2'),
                    custody_digest: sha256_tag('3'),
                    closure_digest: sha256_tag('4'),
                    source_commit_sha: cand0.commit_sha.clone(),
                    source_tree_sha: cand0.tree_sha.clone(),
                    verifier: PrincipalId::new("independent-verifier").expect("verifier"),
                }),
                claimed_checks_passed: true,
            },
            VariantCandidateSubmission {
                unit_id: v1.id.clone(),
                worker_principal: PrincipalId::new("worker-v1").expect("worker-v1"),
                report: VariantReport {
                    unit_id: v1.id.clone(),
                    original: strat_proposal.plan.original.clone(),
                    outcome: VariantOutcome::Candidate {
                        candidate: cand1.clone(),
                        diagnostic: diagnostic_ref('8'),
                    },
                },
                // Distinct verifier string and shaped digests in raw JSON, but unattested by custody adapter!
                claimed_verification: Some(CandidateDecisionEvidence {
                    contract_digest: contract_digest.clone(),
                    verification_policy_digest: sha256_tag('1'),
                    receipt_digest: sha256_tag('2'),
                    custody_digest: sha256_tag('3'),
                    closure_digest: sha256_tag('4'),
                    source_commit_sha: cand1.commit_sha.clone(),
                    source_tree_sha: cand1.tree_sha.clone(),
                    verifier: PrincipalId::new("other-untrusted-verifier").expect("other-verifier"),
                }),
                claimed_checks_passed: true,
            },
        ];

        // With UntrustedProposalCustodyAdapter, BOTH candidates are downgraded to ReconciledFailure.
        let (_, untrusted_summary) = compare_candidates_and_plan_synthesis(
            &strat_proposal.plan,
            &entries,
            &UntrustedProposalCustodyAdapter,
            &registry,
            &observed,
            106,
        )
        .expect("untrusted comparison");
        assert!(untrusted_summary.custody_attested_units.is_empty());
        assert_eq!(untrusted_summary.downgraded_to_failure_units.len(), 2);
        assert!(untrusted_summary.proposal_only);
        assert!(!untrusted_summary.acceptance_granted);
        assert!(!untrusted_summary.execution_granted);

        // With TrustedCustodyVerifierAdapter holding a receipt ONLY for v0, only v0 enters synthesis.
        let custody_verifier =
            TrustedCustodyVerifierAdapter::new().with_receipt(AuthenticatedCandidateReceipt {
                unit_id: v0.id.clone(),
                contract_digest: contract_digest.clone(),
                commit_sha: cand0.commit_sha.clone(),
                tree_sha: cand0.tree_sha.clone(),
                receipt_digest: sha256_tag('2'),
                custody_digest: sha256_tag('3'),
                closure_digest: sha256_tag('4'),
                verifier: PrincipalId::new("independent-verifier").expect("verifier"),
            });

        let (synth_decision, summary) = compare_candidates_and_plan_synthesis(
            &strat_proposal.plan,
            &entries,
            &custody_verifier,
            &registry,
            &observed,
            106,
        )
        .expect("compare and plan synthesis");
        assert_eq!(summary.custody_attested_units, vec![v0.id.clone()]);
        assert_eq!(summary.downgraded_to_failure_units, vec![v1.id.clone()]);
        assert!(summary.proposal_only);
        assert!(!summary.acceptance_granted);
        assert!(!summary.execution_granted);

        let synth_unit = match synth_decision {
            SynthesisDecision::Ready { unit } => *unit,
            other => panic!("expected Ready synthesis unit, got {other:?}"),
        };
        assert_eq!(synth_unit.candidate_inputs, vec![cand0]);

        // When primary synthesis on ornith fails with TestFail after 1 synthesis attempt,
        // ornith still has 1 tier attempt remaining (task_max_attempts = 2), so fallback waits
        // with RegistryRouteRequired rather than prematurely escalating to Luna.
        let in_tier_failure = QualifiedSynthesisFailure {
            unit_id: synth_unit.id.clone(),
            original: strat_proposal.plan.original.clone(),
            reason: FailureReason::TestFail,
            qualified: true,
            diagnostic: diagnostic_ref('f'),
            synthesis_attempts_by_tier: BTreeMap::from([(0, 1)]),
            total_attempts_used: 3,
        };
        let in_tier_decision = propose_synthesis_fallback_reservation(
            &strat_proposal.plan,
            &synth_unit,
            &in_tier_failure,
            &registry,
            &observed,
            &strat_proposal.next_ledger,
            PrincipalId::new("coordinator-1").expect("owner"),
            "inc-strat-1".into(),
            AttemptId::new("attempt-synth-in-tier").expect("attempt_id"),
            AccountId::new("acct-inference").expect("account_id"),
            CurrencyUnit::new("USD", 4).expect("unit"),
            MinorUnits::new(250),
            "res-synth-in-tier".into(),
            "evt-synth-in-tier".into(),
            "corr-synth-in-tier".into(),
            108,
        )
        .expect("in-tier synthesis check");
        assert!(matches!(
            in_tier_decision,
            SynthesisFallbackSlotDecision::Waiting {
                reason: StrategyWait::RegistryRouteRequired
            }
        ));

        // When primary synthesis on ornith fails with ContextExhausted (or exhausts tier cap),
        // it escalates to gpt-6-luna and reserves the Luna slot + budget!
        let synth_failure = QualifiedSynthesisFailure {
            unit_id: synth_unit.id.clone(),
            original: strat_proposal.plan.original.clone(),
            reason: FailureReason::ContextExhausted,
            qualified: true,
            diagnostic: diagnostic_ref('f'),
            synthesis_attempts_by_tier: BTreeMap::from([(0, 1)]),
            total_attempts_used: 3,
        };

        let fallback_decision = propose_synthesis_fallback_reservation(
            &strat_proposal.plan,
            &synth_unit,
            &synth_failure,
            &registry,
            &observed,
            &strat_proposal.next_ledger,
            PrincipalId::new("coordinator-1").expect("owner"),
            "inc-strat-1".into(),
            AttemptId::new("attempt-synth-fallback-1").expect("attempt_id"),
            AccountId::new("acct-inference").expect("account_id"),
            CurrencyUnit::new("USD", 4).expect("unit"),
            MinorUnits::new(250),
            "res-synth-fallback-1".into(),
            "evt-synth-fallback-1".into(),
            "corr-synth-fallback-1".into(),
            108,
        )
        .expect("synthesis fallback reservation");

        let fallback_proposal = match fallback_decision {
            SynthesisFallbackSlotDecision::Ready { proposal } => *proposal,
            other => panic!("expected Ready synthesis fallback proposal, got {other:?}"),
        };
        assert_eq!(fallback_proposal.unit.model_id, "gpt-6-luna");
        assert_eq!(fallback_proposal.slot_claim.model_id, "gpt-6-luna");
        assert_eq!(fallback_proposal.updated_capacity.in_use["gpt-6-luna"], 2);
        assert!(fallback_proposal.proposal_only);
        assert!(fallback_proposal.writer_atomicity_required);
        assert!(!fallback_proposal.execution_granted);
    }

    #[test]
    fn auth_transport_and_unknown_model_failures_require_repair_and_never_escalate() {
        let registry = ornith_luna_registry();
        let task = ready_task(4);
        let observed = observed_slots(&registry, Some(1), Some(0));
        let ledger = funded_ledger(1_000);

        let proposal1 = match propose_model_slot_reservation(
            &registry,
            &task,
            &TaskModelAttemptHistory::default(),
            &base_policy(1, true),
            &observed,
            &ledger,
            100,
        )
        .expect("attempt 1")
        {
            ModelSlotDecision::Ready { proposal } => *proposal,
            other => panic!("expected Ready, got {other:?}"),
        };
        let (report1, verifier1) = build_authenticated_settlement(
            &proposal1.next_ledger,
            &proposal1.slot_claim,
            "res-1",
            MinorUnits::new(0),
            UsageDisposition::ConfirmedNoUsage,
            "receipt:repair-needed",
        );
        let settled1 = settle_model_slot_attempt(
            &proposal1.next_ledger,
            &proposal1.updated_capacity,
            &proposal1.slot_claim,
            "res-1",
            "evt-settle-1",
            &report1,
            &verifier1,
        )
        .expect("settle attempt 1");
        let ledger = settled1.proposed_ledger;

        for repair_kind in [
            TaskExecutionFailureKind::SetupError,
            TaskExecutionFailureKind::AuthenticationError,
            TaskExecutionFailureKind::UnknownModelError,
            TaskExecutionFailureKind::TransportError,
        ] {
            let history = TaskModelAttemptHistory {
                attempts: vec![ModelAttemptRecord {
                    attempt_id: AttemptId::new("attempt-1").expect("attempt_id"),
                    model_id: "ornith".into(),
                    tier: 0,
                    reservation_id: "res-1".into(),
                    contract_revision: 1,
                    contract_digest: proposal1.slot_claim.contract_digest.clone(),
                    failure: Some(AttemptFailureEvidence::from_classified(
                        repair_kind,
                        true,
                        diagnostic_ref('a'),
                        sha256_tag('b'),
                    )),
                }],
            };
            let decision = propose_model_slot_reservation(
                &registry,
                &task,
                &history,
                &base_policy(2, true),
                &observed,
                &ledger,
                102,
            )
            .expect("repair failure evaluation");
            assert!(matches!(
                decision,
                ModelSlotDecision::Waiting {
                    model_id: Some(ref id),
                    reason: ModelSelectionWait::RegistryWait(WaitReason::SetupRequired),
                } if id == "ornith"
            ));
        }
    }

    #[test]
    fn capacity_fallback_task_max_attempts_budget_cap_and_stale_observations_are_enforced() {
        let registry = ornith_luna_registry();
        let task = ready_task(1);
        // ornith is at full concurrency (16/16), luna has free slots (1/4 used).
        let full_ornith = observed_slots(&registry, Some(16), Some(1));
        let ledger = funded_ledger(1_000);

        // Without allow_capacity_fallback, queue waits on ornith capacity rather than spilling to Luna.
        let wait_decision = propose_model_slot_reservation(
            &registry,
            &task,
            &TaskModelAttemptHistory::default(),
            &base_policy(1, true),
            &full_ornith,
            &ledger,
            105,
        )
        .expect("bounded capacity wait");
        assert!(matches!(
            wait_decision,
            ModelSlotDecision::Waiting {
                model_id: Some(ref id),
                reason: ModelSelectionWait::RegistryWait(WaitReason::CapacityOrUnavailable),
            } if id == "ornith"
        ));

        // With allow_capacity_fallback = true, it routes to gpt-6-luna and reserves a Luna slot.
        let mut fallback_policy = base_policy(1, true);
        fallback_policy.allow_capacity_fallback = true;
        let fallback_decision = propose_model_slot_reservation(
            &registry,
            &task,
            &TaskModelAttemptHistory::default(),
            &fallback_policy,
            &full_ornith,
            &ledger,
            105,
        )
        .expect("capacity fallback to Luna");
        let proposal1 = match fallback_decision {
            ModelSlotDecision::Ready { proposal } => *proposal,
            other => panic!("expected Ready on capacity fallback, got {other:?}"),
        };
        assert_eq!(proposal1.selected_model_id, "gpt-6-luna");

        // Settle attempt 1; since task.contract.max_attempts == 1, attempt 2 hits TaskAttemptLimitReached.
        let (report1, verifier1) = build_authenticated_settlement(
            &proposal1.next_ledger,
            &proposal1.slot_claim,
            "res-1",
            MinorUnits::new(250),
            UsageDisposition::SettledAndTerminated,
            "receipt:attempt-1",
        );
        let settled1 = settle_model_slot_attempt(
            &proposal1.next_ledger,
            &proposal1.updated_capacity,
            &proposal1.slot_claim,
            "res-1",
            "evt-settle-1",
            &report1,
            &verifier1,
        )
        .expect("settle attempt 1");
        let ledger = settled1.proposed_ledger;
        let history = TaskModelAttemptHistory {
            attempts: vec![ModelAttemptRecord {
                attempt_id: AttemptId::new("attempt-1").expect("attempt_id"),
                model_id: "gpt-6-luna".into(),
                tier: 1,
                reservation_id: "res-1".into(),
                contract_revision: 1,
                contract_digest: proposal1.slot_claim.contract_digest.clone(),
                failure: Some(AttemptFailureEvidence::from_classified(
                    TaskExecutionFailureKind::TaskCheckFailed,
                    true,
                    diagnostic_ref('c'),
                    sha256_tag('d'),
                )),
            }],
        };
        let cap_decision = propose_model_slot_reservation(
            &registry,
            &task,
            &history,
            &base_policy(2, true),
            &full_ornith,
            &ledger,
            106,
        )
        .expect("task attempt limit check");
        assert!(matches!(
            cap_decision,
            ModelSlotDecision::Waiting {
                reason: ModelSelectionWait::TaskAttemptLimitReached {
                    used: 1,
                    max_attempts: 1
                },
                ..
            }
        ));

        // Stale observation tick fails closed.
        let stale_err = propose_model_slot_reservation(
            &registry,
            &ready_task(4),
            &TaskModelAttemptHistory::default(),
            &base_policy(2, true),
            &full_ornith,
            &ledger,
            121,
        )
        .expect_err("stale observation rejected");
        assert!(matches!(
            stale_err,
            ModelSelectionError::StaleObservation { .. }
        ));

        // Insufficient budget limit fails closed through ReservationError::LimitExceeded.
        let low_ledger = funded_ledger(5);
        let budget_err = propose_model_slot_reservation(
            &registry,
            &ready_task(4),
            &TaskModelAttemptHistory::default(),
            &base_policy(1, true),
            &observed_slots(&registry, Some(0), Some(0)),
            &low_ledger,
            105,
        )
        .expect_err("budget limit exceeded");
        assert!(matches!(
            budget_err,
            ModelSelectionError::Reservation(ReservationError::LimitExceeded("account"))
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn async_server_handler_reads_private_registry_and_proposes_slot_reservation() {
        use std::os::unix::fs::DirBuilderExt;
        let dir = std::env::temp_dir().join(format!(
            "admiralty-model-selection-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&dir)
            .expect("private test dir");
        struct Cleanup(std::path::PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(dir.clone());

        let file = RegistryFile::new(dir.join("models.json")).expect("registry file");
        let reg = ornith_luna_registry();
        let persisted = file.replace("0", reg.models).expect("persist registry");
        let observed = observed_slots(&persisted, Some(1), Some(0));

        let Json(decision) = select_and_propose_model_slot(
            file,
            ready_task(3),
            TaskModelAttemptHistory::default(),
            base_policy(1, true),
            observed,
            funded_ledger(500),
            105,
        )
        .await
        .expect("async slot proposal succeeds");

        match decision {
            ModelSlotDecision::Ready { proposal } => {
                assert_eq!(proposal.selected_model_id, "ornith");
                assert_eq!(proposal.updated_capacity.in_use["ornith"], 2);
                assert!(proposal.proposal_only);
                assert!(proposal.writer_atomicity_required);
                assert!(!proposal.execution_granted);
            }
            other => panic!("expected Ready from async handler, got {other:?}"),
        }
    }
}
