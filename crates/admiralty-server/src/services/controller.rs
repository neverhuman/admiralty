//! Bounded, fail-closed admission into the durable launch-intent queue.
//!
//! The controller records claims and typed outbox intents, then dispatches due
//! intents through the Store writer's fenced claim API. Runtime launch remains
//! injected so the server must supply its actual resource and execution gates.

use admiralty_core::{
    AttemptId, DomainEvent, LifecycleCommand, PrincipalId, SchedulingState, SourceIdentity, TaskId,
    TaskView, content_digest,
};
use admiralty_store::{LifecycleAuthority, Store, StoreError, outbox::OutboxIntent};
use serde::{Deserialize, Serialize};

const MAX_IDENTITY_BYTES: usize = 128;
const MAX_REFERENCE_BYTES: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchRequest {
    pub source: SourceIdentity,
    pub task_id: TaskId,
    pub expected_contract_digest: String,
    pub attempt_id: AttemptId,
    pub owner: PrincipalId,
    pub run_id: String,
    pub context_digest: String,
    pub policy_digest: String,
    pub provider: String,
    pub model: String,
    pub observed_utc: String,
    pub not_before: u64,
}

/// A trusted in-process adapter checks current scoped admission and the live
/// resource reservation. Implementations must not derive this decision from a
/// worker payload, provider exit status, or deserialized grant.
pub trait CurrentAdmission {
    fn authorize_current(&self, task: &TaskView, request: &LaunchRequest) -> Result<(), String>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DurableIntent {
    pub event_seq: String,
    pub task_id: TaskId,
    pub attempt_id: AttemptId,
    pub outbox_key: String,
    pub duplicate: bool,
}

/// Atomic budget admission is still distinct from model/resource capacity,
/// actual provider execution, trusted closure and settled billing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReservedDurableIntent {
    pub intent: DurableIntent,
    pub reservation: admiralty_store::admission_reservations::ReservationResult,
}

/// Atomic declared claim/budget/managed-slot admission. This is not proof of
/// prepared-provider model mapping, physical capacity or production execution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlottedDurableIntent {
    pub intent: DurableIntent,
    pub reservation: admiralty_store::admission_reservations::ReservationResult,
    pub slot: admiralty_store::inference_slots::SlotResult,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DispatchIntent {
    pub schema_version: u32,
    pub task_id: String,
    pub contract_revision: u32,
    pub contract_digest: String,
    pub attempt_id: String,
    pub owner: String,
    pub run_id: String,
    pub context_digest: String,
    pub policy_digest: String,
    pub provider: String,
    pub model: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuntimeDisposition {
    Candidate { candidate_digest: String },
    Failed { evidence_ref: String },
    Unknown { reason: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeReceipt {
    pub task_id: TaskId,
    pub attempt_id: AttemptId,
    pub run_id: String,
    pub receipt_ref: String,
    pub disposition: RuntimeDisposition,
}

/// The implementation must use the configured P13 resource gate and P08/P14
/// trusted execution boundary. This trait is process-local and is not a grant.
pub trait NativeRuntimeExecutor {
    fn execute(&self, task: &TaskView, intent: &DispatchIntent) -> Result<RuntimeReceipt, String>;
}

/// Trusted service-local time source, never supplied by a launch payload or an
/// agent HTTP request. Unknown/out-of-range samples cannot admit or complete a
/// dispatch. Implementations return Unix seconds, matching Store lease units.
pub trait DispatchClock {
    fn now_unix_seconds(&self) -> Option<u64>;
}

pub struct WallClock;

impl DispatchClock for WallClock {
    fn now_unix_seconds(&self) -> Option<u64> {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .map(|duration| duration.as_secs())
            .filter(|seconds| *seconds <= i64::MAX as u64)
    }
}

fn dispatch_now(clock: &impl DispatchClock) -> Option<u64> {
    clock
        .now_unix_seconds()
        .filter(|seconds| *seconds <= i64::MAX as u64)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DispatchOutcome {
    Idle,
    Completed {
        task_id: TaskId,
        attempt_id: AttemptId,
        receipt_ref: String,
    },
    Failed {
        task_id: TaskId,
        attempt_id: AttemptId,
        receipt_ref: String,
    },
    Unknown {
        reason: String,
    },
}

#[derive(Debug)]
pub enum ControllerError {
    Store(StoreError),
    UnknownTask,
    StaleContract,
    TaskAlreadyOwned,
    AdmissionDenied(String),
    InvalidRequest,
    Digest,
    DispatchInvalid,
    ClockUnavailable,
}

/// Apply a frozen I1 lifecycle command through the Store's configured writer
/// authority. The writer checks current grants on every call, including replay,
/// and binds an optional effect to its exact key, payload, digest, and schedule.
/// Candidate decisions carry their own closure evidence; attempt closure remains
/// a separate command and is never synthesized from process exit or check output.
pub fn execute_lifecycle(
    store: &Store,
    source: SourceIdentity,
    command: LifecycleCommand,
    observed_utc: String,
    effect: Option<OutboxIntent>,
) -> Result<admiralty_store::AppendReceipt, ControllerError> {
    store
        .execute_lifecycle(source, command, observed_utc, effect)
        .map_err(ControllerError::Store)
}

/// Construct the Store with its process-local, trusted lifecycle authority.
/// This is a composition hook for the server owner; there is no permissive
/// default authority in this module.
pub fn open_authoritative_store(
    path: impl AsRef<std::path::Path>,
    authority: std::sync::Arc<dyn LifecycleAuthority>,
) -> Result<Store, ControllerError> {
    Store::open_with_lifecycle_authority(path, authority).map_err(ControllerError::Store)
}

impl std::fmt::Display for ControllerError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Store(error) => write!(formatter, "store operation failed: {error}"),
            Self::UnknownTask => {
                formatter.write_str("task is absent from the current durable projection")
            }
            Self::StaleContract => {
                formatter.write_str("task contract changed since the request was prepared")
            }
            Self::TaskAlreadyOwned => {
                formatter.write_str("task has a live attempt other than this exact replay")
            }
            Self::AdmissionDenied(reason) => {
                write!(formatter, "current scoped admission denied: {reason}")
            }
            Self::InvalidRequest => {
                formatter.write_str("launch request is invalid or exceeds controller bounds")
            }
            Self::Digest => formatter.write_str("content digest could not be produced"),
            Self::DispatchInvalid => {
                formatter.write_str("claimed launch intent failed current binding checks")
            }
            Self::ClockUnavailable => {
                formatter.write_str("trusted dispatch clock is unavailable or outside bounds")
            }
        }
    }
}

impl std::error::Error for ControllerError {}

impl From<StoreError> for ControllerError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

/// Claim and execute at most one due launch. The claim token fences both
/// completion and unknown outcomes. All malformed, stale, denied, or ambiguous
/// claimed intents become durable `unknown` and are never silently retried.
pub fn dispatch_one(
    store: &Store,
    clock: &impl DispatchClock,
    owner: String,
    lease_secs: u64,
    admission: &impl DispatchAdmission,
    runtime: &impl NativeRuntimeExecutor,
) -> Result<DispatchOutcome, ControllerError> {
    let now = dispatch_now(clock).ok_or(ControllerError::ClockUnavailable)?;
    let claim = store.claim_outbox_due(Some(now), owner, lease_secs)?;
    dispatch_claim(store, clock, now, claim, admission, runtime)
}

/// Dispatch only the exact frozen effect key owned by the trusted coordinator.
/// Foreign pending or expired claims are not selected or mutated by this call.
/// All fencing, fresh-clock and runtime receipt checks match `dispatch_one`.
pub fn dispatch_key(
    store: &Store,
    clock: &impl DispatchClock,
    owner: String,
    lease_secs: u64,
    idempotency_key: String,
    admission: &impl DispatchAdmission,
    runtime: &impl NativeRuntimeExecutor,
) -> Result<DispatchOutcome, ControllerError> {
    let now = dispatch_now(clock).ok_or(ControllerError::ClockUnavailable)?;
    let claim = store.claim_outbox_key(Some(now), owner, lease_secs, idempotency_key)?;
    dispatch_claim(store, clock, now, claim, admission, runtime)
}

fn dispatch_claim(
    store: &Store,
    clock: &impl DispatchClock,
    now: u64,
    claim: Option<admiralty_store::outbox::Claim>,
    admission: &impl DispatchAdmission,
    runtime: &impl NativeRuntimeExecutor,
) -> Result<DispatchOutcome, ControllerError> {
    let Some(claim) = claim else {
        return Ok(DispatchOutcome::Idle);
    };
    let claimed_unknown = |reason: String| -> Result<DispatchOutcome, ControllerError> {
        let reason = if valid_identity(&reason, 1024) {
            reason
        } else {
            "dispatch outcome requires reconciliation".into()
        };
        store.fail_outbox_unknown(claim.token.clone(), reason.clone())?;
        Ok(DispatchOutcome::Unknown { reason })
    };
    let deadline = match claim.lease_deadline.parse::<u64>() {
        Ok(deadline) if deadline <= i64::MAX as u64 && deadline > now => deadline,
        _ => return claimed_unknown("invalid durable dispatch deadline".into()),
    };
    let intent: DispatchIntent = match serde_json::from_value(claim.payload.clone()) {
        Ok(intent) => intent,
        Err(_) => return claimed_unknown("malformed launch intent".into()),
    };
    if content_digest(&claim.payload).ok().as_deref() != Some(claim.payload_digest.as_str()) {
        return claimed_unknown("claimed payload digest mismatch".into());
    }
    let task_id = match TaskId::new(intent.task_id.clone()) {
        Ok(task_id) => task_id,
        Err(_) => return claimed_unknown("invalid task identity in launch intent".into()),
    };
    let attempt_id = match AttemptId::new(intent.attempt_id.clone()) {
        Ok(attempt_id) => attempt_id,
        Err(_) => return claimed_unknown("invalid attempt identity in launch intent".into()),
    };
    if intent.schema_version != 1
        || claim.idempotency_key != format!("launch/{task_id}/{attempt_id}")
        || !valid_sha256(&intent.contract_digest)
        || !valid_sha256(&intent.context_digest)
        || !valid_sha256(&intent.policy_digest)
        || !valid_identity(&intent.owner, MAX_IDENTITY_BYTES)
        || !valid_identity(&intent.run_id, MAX_IDENTITY_BYTES)
        || !valid_identity(&intent.provider, MAX_IDENTITY_BYTES)
        || !valid_identity(&intent.model, MAX_IDENTITY_BYTES)
    {
        return claimed_unknown("launch intent identity or schema mismatch".into());
    }
    let snapshot = store.snapshot()?;
    let Some(task) = snapshot.projection.tasks.get(&task_id) else {
        return claimed_unknown("task no longer exists".into());
    };
    let digest = content_digest(&task.contract).map_err(|_| ControllerError::Digest)?;
    if task.state != SchedulingState::Claimed
        || task.attempt_id.as_ref().is_none_or(|id| id != &attempt_id)
        || task
            .owner
            .as_ref()
            .is_none_or(|owner| owner.as_str() != intent.owner)
        || task.contract.revision != intent.contract_revision
        || digest != intent.contract_digest
        || task.contract.task_id != task_id
    {
        return claimed_unknown("current task projection no longer matches launch intent".into());
    }
    if let Err(reason) = admission.authorize_dispatch(task, &intent) {
        return claimed_unknown(format!("current dispatch admission denied: {reason}"));
    }
    // Validation/admission may consume the lease. Never start a runtime using
    // authority that expired while the controller was preparing its dispatch.
    let Some(execution_now) = dispatch_now(clock) else {
        return claimed_unknown("dispatch clock unavailable before execution".into());
    };
    if execution_now < now {
        return claimed_unknown("dispatch clock moved backwards before execution".into());
    }
    if execution_now >= deadline {
        return claimed_unknown("dispatch lease expired before execution".into());
    }
    let runtime_result = runtime.execute(task, &intent);
    // A runtime may block for longer than the claim lease. The pre-execution
    // timestamp is never reused to acknowledge a later effect.
    let Some(completion_now) = dispatch_now(clock) else {
        return claimed_unknown("dispatch clock unavailable after execution".into());
    };
    if completion_now < execution_now {
        return claimed_unknown("dispatch clock moved backwards after execution".into());
    }
    if completion_now >= deadline {
        return claimed_unknown("dispatch lease expired after execution".into());
    }
    let receipt = match runtime_result {
        Ok(receipt) => receipt,
        Err(reason) => {
            return claimed_unknown(format!("runtime outcome requires reconciliation: {reason}"));
        }
    };
    if receipt.task_id != task_id
        || receipt.attempt_id != attempt_id
        || receipt.run_id != intent.run_id
        || !valid_identity(&receipt.receipt_ref, 1024)
    {
        return claimed_unknown("runtime receipt binding mismatch".into());
    }
    match receipt.disposition {
        RuntimeDisposition::Candidate { candidate_digest } => {
            if !valid_sha256(&candidate_digest) {
                return claimed_unknown("runtime candidate digest is invalid".into());
            }
            store.complete_outbox(
                claim.token,
                receipt.receipt_ref.clone(),
                Some(completion_now),
            )?;
            Ok(DispatchOutcome::Completed {
                task_id,
                attempt_id,
                receipt_ref: receipt.receipt_ref,
            })
        }
        RuntimeDisposition::Failed { evidence_ref } => {
            if !valid_identity(&evidence_ref, 1024) {
                return claimed_unknown("runtime failure evidence reference is invalid".into());
            }
            store.complete_outbox(
                claim.token,
                receipt.receipt_ref.clone(),
                Some(completion_now),
            )?;
            Ok(DispatchOutcome::Failed {
                task_id,
                attempt_id,
                receipt_ref: receipt.receipt_ref,
            })
        }
        RuntimeDisposition::Unknown { reason } => {
            claimed_unknown(format!("runtime outcome unknown: {reason}"))
        }
    }
}

pub trait DispatchAdmission {
    fn authorize_dispatch(&self, task: &TaskView, intent: &DispatchIntent) -> Result<(), String>;
}

fn valid_identity(value: &str, max: usize) -> bool {
    !value.is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("sha256:")
        && value[7..].bytes().all(|b| b.is_ascii_hexdigit())
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct LaunchIntentPayload<'a> {
    schema_version: u32,
    task_id: &'a str,
    contract_revision: u32,
    contract_digest: &'a str,
    attempt_id: &'a str,
    owner: &'a str,
    run_id: &'a str,
    context_digest: &'a str,
    policy_digest: &'a str,
    provider: &'a str,
    model: &'a str,
}

/// Perform one bounded admission tick. The Store reducer rechecks that the
/// task is still ready in the same transaction that persists TaskClaimed and
/// its idempotent outbox intent. This stage only queues work; it never spawns.
pub fn admit_one(
    store: &Store,
    admission: &impl CurrentAdmission,
    request: &LaunchRequest,
) -> Result<DurableIntent, ControllerError> {
    validate_request(request)?;
    let snapshot = store.snapshot()?;
    let task = snapshot
        .projection
        .tasks
        .get(&request.task_id)
        .ok_or(ControllerError::UnknownTask)?;
    let digest = content_digest(&task.contract).map_err(|_| ControllerError::Digest)?;
    if digest != request.expected_contract_digest {
        return Err(ControllerError::StaleContract);
    }
    let exact_replay = task.state == SchedulingState::Claimed
        && task.attempt_id.as_ref() == Some(&request.attempt_id)
        && task.owner.as_ref() == Some(&request.owner);
    if task.state != SchedulingState::Ready && !exact_replay {
        return Err(ControllerError::TaskAlreadyOwned);
    }
    admission
        .authorize_current(task, request)
        .map_err(ControllerError::AdmissionDenied)?;

    let intent = planned_launch_intent(task, request)?;
    let key = intent.idempotency_key.clone();
    let receipt = store.append_with_intent(
        request.source.clone(),
        DomainEvent::TaskClaimed {
            task_id: request.task_id.clone(),
            revision: task.contract.revision,
            attempt_id: request.attempt_id.clone(),
            owner: request.owner.clone(),
        },
        request.observed_utc.clone(),
        intent,
    )?;
    Ok(DurableIntent {
        event_seq: receipt.event_seq,
        task_id: request.task_id.clone(),
        attempt_id: request.attempt_id.clone(),
        outbox_key: key,
        duplicate: receipt.duplicate,
    })
}

/// Bounded pure planning identity for a separately issued budget proof. This
/// does not reserve, authenticate, enqueue, or execute anything.
pub fn planned_launch_intent(
    task: &TaskView,
    request: &LaunchRequest,
) -> Result<OutboxIntent, ControllerError> {
    validate_request(request)?;
    let digest = content_digest(&task.contract).map_err(|_| ControllerError::Digest)?;
    if task.contract.task_id != request.task_id || digest != request.expected_contract_digest {
        return Err(ControllerError::StaleContract);
    }
    let payload = serde_json::to_value(LaunchIntentPayload {
        schema_version: 1,
        task_id: request.task_id.as_str(),
        contract_revision: task.contract.revision,
        contract_digest: &digest,
        attempt_id: request.attempt_id.as_str(),
        owner: request.owner.as_str(),
        run_id: &request.run_id,
        context_digest: &request.context_digest,
        policy_digest: &request.policy_digest,
        provider: &request.provider,
        model: &request.model,
    })
    .map_err(|_| ControllerError::InvalidRequest)?;
    Ok(OutboxIntent {
        idempotency_key: format!("launch/{}/{}", request.task_id, request.attempt_id),
        payload,
        not_before: request.not_before,
    })
}

/// Explicit protected admission path. Startup must independently install a
/// current Store reservation authority; the caller's policy/proof remains
/// process-local and no production issuer is installed by this API. Store
/// resolves exact historical retries before reducing a new claim.
pub fn admit_one_reserved(
    store: &Store,
    admission: &impl CurrentAdmission,
    request: &LaunchRequest,
    reservation: admiralty_store::admission_reservations::ReservationCommand,
    proof: admiralty_store::admission_reservations::ReservationProof,
) -> Result<ReservedDurableIntent, ControllerError> {
    validate_request(request)?;
    let snapshot = store.snapshot()?;
    let task = snapshot
        .projection
        .tasks
        .get(&request.task_id)
        .ok_or(ControllerError::UnknownTask)?;
    let intent = planned_launch_intent(task, request)?;
    admission
        .authorize_current(task, request)
        .map_err(ControllerError::AdmissionDenied)?;
    let key = intent.idempotency_key.clone();
    let receipt = store.claim_with_reservation(
        request.source.clone(),
        DomainEvent::TaskClaimed {
            task_id: request.task_id.clone(),
            revision: task.contract.revision,
            attempt_id: request.attempt_id.clone(),
            owner: request.owner.clone(),
        },
        request.observed_utc.clone(),
        intent,
        reservation,
        proof,
    )?;
    Ok(ReservedDurableIntent {
        intent: DurableIntent {
            event_seq: receipt.claim.event_seq,
            task_id: request.task_id.clone(),
            attempt_id: request.attempt_id.clone(),
            outbox_key: key,
            duplicate: receipt.claim.duplicate,
        },
        reservation: receipt.reservation,
    })
}

/// Explicit managed-slot path; both Store authorities/proofs are independently
/// installed and current. Exact historical retries are resolved by Store, not
/// by replacing their claim or falling back to a less protected admission mode.
/// Public model declarations alone never authenticate a prepared provider tuple.
pub fn admit_one_slotted(
    store: &Store,
    admission: &impl CurrentAdmission,
    request: &LaunchRequest,
    inputs: admiralty_store::BudgetAndSlotAdmission,
) -> Result<SlottedDurableIntent, ControllerError> {
    validate_request(request)?;
    let snapshot = store.snapshot()?;
    let task = snapshot
        .projection
        .tasks
        .get(&request.task_id)
        .ok_or(ControllerError::UnknownTask)?;
    let intent = planned_launch_intent(task, request)?;
    admission
        .authorize_current(task, request)
        .map_err(ControllerError::AdmissionDenied)?;
    let key = intent.idempotency_key.clone();
    let receipt = store.claim_with_budget_and_slot(
        request.source.clone(),
        DomainEvent::TaskClaimed {
            task_id: request.task_id.clone(),
            revision: task.contract.revision,
            attempt_id: request.attempt_id.clone(),
            owner: request.owner.clone(),
        },
        request.observed_utc.clone(),
        intent,
        inputs,
    )?;
    Ok(SlottedDurableIntent {
        intent: DurableIntent {
            event_seq: receipt.claim.event_seq,
            task_id: request.task_id.clone(),
            attempt_id: request.attempt_id.clone(),
            outbox_key: key,
            duplicate: receipt.claim.duplicate,
        },
        reservation: receipt.reservation,
        slot: receipt.slot,
    })
}

fn validate_request(request: &LaunchRequest) -> Result<(), ControllerError> {
    let nonempty_bounded = |value: &str, max: usize| {
        !value.is_empty() && value.len() <= max && !value.chars().any(char::is_control)
    };
    if request.source.validate().is_err()
        || !nonempty_bounded(&request.expected_contract_digest, 71)
        || request.expected_contract_digest.len() != 71
        || !request.expected_contract_digest.starts_with("sha256:")
        || !nonempty_bounded(&request.run_id, MAX_IDENTITY_BYTES)
        || !nonempty_bounded(&request.context_digest, 71)
        || request.context_digest.len() != 71
        || !request.context_digest.starts_with("sha256:")
        || !nonempty_bounded(&request.policy_digest, 71)
        || request.policy_digest.len() != 71
        || !request.policy_digest.starts_with("sha256:")
        || !nonempty_bounded(&request.provider, MAX_IDENTITY_BYTES)
        || !nonempty_bounded(&request.model, MAX_IDENTITY_BYTES)
        || !nonempty_bounded(&request.observed_utc, MAX_REFERENCE_BYTES)
    {
        return Err(ControllerError::InvalidRequest);
    }
    Ok(())
}

/// Explicit default-deny adapter for deployments without a qualified P14 and
/// resource-gate composition. Installing it guarantees that no task is queued.
pub struct DenyAdmission;

impl CurrentAdmission for DenyAdmission {
    fn authorize_current(&self, _task: &TaskView, _request: &LaunchRequest) -> Result<(), String> {
        Err("no trusted admission and resource adapter is configured".into())
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use admiralty_core::{
        AcceptanceCriterion, CheckSpec, LifecycleAction, LifecycleCommand, ProjectId, RootId,
        TaskContract,
    };
    use admiralty_store::{LifecycleAuthority, LifecycleEffect};
    use std::cell::Cell;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Arc;
    use std::{fs, path::PathBuf};

    struct FixedClock(u64);
    impl DispatchClock for FixedClock {
        fn now_unix_seconds(&self) -> Option<u64> {
            Some(self.0)
        }
    }

    struct TestClock(Cell<Option<u64>>);
    impl DispatchClock for TestClock {
        fn now_unix_seconds(&self) -> Option<u64> {
            self.0.get()
        }
    }

    struct ClockAdvancingRuntime<'a> {
        clock: &'a TestClock,
        after: Option<u64>,
        calls: &'a Cell<usize>,
        disposition: RuntimeDisposition,
    }
    impl NativeRuntimeExecutor for ClockAdvancingRuntime<'_> {
        fn execute(
            &self,
            task: &TaskView,
            intent: &DispatchIntent,
        ) -> Result<RuntimeReceipt, String> {
            self.calls.set(self.calls.get() + 1);
            self.clock.0.set(self.after);
            FakeRuntime {
                disposition: self.disposition.clone(),
            }
            .execute(task, intent)
        }
    }

    struct ClockAdvancingAdmission<'a> {
        clock: &'a TestClock,
        after: Option<u64>,
    }
    impl DispatchAdmission for ClockAdvancingAdmission<'_> {
        fn authorize_dispatch(
            &self,
            task: &TaskView,
            intent: &DispatchIntent,
        ) -> Result<(), String> {
            DispatchPermit.authorize_dispatch(task, intent)?;
            self.clock.0.set(self.after);
            Ok(())
        }
    }

    struct PermitAdmission;
    impl CurrentAdmission for PermitAdmission {
        fn authorize_current(
            &self,
            task: &TaskView,
            request: &LaunchRequest,
        ) -> Result<(), String> {
            if task.contract.project_id.as_str() == "controller-project"
                && task.contract.root_id.as_str() == "controller-root"
                && task.contract.task_id == request.task_id
                && request.run_id == "round22-fake-child-run"
            {
                Ok(())
            } else {
                Err("scope mismatch".into())
            }
        }
    }

    struct DispatchPermit;
    impl DispatchAdmission for DispatchPermit {
        fn authorize_dispatch(
            &self,
            task: &TaskView,
            intent: &DispatchIntent,
        ) -> Result<(), String> {
            if task.contract.task_id.as_str() == intent.task_id
                && task.contract.project_id.as_str() == "controller-project"
                && intent.provider == "fake"
            {
                Ok(())
            } else {
                Err("scope mismatch".into())
            }
        }
    }

    struct DenyDispatch;
    impl DispatchAdmission for DenyDispatch {
        fn authorize_dispatch(
            &self,
            _task: &TaskView,
            _intent: &DispatchIntent,
        ) -> Result<(), String> {
            Err("no live permit".into())
        }
    }

    struct FakeRuntime {
        disposition: RuntimeDisposition,
    }
    impl NativeRuntimeExecutor for FakeRuntime {
        fn execute(
            &self,
            _task: &TaskView,
            intent: &DispatchIntent,
        ) -> Result<RuntimeReceipt, String> {
            Ok(RuntimeReceipt {
                task_id: TaskId::new(intent.task_id.clone()).map_err(|error| error.to_string())?,
                attempt_id: AttemptId::new(intent.attempt_id.clone())
                    .map_err(|error| error.to_string())?,
                run_id: intent.run_id.clone(),
                receipt_ref: "receipt:fake-child-1".into(),
                disposition: self.disposition.clone(),
            })
        }
    }

    struct ExactCloseAuthority;
    impl LifecycleAuthority for ExactCloseAuthority {
        fn authorize(
            &self,
            source: &SourceIdentity,
            command: &LifecycleCommand,
            task: &TaskView,
            effect: Option<&LifecycleEffect<'_>>,
        ) -> bool {
            source.producer_id == "controller-test"
                && task.contract.task_id == command.task_id
                && matches!(
                    &command.action,
                    LifecycleAction::CloseAttempt {
                        attempt_id,
                        closure_digest,
                        reason,
                        cancelled: false,
                    } if attempt_id.as_str() == "controller-attempt"
                        && closure_digest == &format!("sha256:{}", "d".repeat(64))
                        && reason == "supervisor confirmed closed"
                )
                && effect.is_some_and(|effect| {
                    effect.idempotency_key == "controller/close-followup"
                        && effect.payload == r#"{"audit":"closed"}"#
                        && effect.payload_digest.starts_with("sha256:")
                        && effect.intent_digest.starts_with("sha256:")
                        && effect.not_before == 25
                })
        }
    }

    fn contract() -> TaskContract {
        TaskContract {
            schema_version: 1,
            project_id: ProjectId::new("controller-project").expect("project ID"),
            root_id: RootId::new("controller-root").expect("root ID"),
            task_id: TaskId::new("controller-task").expect("task ID"),
            revision: 1,
            title: "Durable controller fixture".into(),
            objective: "Persist one bounded launch intent".into(),
            non_goals: vec![],
            owned_paths: vec!["src/lib.rs".into()],
            forbidden_paths: vec![],
            read_first: vec![],
            dependencies: vec![],
            criteria: vec![AcceptanceCriterion {
                id: "C1".into(),
                text: "Intent is durable".into(),
                check_ids: vec!["unit".into()],
            }],
            checks: vec![CheckSpec {
                id: "unit".into(),
                program: "cargo".into(),
                args: vec!["test".into()],
                cwd: ".".into(),
                timeout_secs: 10,
            }],
            authority_ref: "grant:test-candidate".into(),
            max_attempts: 1,
            simulated: true,
        }
    }

    fn request() -> LaunchRequest {
        LaunchRequest {
            source: SourceIdentity {
                producer_id: "controller-test".into(),
                incarnation: "fixed-incarnation".into(),
                sequence: "2".into(),
            },
            task_id: TaskId::new("controller-task").expect("task ID"),
            expected_contract_digest: content_digest(&contract()).expect("contract digest"),
            attempt_id: AttemptId::new("controller-attempt").expect("attempt ID"),
            owner: PrincipalId::new("controller-owner").expect("owner ID"),
            run_id: "round22-fake-child-run".into(),
            context_digest: format!("sha256:{}", "a".repeat(64)),
            policy_digest: format!("sha256:{}", "b".repeat(64)),
            provider: "fake".into(),
            model: "deterministic".into(),
            observed_utc: "2026-10-18T20:00:00Z".into(),
            not_before: 10,
        }
    }

    fn database() -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("admiralty-controller-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).expect("create private fixture directory");
        #[cfg(unix)]
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700))
            .expect("secure fixture directory");
        root.join("controller.sqlite")
    }

    fn store_with_task(path: &PathBuf) -> Store {
        let store = Store::open(path).expect("open temporary controller store");
        store
            .append(
                SourceIdentity {
                    producer_id: "controller-test".into(),
                    incarnation: "fixed-incarnation".into(),
                    sequence: "1".into(),
                },
                DomainEvent::TaskSubmitted {
                    contract: contract(),
                },
                "2026-10-18T20:00:00Z".into(),
            )
            .expect("submit controller fixture");
        store
    }

    fn remove_database(path: &PathBuf) {
        let _ = fs::remove_file(path);
        let _ = fs::remove_file(path.with_extension("sqlite.writer.lock"));
        let _ = fs::remove_file(path.with_extension("sqlite-wal"));
        let _ = fs::remove_file(path.with_extension("sqlite-shm"));
        if let Some(parent) = path.parent() {
            let _ = fs::remove_dir_all(parent);
        }
    }

    #[test]
    fn actual_store_records_one_pending_intent_and_exact_replay_is_idempotent() {
        let path = database();
        let store = store_with_task(&path);
        let request = request();
        let first = admit_one(&store, &PermitAdmission, &request).expect("durable admission");
        assert!(!first.duplicate);
        let replay = admit_one(&store, &PermitAdmission, &request).expect("exact replay");
        assert!(replay.duplicate);
        assert_eq!(first.event_seq, replay.event_seq);
        assert_eq!(first.outbox_key, replay.outbox_key);
        assert_eq!(
            &store
                .snapshot()
                .expect("current durable state")
                .projection
                .tasks
                .get(&request.task_id)
                .expect("fixture task")
                .state,
            &SchedulingState::Claimed
        );
        drop(store);
        let reopened = Store::open(&path).expect("reopen durable controller store");
        assert_eq!(
            reopened
                .snapshot()
                .expect("replayed durable state")
                .projection
                .tasks
                .get(&request.task_id)
                .expect("reopened task")
                .attempt_id
                .as_ref(),
            Some(&request.attempt_id)
        );
        drop(reopened);
        remove_database(&path);
    }

    #[test]
    fn protected_controller_commits_one_bound_budget_and_rechecks_retry_authority() {
        use admiralty_runtime::reservations::{
            AccountId, CurrencyUnit, MinorUnits, ReservationEvent, ReservationLedger,
            ReservationScope, ScopeLimits,
        };
        use admiralty_store::admission_reservations::{
            ReservationAuthority, ReservationBinding, ReservationCommand, ledger_digest,
        };
        use std::{
            any::Any,
            sync::atomic::{AtomicBool, Ordering},
        };
        struct Policy(AtomicBool);
        impl ReservationAuthority for Policy {
            fn authorize_write(
                &self,
                _: &ReservationBinding<'_>,
                proof: &(dyn Any + Send + Sync),
            ) -> bool {
                proof.is::<()>() && !self.0.load(Ordering::SeqCst)
            }
            fn authorize_read(
                &self,
                _: &ReservationLedger,
                proof: &(dyn Any + Send + Sync),
            ) -> bool {
                proof.is::<()>() && !self.0.load(Ordering::SeqCst)
            }
        }
        let path = database();
        let policy = Arc::new(Policy(AtomicBool::new(false)));
        let store = Store::open_with_startup_authorities(
            &path,
            admiralty_store::StoreAuthorities {
                reservations: Some(policy.clone()),
                ..Default::default()
            },
        )
        .unwrap();
        store
            .append(
                SourceIdentity {
                    producer_id: "controller-test".into(),
                    incarnation: "fixed-incarnation".into(),
                    sequence: "1".into(),
                },
                DomainEvent::TaskSubmitted {
                    contract: contract(),
                },
                "original".into(),
            )
            .unwrap();
        let unit = CurrencyUnit::new("USD", 2).unwrap();
        let account = AccountId::new("fixture").unwrap();
        let ledger = store.reservation_ledger(Arc::new(())).unwrap();
        store
            .record_reservation(
                ReservationCommand {
                    schema_version: 1,
                    command_id: "limits".into(),
                    source: SourceIdentity {
                        producer_id: "budget".into(),
                        incarnation: "fixed-incarnation".into(),
                        sequence: "1".into(),
                    },
                    expected_ledger_revision: 0,
                    expected_ledger_digest: ledger_digest(&ledger).unwrap(),
                    launch_intent_digest: None,
                    event: ReservationEvent::SetLimits {
                        event_id: "limits".into(),
                        account_id: account.clone(),
                        limits: ScopeLimits {
                            unit: unit.clone(),
                            account: Some(MinorUnits::new(100)),
                            project: Some(MinorUnits::new(100)),
                            root: Some(MinorUnits::new(100)),
                        },
                    },
                },
                Arc::new(()),
            )
            .unwrap();
        let request = request();
        let task = store.snapshot().unwrap().projection.tasks[&request.task_id].clone();
        let intent = planned_launch_intent(&task, &request).unwrap();
        let ledger = store.reservation_ledger(Arc::new(())).unwrap();
        let reservation = ReservationCommand {
            schema_version: 1,
            command_id: "reserve-controller".into(),
            source: request.source.clone(),
            expected_ledger_revision: ledger.revision,
            expected_ledger_digest: ledger_digest(&ledger).unwrap(),
            launch_intent_digest: Some(admiralty_store::outbox::intent_digest(&intent).unwrap()),
            event: ReservationEvent::Reserve {
                event_id: "reserve-controller".into(),
                reservation_id: "reserved-attempt".into(),
                scope: ReservationScope {
                    project_id: task.contract.project_id.clone(),
                    root_id: task.contract.root_id.clone(),
                    task_id: request.task_id.clone(),
                    attempt_id: request.attempt_id.clone(),
                    owner: request.owner.clone(),
                    incarnation: request.source.incarnation.clone(),
                    contract_revision: task.contract.revision,
                    contract_digest: request.expected_contract_digest.clone(),
                    authority_ref: task.contract.authority_ref.clone(),
                    resource_claims_digest: request.policy_digest.clone(),
                    account_id: account,
                    unit,
                },
                correlation_id: request.run_id.clone(),
                amount: MinorUnits::new(60),
            },
        };
        let first = admit_one_reserved(
            &store,
            &PermitAdmission,
            &request,
            reservation.clone(),
            Arc::new(()),
        )
        .unwrap();
        assert!(!first.intent.duplicate);
        let retry = admit_one_reserved(
            &store,
            &PermitAdmission,
            &request,
            reservation.clone(),
            Arc::new(()),
        )
        .unwrap();
        assert!(retry.intent.duplicate);
        assert_eq!(retry.reservation.receipt, first.reservation.receipt);
        let mut wrong = request.clone();
        wrong.model = "different-model".into();
        assert!(
            admit_one_reserved(
                &store,
                &PermitAdmission,
                &wrong,
                reservation.clone(),
                Arc::new(())
            )
            .is_err()
        );
        assert_eq!(
            store
                .reservation_ledger(Arc::new(()))
                .unwrap()
                .reservations
                .len(),
            1
        );
        policy.0.store(true, Ordering::SeqCst);
        assert!(
            admit_one_reserved(
                &store,
                &PermitAdmission,
                &request,
                reservation,
                Arc::new(())
            )
            .is_err()
        );
        drop(store);
        remove_database(&path);
    }

    #[test]
    fn default_deny_and_stale_contract_never_record_a_claim_or_intent() {
        let path = database();
        let store = store_with_task(&path);
        let mut request = request();
        assert!(matches!(
            admit_one(&store, &DenyAdmission, &request),
            Err(ControllerError::AdmissionDenied(_))
        ));
        request.expected_contract_digest = format!("sha256:{}", "c".repeat(64));
        assert!(matches!(
            admit_one(&store, &PermitAdmission, &request),
            Err(ControllerError::StaleContract)
        ));
        let state = store.snapshot().expect("unchanged durable state");
        assert!(matches!(
            state
                .projection
                .tasks
                .get(&request.task_id)
                .expect("task")
                .state,
            SchedulingState::Ready
        ));
        drop(store);
        let reopened = Store::open(&path).expect("reopen denied store");
        let reopened_state = reopened.snapshot().expect("denial left state unchanged");
        assert!(matches!(
            reopened_state
                .projection
                .tasks
                .get(&request.task_id)
                .expect("reopened task")
                .state,
            SchedulingState::Ready
        ));
        drop(reopened);
        remove_database(&path);
    }

    #[test]
    fn writer_claim_dispatches_one_runtime_candidate_without_accepting_task() {
        let path = database();
        let store = store_with_task(&path);
        let request = request();
        admit_one(&store, &PermitAdmission, &request).expect("durable admission");
        let result = dispatch_one(
            &store,
            &FixedClock(10),
            "controller-worker".into(),
            30,
            &DispatchPermit,
            &FakeRuntime {
                disposition: RuntimeDisposition::Candidate {
                    candidate_digest: format!("sha256:{}", "e".repeat(64)),
                },
            },
        )
        .expect("dispatch");
        assert_eq!(
            result,
            DispatchOutcome::Completed {
                task_id: request.task_id.clone(),
                attempt_id: request.attempt_id.clone(),
                receipt_ref: "receipt:fake-child-1".into(),
            }
        );
        let task = store
            .snapshot()
            .expect("projection")
            .projection
            .tasks
            .get(&request.task_id)
            .expect("task")
            .clone();
        assert_eq!(
            task.state,
            SchedulingState::Claimed,
            "runtime candidate is not task acceptance"
        );
        assert_eq!(
            dispatch_one(
                &store,
                &FixedClock(11),
                "next-worker".into(),
                30,
                &DispatchPermit,
                &FakeRuntime {
                    disposition: RuntimeDisposition::Candidate {
                        candidate_digest: format!("sha256:{}", "e".repeat(64))
                    }
                }
            )
            .expect("completed intent is not redelivered"),
            DispatchOutcome::Idle,
        );
        drop(store);
        remove_database(&path);
    }

    #[test]
    fn denied_or_unknown_dispatch_is_durably_held_without_retry() {
        let path = database();
        let store = store_with_task(&path);
        let request = request();
        admit_one(&store, &PermitAdmission, &request).expect("durable admission");
        let result = dispatch_one(
            &store,
            &FixedClock(10),
            "controller-worker".into(),
            30,
            &DenyDispatch,
            &FakeRuntime {
                disposition: RuntimeDisposition::Candidate {
                    candidate_digest: format!("sha256:{}", "e".repeat(64)),
                },
            },
        )
        .expect("denial becomes explicit unknown");
        assert!(matches!(result, DispatchOutcome::Unknown { .. }));
        assert_eq!(
            dispatch_one(
                &store,
                &FixedClock(11),
                "next-worker".into(),
                30,
                &DispatchPermit,
                &FakeRuntime {
                    disposition: RuntimeDisposition::Unknown {
                        reason: "should not run".into()
                    }
                }
            )
            .expect("unknown claim is held"),
            DispatchOutcome::Idle
        );
        assert_eq!(
            store
                .snapshot()
                .expect("domain projection")
                .projection
                .tasks
                .get(&request.task_id)
                .expect("task")
                .state,
            SchedulingState::Claimed
        );
        drop(store);
        remove_database(&path);
    }

    fn candidate_disposition() -> RuntimeDisposition {
        RuntimeDisposition::Candidate {
            candidate_digest: format!("sha256:{}", "e".repeat(64)),
        }
    }

    #[test]
    fn unavailable_clock_before_claim_leaves_intent_pending_and_never_launches() {
        let path = database();
        let store = store_with_task(&path);
        admit_one(&store, &PermitAdmission, &request()).expect("durable admission");
        let clock = TestClock(Cell::new(None));
        let calls = Cell::new(0);
        let runtime = ClockAdvancingRuntime {
            clock: &clock,
            after: Some(11),
            calls: &calls,
            disposition: candidate_disposition(),
        };
        for invalid in [None, Some(u64::MAX)] {
            clock.0.set(invalid);
            assert!(matches!(
                dispatch_one(
                    &store,
                    &clock,
                    "controller-worker".into(),
                    30,
                    &DispatchPermit,
                    &runtime,
                ),
                Err(ControllerError::ClockUnavailable)
            ));
            assert_eq!(calls.get(), 0);
        }
        clock.0.set(Some(10));
        assert!(matches!(
            dispatch_one(
                &store,
                &clock,
                "controller-worker".into(),
                30,
                &DispatchPermit,
                &runtime,
            )
            .expect("the unclaimed intent is still pending"),
            DispatchOutcome::Completed { .. }
        ));
        assert_eq!(calls.get(), 1);
        assert!(
            matches!(
                store.claim_outbox_due(Some(10), "retroactive-worker".into(), 30),
                Err(StoreError::Outbox(
                    admiralty_store::outbox::OutboxError::ClockRollback
                ))
            ),
            "completion records the fresh post-runtime clock, not the claim timestamp"
        );
        drop(store);
        remove_database(&path);
    }

    #[test]
    fn expired_or_uncertain_clock_after_admission_never_starts_runtime() {
        for after in [Some(40), Some(41), Some(9), None, Some(u64::MAX)] {
            let path = database();
            let store = store_with_task(&path);
            admit_one(&store, &PermitAdmission, &request()).expect("durable admission");
            let clock = TestClock(Cell::new(Some(10)));
            let calls = Cell::new(0);
            let runtime = ClockAdvancingRuntime {
                clock: &clock,
                after: Some(11),
                calls: &calls,
                disposition: candidate_disposition(),
            };
            let admission = ClockAdvancingAdmission {
                clock: &clock,
                after,
            };
            assert!(matches!(
                dispatch_one(
                    &store,
                    &clock,
                    "controller-worker".into(),
                    30,
                    &admission,
                    &runtime,
                )
                .expect("uncertain claim is held"),
                DispatchOutcome::Unknown { .. }
            ));
            assert_eq!(calls.get(), 0, "expired admission cannot start a child");
            clock.0.set(Some(100));
            assert_eq!(
                dispatch_one(
                    &store,
                    &clock,
                    "next-worker".into(),
                    30,
                    &DispatchPermit,
                    &runtime,
                )
                .expect("held claim is not automatically retried"),
                DispatchOutcome::Idle
            );
            assert_eq!(calls.get(), 0);
            drop(store);
            remove_database(&path);
        }
    }

    #[test]
    fn runtime_cannot_complete_with_expired_backwards_or_unknown_fresh_clock() {
        for disposition in [
            candidate_disposition(),
            RuntimeDisposition::Failed {
                evidence_ref: "evidence:failed-child".into(),
            },
        ] {
            for after in [Some(40), Some(41), Some(9), None, Some(u64::MAX)] {
                let path = database();
                let store = store_with_task(&path);
                let request = request();
                admit_one(&store, &PermitAdmission, &request).expect("durable admission");
                let clock = TestClock(Cell::new(Some(10)));
                let calls = Cell::new(0);
                let runtime = ClockAdvancingRuntime {
                    clock: &clock,
                    after,
                    calls: &calls,
                    disposition: disposition.clone(),
                };
                assert!(matches!(
                    dispatch_one(
                        &store,
                        &clock,
                        "controller-worker".into(),
                        30,
                        &DispatchPermit,
                        &runtime,
                    )
                    .expect("ambiguous completion becomes durable unknown"),
                    DispatchOutcome::Unknown { .. }
                ));
                assert_eq!(calls.get(), 1);
                let task = store.snapshot().expect("retained task projection");
                assert_eq!(
                    task.projection.tasks[&request.task_id].state,
                    SchedulingState::Claimed
                );
                drop(store);
                let reopened = Store::open(&path).expect("reopen held claim");
                clock.0.set(Some(100));
                assert_eq!(
                    dispatch_one(
                        &reopened,
                        &clock,
                        "next-worker".into(),
                        30,
                        &DispatchPermit,
                        &runtime,
                    )
                    .expect("unknown survives restart without redelivery"),
                    DispatchOutcome::Idle
                );
                assert_eq!(calls.get(), 1, "uncertainty cannot launch a second child");
                drop(reopened);
                remove_database(&path);
            }
        }
    }

    #[test]
    fn frozen_lifecycle_writer_binds_exact_effect_and_separate_closure() {
        let path = database();
        let store = open_authoritative_store(&path, Arc::new(ExactCloseAuthority))
            .expect("open with trusted lifecycle authority");
        store
            .append(
                SourceIdentity {
                    producer_id: "controller-test".into(),
                    incarnation: "fixed-incarnation".into(),
                    sequence: "1".into(),
                },
                DomainEvent::TaskSubmitted {
                    contract: contract(),
                },
                "2026-10-18T20:00:00Z".into(),
            )
            .expect("submit lifecycle fixture");
        let request = request();
        admit_one(&store, &PermitAdmission, &request).expect("claim lifecycle fixture");
        let task = store
            .snapshot()
            .expect("current task")
            .projection
            .tasks
            .get(&request.task_id)
            .expect("task exists")
            .clone();
        let command = LifecycleCommand {
            protocol_version: 2,
            task_id: request.task_id.clone(),
            expected_task_digest: content_digest(&task).expect("I1 task view digest"),
            actor: request.owner.clone(),
            action: LifecycleAction::CloseAttempt {
                attempt_id: request.attempt_id.clone(),
                closure_digest: format!("sha256:{}", "d".repeat(64)),
                reason: "supervisor confirmed closed".into(),
                cancelled: false,
            },
        };
        let receipt = execute_lifecycle(
            &store,
            SourceIdentity {
                producer_id: "controller-test".into(),
                incarnation: "fixed-incarnation".into(),
                sequence: "3".into(),
            },
            command,
            "2026-10-18T20:00:01Z".into(),
            Some(OutboxIntent {
                idempotency_key: "controller/close-followup".into(),
                payload: serde_json::json!({"audit":"closed"}),
                not_before: 25,
            }),
        )
        .expect("I1 writer accepts exact authorized effect");
        assert!(!receipt.duplicate);
        assert_eq!(
            store
                .snapshot()
                .expect("closed lifecycle state")
                .projection
                .tasks
                .get(&request.task_id)
                .expect("closed task")
                .state,
            SchedulingState::Failed
        );
        drop(store);
        remove_database(&path);
    }
}
