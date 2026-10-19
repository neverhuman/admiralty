//! Bounded trusted-local task admission/dispatch composition. No startup route,
//! issuer, provider selection, automatic retry, source candidate or acceptance.
//! Shared Store dispatch is exact-key scoped. The concrete executor and its
//! StoreStartedSink are constructed together and never exposed for replacement.
//!
//! Later composition must retain the original completion capability while it
//! captures an immutable source commit/tree, runs independently authenticated
//! checks, records CandidateSubmitted, and obtains the exact Store lifecycle
//! authority/CAS. Transcript digests are never source candidates. Actual Linux
//! admission/start/closure and provider qualification remain separate gates.
//! A closed proof may be buffered while its outbox acknowledgement is unknown;
//! buffering does not reconcile that effect or authorize a task retry/acceptance.
//! The later lifecycle authority must independently prove effect reconciliation.

use super::{
    controller::{
        self, CurrentAdmission, DispatchAdmission, DispatchClock, DispatchIntent, DispatchOutcome,
        DurableIntent, LaunchRequest, NativeRuntimeExecutor, RuntimeReceipt,
    },
    native_codex_plan::NativeCodexSlotAdmission,
    native_executor::{
        ClosedExecution, NativeExecutionPlan, NativeExecutor, PendingDiagnostics,
        native_resource_claims_digest,
    },
    native_loop::{StartedScope, StoreStartedSink},
};
use admiralty_core::{PrincipalId, SchedulingState, TaskView, content_digest};
use admiralty_runtime::{
    reservations::{Reservation, ReservationEvent},
    resource_gate::{ResourceGate, VerifiedResourcePermit},
    session::validate_launch_envelope,
};
use admiralty_store::{
    Store,
    admission_reservations::{ReservationCommand, ReservationProof, validate_command},
    inference_slots::{SlotEvent, SlotHold},
    outbox::MAX_LEASE_SECONDS,
};
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    path::PathBuf,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub const MAX_RETAINED_RUNS: usize = 1024;

/// Trusted startup configuration, deliberately not Deserialize or a network grant.
pub struct NativeTaskLoopConfig {
    pub scope: StartedScope,
    pub task_owner: PrincipalId,
    pub dispatch_owner: String,
    pub dispatch_lease_secs: u64,
    /// Lifetime bound, including handed-off identities: no silent history eviction.
    pub max_retained_runs: usize,
    pub max_buffered_completions: usize,
}
pub struct NativeTaskRuntimeConfig {
    pub journal_root: PathBuf,
    pub deadline: Duration,
    pub capture_limit: usize,
}

/// Explicit process-local budget admission. A command or public digest does not
/// construct its opaque proof or install a current Store authority.
pub struct NativeBudgetAdmission {
    pub command: ReservationCommand,
    /// Write authorization is not ledger-read authorization. Retain both roles
    /// explicitly; a denied read never falls back to the admission proof.
    pub proof: ReservationProof,
    pub read_proof: ReservationProof,
}

struct RetainedBudget {
    admission: NativeBudgetAdmission,
    /// Stable fields come from the actual plan. The budget revision is the
    /// frozen ledger CAS successor, independently confirmed by Store's receipt;
    /// it is not the resource plan's reservation version.
    expected: Reservation,
}

struct RetainedSlot {
    admission: NativeCodexSlotAdmission,
    expected: SlotHold,
    pool_key: String,
    revision: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoopError {
    InvalidConfiguration,
    InvalidBinding,
    Capacity,
    UnknownRun,
    StoreUnavailable,
    RegistrationRejected,
}
impl std::fmt::Display for LoopError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidConfiguration => "native task loop configuration is outside bounds",
            Self::InvalidBinding => "native task loop binding differs",
            Self::Capacity => "native task loop retained capacity is exhausted",
            Self::UnknownRun => "run has no retained process-local admission",
            Self::StoreUnavailable => "native task loop durable state is unavailable",
            Self::RegistrationRejected => "native plan registration was rejected before admission",
        })
    }
}
impl std::error::Error for LoopError {}

/// Pre-admission rejection always returns the original actual preparation.
pub struct RejectedAdmission {
    pub reason: LoopError,
    pub plan: Box<NativeExecutionPlan>,
    /// Reserved-mode rejection returns the original command and both opaque
    /// proof roles. Ordinary admission returns None; nothing is reissued.
    pub budget: Option<NativeBudgetAdmission>,
}
impl std::fmt::Debug for RejectedAdmission {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RejectedAdmission")
            .field("reason", &self.reason)
            .finish_non_exhaustive()
    }
}

/// A refused slotted admission returns both original ownership capabilities.
/// This does not release an existing hold or authorize a different attempt.
pub struct RejectedSlotAdmission {
    pub reason: LoopError,
    pub plan: Box<NativeExecutionPlan>,
    pub slot: Box<NativeCodexSlotAdmission>,
}
impl std::fmt::Debug for RejectedSlotAdmission {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RejectedSlotAdmission")
            .field("reason", &self.reason)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HoldReason {
    AdmissionUncertain,
    DispatchUncertain,
    RuntimeUncertain,
    CompletionUnavailable,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AdmissionStatus {
    Queued(DurableIntent),
    Held(HoldReason),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TaskDispatchStatus {
    Idle,
    /// An outbox effect receipt was recorded; the task is NOT accepted.
    EffectRecorded(RuntimeReceipt),
    Held {
        reason: HoldReason,
        /// The executor was called; this is not proof that a child started.
        runtime_attempted: bool,
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffectAckState {
    Unconfirmed,
    Recorded,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompletionStatus {
    WaitingForRuntime,
    Backpressure(RuntimeReceipt),
    Buffered {
        receipt: RuntimeReceipt,
        effect_ack: EffectAckState,
    },
    HandedOff,
    Held(HoldReason),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    AdmissionUncertain,
    Queued,
    DispatchHeld,
    EffectRecorded,
    Buffered,
    HandedOff,
}
struct RetainedRun {
    request: LaunchRequest,
    contract_revision: u32,
    durable: Option<DurableIntent>,
    budget: Option<RetainedBudget>,
    slot: Option<RetainedSlot>,
    phase: Phase,
    receipt: Option<RuntimeReceipt>,
    // Independent of closure/buffer phase; only a confirmed fenced ACK sets it.
    effect_recorded: bool,
}

pub struct NativeTaskLoop {
    store: Store,
    config: NativeTaskLoopConfig,
    executor: NativeExecutor,
    runs: BTreeMap<String, RetainedRun>,
    completions: BTreeMap<String, ClosedExecution>,
}
impl NativeTaskLoop {
    pub fn new(
        store: Store,
        gate: ResourceGate,
        config: NativeTaskLoopConfig,
        runtime: NativeTaskRuntimeConfig,
    ) -> Result<Self, LoopError> {
        // A whole extra clock second covers dispatch's integer-second boundary;
        // this remains a lease guard, not a promise that Store I/O cannot block.
        let minimum_lease = runtime
            .deadline
            .as_secs()
            .checked_add(u64::from(runtime.deadline.subsec_nanos() != 0))
            .and_then(|s| s.checked_add(1))
            .ok_or(LoopError::InvalidConfiguration)?;
        if !(1..=MAX_RETAINED_RUNS).contains(&config.max_retained_runs)
            || !(1..=config.max_retained_runs).contains(&config.max_buffered_completions)
            || !bounded(&config.dispatch_owner, 128)
            || !(minimum_lease..=MAX_LEASE_SECONDS).contains(&config.dispatch_lease_secs)
        {
            return Err(LoopError::InvalidConfiguration);
        }
        let sink = StoreStartedSink::new(
            store.clone(),
            config.scope.clone(),
            config.max_retained_runs,
        )
        .map_err(|_| LoopError::InvalidConfiguration)?;
        let executor = NativeExecutor::new(
            gate,
            runtime.journal_root,
            runtime.deadline,
            runtime.capture_limit,
            config.max_retained_runs,
            Arc::new(sink),
        )
        .map_err(|_| LoopError::InvalidConfiguration)?;
        Ok(Self {
            store,
            config,
            executor,
            runs: BTreeMap::new(),
            completions: BTreeMap::new(),
        })
    }

    /// Register the actual plan before attempting an atomic claim+intent. Every
    /// ambiguous admission reply retains both registration and frozen request.
    /// Recovery never recreates a prepared workspace from a path or Store JSON.
    pub fn admit(
        &mut self,
        request: LaunchRequest,
        plan: NativeExecutionPlan,
        admission: &impl CurrentAdmission,
    ) -> Result<AdmissionStatus, RejectedAdmission> {
        self.admit_mode(request, plan, admission, None)
    }

    /// Atomically reserve budget with the task claim and launch intent. The
    /// exact command/proof and actual plan reservation stay with this retained
    /// run across ambiguous replies. No caller can replace them on a retry.
    pub fn admit_reserved(
        &mut self,
        request: LaunchRequest,
        plan: NativeExecutionPlan,
        admission: &impl CurrentAdmission,
        budget: NativeBudgetAdmission,
    ) -> Result<AdmissionStatus, RejectedAdmission> {
        self.admit_mode(request, plan, admission, Some(budget))
    }

    /// Use the original private Codex preparation binding. Public request/model
    /// strings cannot create this value. Current Store policies remain separate
    /// from the factory's inert binding and are sampled again on every retry.
    pub fn admit_slotted(
        &mut self,
        request: LaunchRequest,
        plan: NativeExecutionPlan,
        admission: &impl CurrentAdmission,
        slot: NativeCodexSlotAdmission,
    ) -> Result<AdmissionStatus, RejectedSlotAdmission> {
        let inputs = slot.inputs();
        let budget = NativeBudgetAdmission {
            command: inputs.budget_command.clone(),
            proof: Arc::clone(&inputs.budget_proof),
            read_proof: Arc::clone(slot.budget_read_proof()),
        };
        let checked = self.preflight(&request, &plan).and_then(|revision| {
            if !slot.matches_plan(&request, &plan) {
                return Err(LoopError::InvalidBinding);
            }
            self.validate_budget(&request, &plan, &budget)?;
            let SlotEvent::Hold { hold_id, claim } = &inputs.slot_command.event else {
                return Err(LoopError::InvalidBinding);
            };
            let ledger = self
                .store
                .inference_slot_ledger(Arc::clone(slot.slot_read_proof()))
                .map_err(|_| LoopError::StoreUnavailable)?;
            let pool = ledger
                .pools
                .get(&claim.pool_id)
                .ok_or(LoopError::InvalidBinding)?;
            if pool.generation != claim.generation {
                return Err(LoopError::InvalidBinding);
            }
            let slot_revision = inputs
                .slot_command
                .expected_ledger_revision
                .checked_add(1)
                .ok_or(LoopError::InvalidBinding)?;
            Ok((
                revision,
                SlotHold {
                    id: hold_id.clone(),
                    claim: claim.clone(),
                },
                pool.capacity_key.clone(),
                slot_revision,
            ))
        });
        let (revision, expected_slot, pool_key, slot_revision) = match checked {
            Ok(checked) => checked,
            Err(reason) => {
                return Err(RejectedSlotAdmission {
                    reason,
                    plan: Box::new(plan),
                    slot: Box::new(slot),
                });
            }
        };
        let Some(budget_revision) = budget.command.expected_ledger_revision.checked_add(1) else {
            return Err(RejectedSlotAdmission {
                reason: LoopError::InvalidBinding,
                plan: Box::new(plan),
                slot: Box::new(slot),
            });
        };
        let mut expected_budget = plan.reservation.clone();
        expected_budget.revision = budget_revision;
        if let Err(failure) = self.executor.try_register_plan(plan) {
            let (_, plan) = failure.into_parts();
            return Err(RejectedSlotAdmission {
                reason: LoopError::RegistrationRejected,
                plan: Box::new(plan),
                slot: Box::new(slot),
            });
        }
        let run_id = request.run_id.clone();
        self.runs.insert(
            run_id.clone(),
            RetainedRun {
                request,
                contract_revision: revision,
                durable: None,
                budget: Some(RetainedBudget {
                    admission: budget,
                    expected: expected_budget,
                }),
                slot: Some(RetainedSlot {
                    admission: slot,
                    expected: expected_slot,
                    pool_key,
                    revision: slot_revision,
                }),
                phase: Phase::AdmissionUncertain,
                receipt: None,
                effect_recorded: false,
            },
        );
        Ok(self.admit_retained(&run_id, admission))
    }

    fn admit_mode(
        &mut self,
        request: LaunchRequest,
        plan: NativeExecutionPlan,
        admission: &impl CurrentAdmission,
        budget: Option<NativeBudgetAdmission>,
    ) -> Result<AdmissionStatus, RejectedAdmission> {
        let checked = self.preflight(&request, &plan).and_then(|revision| {
            if let Some(budget) = &budget {
                self.validate_budget(&request, &plan, budget)?;
            }
            Ok(revision)
        });
        let revision = match checked {
            Ok(revision) => revision,
            Err(reason) => {
                return Err(RejectedAdmission {
                    reason,
                    plan: Box::new(plan),
                    budget,
                });
            }
        };
        // Capacity and duplicate checks precede ownership transfer. The runtime's
        // ownership-returning API also retains the plan on journal/capacity errors.
        let budget = match budget {
            Some(admission) => {
                let Some(revision) = admission.command.expected_ledger_revision.checked_add(1)
                else {
                    return Err(RejectedAdmission {
                        reason: LoopError::InvalidBinding,
                        plan: Box::new(plan),
                        budget: Some(admission),
                    });
                };
                let mut expected = plan.reservation.clone();
                expected.revision = revision;
                Some(RetainedBudget {
                    admission,
                    expected,
                })
            }
            None => None,
        };
        if let Err(failure) = self.executor.try_register_plan(plan) {
            let (_, plan) = failure.into_parts();
            return Err(RejectedAdmission {
                reason: LoopError::RegistrationRejected,
                plan: Box::new(plan),
                budget: budget.map(|retained| retained.admission),
            });
        }
        let run_id = request.run_id.clone();
        self.runs.insert(
            run_id.clone(),
            RetainedRun {
                request,
                contract_revision: revision,
                durable: None,
                budget,
                slot: None,
                phase: Phase::AdmissionUncertain,
                receipt: None,
                effect_recorded: false,
            },
        );
        // No withdrawal/re-registration after this point, even on a Store error.
        Ok(self.admit_retained(&run_id, admission))
    }

    pub fn retry_admission(
        &mut self,
        run_id: &str,
        admission: &impl CurrentAdmission,
    ) -> Result<AdmissionStatus, LoopError> {
        let run = self.runs.get(run_id).ok_or(LoopError::UnknownRun)?;
        match run.phase {
            Phase::AdmissionUncertain => Ok(self.admit_retained(run_id, admission)),
            Phase::Queued if run.budget.is_some() => Ok(self.admit_retained(run_id, admission)),
            Phase::Queued => Ok(AdmissionStatus::Queued(
                run.durable.clone().ok_or(LoopError::InvalidBinding)?,
            )),
            _ => Ok(AdmissionStatus::Held(HoldReason::DispatchUncertain)),
        }
    }

    fn admit_retained(
        &mut self,
        run_id: &str,
        admission: &impl CurrentAdmission,
    ) -> AdmissionStatus {
        let Some(run) = self.runs.get_mut(run_id) else {
            return AdmissionStatus::Held(HoldReason::AdmissionUncertain);
        };
        let scoped = ScopedAdmission {
            scope: &self.config.scope,
            owner: &self.config.task_owner,
            request: &run.request,
            revision: run.contract_revision,
            inner: admission,
        };
        let result = if let Some(slot) = &run.slot {
            controller::admit_one_slotted(
                &self.store,
                &scoped,
                &run.request,
                clone_slot_inputs(slot),
            )
            .map(|result| {
                (
                    result.intent,
                    Some(result.reservation.receipt.ledger_revision),
                    Some(result.slot.receipt.ledger_revision),
                )
            })
        } else {
            match &run.budget {
                Some(budget) => controller::admit_one_reserved(
                    &self.store,
                    &scoped,
                    &run.request,
                    budget.admission.command.clone(),
                    Arc::clone(&budget.admission.proof),
                )
                .map(|result| {
                    (
                        result.intent,
                        Some(result.reservation.receipt.ledger_revision),
                        None,
                    )
                }),
                None => controller::admit_one(&self.store, &scoped, &run.request)
                    .map(|intent| (intent, None, None)),
            }
        };
        match result {
            Ok((durable, budget_revision, slot_revision)) => {
                if run.budget.as_ref().is_some_and(|budget| {
                    budget_revision != Some(budget.expected.revision)
                        || !budget_current(&self.store, budget)
                }) || run.slot.as_ref().is_some_and(|slot| {
                    slot_revision != Some(slot.revision) || !slot_current(&self.store, slot)
                }) {
                    // The atomic write may already be durable. Retain all
                    // custody; no ordinary admission or registration fallback.
                    return AdmissionStatus::Held(HoldReason::AdmissionUncertain);
                }
                run.durable = Some(durable.clone());
                run.phase = Phase::Queued;
                AdmissionStatus::Queued(durable)
            }
            Err(_) => AdmissionStatus::Held(HoldReason::AdmissionUncertain),
        }
    }

    /// Dispatch one exact retained key. Uncertainty is never reset to queued.
    /// The external adapter must authenticate current project/resource policy.
    pub fn dispatch(
        &mut self,
        run_id: &str,
        clock: &impl DispatchClock,
        admission: &impl DispatchAdmission,
    ) -> Result<TaskDispatchStatus, LoopError> {
        let run = self.runs.get(run_id).ok_or(LoopError::UnknownRun)?;
        if run.phase != Phase::Queued {
            return Ok(TaskDispatchStatus::Held {
                reason: HoldReason::DispatchUncertain,
                runtime_attempted: false,
            });
        }
        let durable = run.durable.as_ref().ok_or(LoopError::InvalidBinding)?;
        let key = launch_key(&run.request);
        if durable.outbox_key != key {
            return Err(LoopError::InvalidBinding);
        }
        let scoped = ScopedDispatch {
            scope: &self.config.scope,
            owner: &self.config.task_owner,
            request: &run.request,
            revision: run.contract_revision,
            inner: admission,
            store: &self.store,
            durable,
            budget: run.budget.as_ref(),
            slot: run.slot.as_ref(),
        };
        let runtime = RecordingRuntime {
            executor: &self.executor,
            request: &run.request,
            receipt: RefCell::new(None),
            attempted: Cell::new(false),
            final_budget_admission: run
                .budget
                .is_some()
                .then_some(&scoped as &dyn DispatchAdmission),
        };
        let result = controller::dispatch_key(
            &self.store,
            clock,
            self.config.dispatch_owner.clone(),
            self.config.dispatch_lease_secs,
            key,
            &scoped,
            &runtime,
        );
        let receipt = runtime.receipt.into_inner();
        let attempted = runtime.attempted.get();
        let run = self.runs.get_mut(run_id).ok_or(LoopError::InvalidBinding)?;
        if let Some(receipt) = receipt {
            run.receipt = Some(receipt);
        }
        match result {
            Ok(DispatchOutcome::Idle) => Ok(TaskDispatchStatus::Idle),
            Ok(DispatchOutcome::Completed { .. } | DispatchOutcome::Failed { .. }) => {
                run.effect_recorded = true;
                run.phase = Phase::EffectRecorded;
                match run.receipt.clone() {
                    Some(receipt) => Ok(TaskDispatchStatus::EffectRecorded(receipt)),
                    None => {
                        run.phase = Phase::DispatchHeld;
                        Ok(TaskDispatchStatus::Held {
                            reason: HoldReason::RuntimeUncertain,
                            runtime_attempted: attempted,
                        })
                    }
                }
            }
            Err(controller::ControllerError::ClockUnavailable) => {
                // This variant is returned only before attempting Store's claim.
                // A fresh explicit call may safely retry once the clock returns.
                Ok(TaskDispatchStatus::Held {
                    reason: HoldReason::DispatchUncertain,
                    runtime_attempted: false,
                })
            }
            Ok(DispatchOutcome::Unknown { .. }) | Err(_) => {
                run.phase = Phase::DispatchHeld;
                Ok(TaskDispatchStatus::Held {
                    reason: HoldReason::DispatchUncertain,
                    runtime_attempted: attempted,
                })
            }
        }
    }

    /// Re-observe retained runtime custody; this never relaunches or reclaims an
    /// expired/unknown outbox intent. Proof stays in the executor at backpressure.
    pub fn reconcile_completion(&mut self, run_id: &str) -> Result<CompletionStatus, LoopError> {
        let run = self.runs.get(run_id).ok_or(LoopError::UnknownRun)?;
        if run.phase == Phase::HandedOff {
            return Ok(CompletionStatus::HandedOff);
        }
        let effect_ack = ack_state(run);
        if let Some(closed) = self.completions.get(run_id) {
            return Ok(CompletionStatus::Buffered {
                receipt: closed.receipt().clone(),
                effect_ack,
            });
        }
        let receipt = match self.executor.observe_pending(run_id) {
            Ok(Some(receipt)) => receipt,
            Ok(None) => return Ok(CompletionStatus::WaitingForRuntime),
            Err(_) => return Ok(CompletionStatus::Held(HoldReason::RuntimeUncertain)),
        };
        if !receipt_matches(&run.request, &receipt) {
            return Err(LoopError::InvalidBinding);
        }
        // All identity/capacity work precedes the one-shot transfer. No fallible
        // downstream work may consume/drop the original preparation or closure.
        match buffer_completion(
            &mut self.completions,
            self.config.max_buffered_completions,
            run_id,
            || self.executor.take_closed(&receipt),
        ) {
            Ok(BufferOutcome::Full) => return Ok(CompletionStatus::Backpressure(receipt)),
            Ok(BufferOutcome::Buffered) => {}
            _ => return Ok(CompletionStatus::Held(HoldReason::CompletionUnavailable)),
        }
        if let Some(run) = self.runs.get_mut(run_id) {
            run.receipt = Some(receipt.clone());
            run.phase = Phase::Buffered;
        }
        Ok(CompletionStatus::Buffered {
            receipt,
            effect_ack,
        })
    }

    pub fn completion(&self, run_id: &str) -> Option<&ClosedExecution> {
        self.completions.get(run_id)
    }
    /// Read retained advisory diagnostics without observing, signalling or
    /// reconciling a run. This grants no effect acknowledgement or closure.
    pub fn pending_diagnostics(&self, run_id: &str) -> Option<PendingDiagnostics> {
        self.runs.get(run_id)?;
        self.executor.pending_diagnostics(run_id)
    }
    pub fn effect_ack_state(&self, run_id: &str) -> Result<EffectAckState, LoopError> {
        self.runs
            .get(run_id)
            .map(ack_state)
            .ok_or(LoopError::UnknownRun)
    }
    /// Trusted receiver assumes custody; this is not publication or acceptance.
    /// Retain the returned proof through any fallible source/check/Store steps.
    /// Unconfirmed ACKs keep the proof here; no local setter can resolve them.
    pub fn take_completion(&mut self, run_id: &str) -> Option<ClosedExecution> {
        let run = self.runs.get_mut(run_id)?;
        if run.phase != Phase::Buffered {
            return None;
        }
        let closed = take_confirmed_completion(&mut self.completions, run_id, ack_state(run))?;
        run.phase = Phase::HandedOff;
        Some(closed)
    }

    fn preflight(
        &self,
        request: &LaunchRequest,
        plan: &NativeExecutionPlan,
    ) -> Result<u32, LoopError> {
        if self.runs.len() >= self.config.max_retained_runs {
            return Err(LoopError::Capacity);
        }
        if self.runs.contains_key(&request.run_id)
            || self
                .runs
                .values()
                .any(|r| r.request.task_id == request.task_id)
        {
            return Err(LoopError::InvalidBinding);
        }
        validate_request(request)?;
        let snapshot = self
            .store
            .snapshot()
            .map_err(|_| LoopError::StoreUnavailable)?;
        let task = snapshot
            .projection
            .tasks
            .get(&request.task_id)
            .ok_or(LoopError::InvalidBinding)?;
        if task.state != SchedulingState::Ready || task.attempt_id.is_some() || task.owner.is_some()
        {
            return Err(LoopError::InvalidBinding);
        }
        validate_task(&self.config.scope, &self.config.task_owner, task, request)?;
        validate_plan(&self.config.scope, task, request, plan)?;
        Ok(task.contract.revision)
    }

    fn validate_budget(
        &self,
        request: &LaunchRequest,
        plan: &NativeExecutionPlan,
        budget: &NativeBudgetAdmission,
    ) -> Result<(), LoopError> {
        validate_command(&budget.command).map_err(|_| LoopError::InvalidBinding)?;
        let ReservationEvent::Reserve {
            reservation_id,
            scope,
            correlation_id,
            amount,
            ..
        } = &budget.command.event
        else {
            return Err(LoopError::InvalidBinding);
        };
        let expected = &plan.reservation;
        if budget.command.source != request.source
            || budget.command.source.incarnation != scope.incarnation
            || reservation_id != &expected.id
            || scope != &expected.scope
            || correlation_id != &expected.correlation_id
            || amount != &expected.original
            || budget
                .command
                .expected_ledger_revision
                .checked_add(1)
                .is_none()
            || expected.settled.get() != 0
            || expected.stop_requested
            || !expected.outstanding
        {
            return Err(LoopError::InvalidBinding);
        }
        let snapshot = self
            .store
            .snapshot()
            .map_err(|_| LoopError::StoreUnavailable)?;
        let task = snapshot
            .projection
            .tasks
            .get(&request.task_id)
            .ok_or(LoopError::InvalidBinding)?;
        let intent = controller::planned_launch_intent(task, request)
            .map_err(|_| LoopError::InvalidBinding)?;
        if budget.command.launch_intent_digest.as_deref()
            != Some(
                admiralty_store::outbox::intent_digest(&intent)
                    .map_err(|_| LoopError::InvalidBinding)?
                    .as_str(),
            )
        {
            return Err(LoopError::InvalidBinding);
        }
        // Check the separate read capability before registering the plan. This
        // is an observational policy sample, not a lease through admission;
        // dispatch repeats the read against the retained exact reservation.
        self.store
            .reservation_ledger(Arc::clone(&budget.read_proof))
            .map_err(|_| LoopError::StoreUnavailable)?;
        Ok(())
    }
}

fn budget_current(store: &Store, budget: &RetainedBudget) -> bool {
    store
        .reservation_ledger(Arc::clone(&budget.admission.read_proof))
        .is_ok_and(|ledger| ledger.reservations.get(&budget.expected.id) == Some(&budget.expected))
}

fn clone_slot_inputs(slot: &RetainedSlot) -> admiralty_store::BudgetAndSlotAdmission {
    let inputs = slot.admission.inputs();
    admiralty_store::BudgetAndSlotAdmission {
        budget_command: inputs.budget_command.clone(),
        budget_proof: Arc::clone(&inputs.budget_proof),
        slot_command: inputs.slot_command.clone(),
        slot_proof: Arc::clone(&inputs.slot_proof),
    }
}

fn slot_current(store: &Store, slot: &RetainedSlot) -> bool {
    store
        .inference_slot_ledger(Arc::clone(slot.admission.slot_read_proof()))
        .is_ok_and(|ledger| {
            ledger.holds.get(&slot.expected.id) == Some(&slot.expected)
                && ledger
                    .pools
                    .get(&slot.expected.claim.pool_id)
                    .is_some_and(|pool| {
                        pool.capacity_key == slot.pool_key
                            && pool.generation == slot.expected.claim.generation
                    })
        })
}

fn ack_state(run: &RetainedRun) -> EffectAckState {
    if run.effect_recorded {
        EffectAckState::Recorded
    } else {
        EffectAckState::Unconfirmed
    }
}
fn take_confirmed_completion<T>(
    buffer: &mut BTreeMap<String, T>,
    run_id: &str,
    ack: EffectAckState,
) -> Option<T> {
    if ack != EffectAckState::Recorded {
        return None;
    }
    buffer.remove(run_id)
}

enum BufferOutcome {
    Full,
    Missing,
    Buffered,
}
fn buffer_completion<T, E>(
    buffer: &mut BTreeMap<String, T>,
    limit: usize,
    run_id: &str,
    take: impl FnOnce() -> Result<Option<T>, E>,
) -> Result<BufferOutcome, E> {
    if buffer.contains_key(run_id) {
        return Ok(BufferOutcome::Buffered);
    }
    if buffer.len() >= limit {
        return Ok(BufferOutcome::Full);
    }
    let Some(owned) = take()? else {
        return Ok(BufferOutcome::Missing);
    };
    buffer.insert(run_id.to_owned(), owned);
    Ok(BufferOutcome::Buffered)
}

struct ScopedAdmission<'a, A> {
    scope: &'a StartedScope,
    owner: &'a PrincipalId,
    request: &'a LaunchRequest,
    revision: u32,
    inner: &'a A,
}
impl<A: CurrentAdmission> CurrentAdmission for ScopedAdmission<'_, A> {
    fn authorize_current(&self, task: &TaskView, request: &LaunchRequest) -> Result<(), String> {
        if request != self.request
            || task.contract.revision != self.revision
            || validate_task(self.scope, self.owner, task, request).is_err()
        {
            return Err("frozen admission binding differs".into());
        }
        self.inner
            .authorize_current(task, request)
            .map_err(|_| "trusted admission denied".into())
    }
}
struct ScopedDispatch<'a, A> {
    scope: &'a StartedScope,
    owner: &'a PrincipalId,
    request: &'a LaunchRequest,
    revision: u32,
    inner: &'a A,
    store: &'a Store,
    durable: &'a DurableIntent,
    budget: Option<&'a RetainedBudget>,
    slot: Option<&'a RetainedSlot>,
}
impl<A: DispatchAdmission> DispatchAdmission for ScopedDispatch<'_, A> {
    fn authorize_dispatch(&self, task: &TaskView, intent: &DispatchIntent) -> Result<(), String> {
        if !intent_matches(self.request, self.revision, intent)
            || validate_task(self.scope, self.owner, task, self.request).is_err()
        {
            return Err("frozen dispatch binding differs".into());
        }
        if let Some(budget) = self.budget {
            let current = DispatchReplayAdmission {
                scope: self.scope,
                owner: self.owner,
                request: self.request,
                revision: self.revision,
                inner: self.inner,
            };
            let (replay_intent, replay_reservation, replay_slot) = if let Some(slot) = self.slot {
                let replay = controller::admit_one_slotted(
                    self.store,
                    &current,
                    self.request,
                    clone_slot_inputs(slot),
                )
                .map_err(|_| "current slotted dispatch admission denied".to_owned())?;
                (replay.intent, replay.reservation, Some(replay.slot))
            } else {
                let replay = controller::admit_one_reserved(
                    self.store,
                    &current,
                    self.request,
                    budget.admission.command.clone(),
                    Arc::clone(&budget.admission.proof),
                )
                .map_err(|_| "current budget dispatch admission denied".to_owned())?;
                (replay.intent, replay.reservation, None)
            };
            let mut expected = self.durable.clone();
            expected.duplicate = true;
            if replay_intent != expected
                || !replay_reservation.replayed
                || replay_reservation.receipt.ledger_revision != budget.expected.revision
                || !budget_current(self.store, budget)
                || self.slot.is_some_and(|slot| {
                    replay_slot.as_ref().is_none_or(|result| {
                        !result.replayed || result.receipt.ledger_revision != slot.revision
                    }) || !slot_current(self.store, slot)
                })
            {
                return Err("frozen budget dispatch binding differs".into());
            }
            Ok(())
        } else {
            self.inner
                .authorize_dispatch(task, intent)
                .map_err(|_| "trusted dispatch admission denied".into())
        }
    }
}

/// Adapter for an exact already-claimed budget replay at the dispatch boundary.
/// It cannot make a Ready/historical task newly eligible while reauthorizing.
struct DispatchReplayAdmission<'a, A> {
    scope: &'a StartedScope,
    owner: &'a PrincipalId,
    request: &'a LaunchRequest,
    revision: u32,
    inner: &'a A,
}
impl<A: DispatchAdmission> CurrentAdmission for DispatchReplayAdmission<'_, A> {
    fn authorize_current(&self, task: &TaskView, request: &LaunchRequest) -> Result<(), String> {
        if request != self.request
            || task.contract.revision != self.revision
            || task.state != SchedulingState::Claimed
            || task.attempt_id.as_ref() != Some(&request.attempt_id)
            || task.owner.as_ref() != Some(&request.owner)
            || validate_task(self.scope, self.owner, task, request).is_err()
        {
            return Err("frozen budget replay is not the current claimed attempt".into());
        }
        let planned = controller::planned_launch_intent(task, request)
            .map_err(|_| "budget replay intent differs".to_owned())?;
        let intent: DispatchIntent = serde_json::from_value(planned.payload)
            .map_err(|_| "budget replay intent is invalid".to_owned())?;
        self.inner
            .authorize_dispatch(task, &intent)
            .map_err(|_| "trusted budget dispatch admission denied".into())
    }
}
struct RecordingRuntime<'a> {
    executor: &'a NativeExecutor,
    request: &'a LaunchRequest,
    receipt: RefCell<Option<RuntimeReceipt>>,
    attempted: Cell<bool>,
    final_budget_admission: Option<&'a dyn DispatchAdmission>,
}
impl NativeRuntimeExecutor for RecordingRuntime<'_> {
    fn execute(&self, task: &TaskView, intent: &DispatchIntent) -> Result<RuntimeReceipt, String> {
        if !intent_matches(self.request, task.contract.revision, intent) {
            return Err("runtime dispatch binding differs".into());
        }
        if let Some(admission) = self.final_budget_admission {
            // The controller reads its clock after the first admission check.
            // Revalidate any intervening stop/revocation before calling the
            // executor. This is no claim of atomic physical launch with Store.
            admission
                .authorize_dispatch(task, intent)
                .map_err(|_| "current budget runtime admission denied".to_owned())?;
        }
        self.attempted.set(true);
        let receipt = self.executor.execute(task, intent)?;
        if !receipt_matches(self.request, &receipt) {
            return Err("runtime receipt binding differs".into());
        }
        *self.receipt.borrow_mut() = Some(receipt.clone());
        Ok(receipt)
    }
}
fn launch_key(request: &LaunchRequest) -> String {
    format!("launch/{}/{}", request.task_id, request.attempt_id)
}
fn receipt_matches(request: &LaunchRequest, receipt: &RuntimeReceipt) -> bool {
    receipt.task_id == request.task_id
        && receipt.attempt_id == request.attempt_id
        && receipt.run_id == request.run_id
}
fn intent_matches(r: &LaunchRequest, revision: u32, i: &DispatchIntent) -> bool {
    i.schema_version == 1
        && i.task_id == r.task_id.as_str()
        && i.contract_revision == revision
        && i.contract_digest == r.expected_contract_digest
        && i.attempt_id == r.attempt_id.as_str()
        && i.owner == r.owner.as_str()
        && i.run_id == r.run_id
        && i.context_digest == r.context_digest
        && i.policy_digest == r.policy_digest
        && i.provider == r.provider
        && i.model == r.model
}
fn bounded(s: &str, limit: usize) -> bool {
    !s.is_empty() && s.len() <= limit && !s.chars().any(char::is_control)
}
fn digest(s: &str) -> bool {
    s.strip_prefix("sha256:").is_some_and(|h| {
        h.len() == 64
            && h.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}
fn validate_request(r: &LaunchRequest) -> Result<(), LoopError> {
    if r.source.validate().ok().is_none_or(|s| s > i64::MAX as u64)
        || !bounded(&r.source.producer_id, 256)
        || !bounded(&r.source.incarnation, 256)
        || !bounded(&r.run_id, 128)
        || !r
            .run_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
        || ![
            &r.expected_contract_digest,
            &r.context_digest,
            &r.policy_digest,
        ]
        .into_iter()
        .all(|d| digest(d))
        || !bounded(&r.provider, 128)
        || !bounded(&r.model, 128)
        || !bounded(&r.observed_utc, 128)
        || r.not_before > i64::MAX as u64
        || launch_key(r).len() > 256
    {
        return Err(LoopError::InvalidBinding);
    }
    Ok(())
}
fn validate_task(
    scope: &StartedScope,
    owner: &PrincipalId,
    task: &TaskView,
    request: &LaunchRequest,
) -> Result<(), LoopError> {
    if task.contract.validate().is_err()
        || task.contract.simulated
        || task.contract.project_id != scope.project_id
        || task.contract.root_id != scope.root_id
        || task.contract.task_id != request.task_id
        || &request.owner != owner
        || content_digest(&task.contract).map_err(|_| LoopError::InvalidBinding)?
            != request.expected_contract_digest
    {
        return Err(LoopError::InvalidBinding);
    }
    Ok(())
}
fn validate_plan(
    scope: &StartedScope,
    task: &TaskView,
    r: &LaunchRequest,
    p: &NativeExecutionPlan,
) -> Result<(), LoopError> {
    if ![
        &p.permit_claims.issuer_ref,
        &p.permit_claims.permit_id,
        &p.permit_claims.reservation_ref,
        &p.permit_claims.host_incarnation,
    ]
    .into_iter()
    .all(|s| bounded(s, 256))
        || admiralty_core::limits::bounded_json_size(&p.reservation, 16 * 1024).is_err()
    {
        return Err(LoopError::InvalidBinding);
    }
    validate_launch_envelope(&p.launch_spec).map_err(|_| LoopError::InvalidBinding)?;
    let checked = p
        .workspace
        .validate()
        .map_err(|_| LoopError::InvalidBinding)?;
    let prepared = p.prepared_workspace.workspace();
    let identity = &p.permit_claims.identity;
    let s = &p.reservation.scope;
    let owned: Vec<PathBuf> = task
        .contract
        .owned_paths
        .iter()
        .map(PathBuf::from)
        .collect();
    let forbidden: Vec<PathBuf> = task
        .contract
        .forbidden_paths
        .iter()
        .map(PathBuf::from)
        .collect();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| u64::try_from(d.as_millis()).ok())
        .ok_or(LoopError::InvalidBinding)?;
    if p.task_id != r.task_id
        || p.attempt_id != r.attempt_id
        || p.run_id != r.run_id
        || p.launch_spec.run_id != r.run_id
        || p.contract_digest != r.expected_contract_digest
        || p.context_digest != r.context_digest
        || p.policy_digest != r.policy_digest
        || p.provider_declared != r.provider
        || p.model_declared != r.model
        || identity.project_id != scope.project_id
        || identity.node_id != scope.node_id
        || identity.task_id != r.task_id
        || identity.attempt_id != r.attempt_id
        || p.permit_claims.reservation_ref != p.reservation.id
        || !bounded(&identity.lease_id, 256)
        || p.permit_claims.expires_at_unix_ms <= now
        || p.permit_claims.lease_expires_at_unix_ms <= now
        || p.permit_claims.launch_spec_digest
            != content_digest(&p.launch_spec).map_err(|_| LoopError::InvalidBinding)?
        || !digest(&p.permit_claims.worker_environment_digest)
        || !digest(&p.permit_claims.backend_config_digest)
        || p.permit_claims.cpu_set.is_empty()
        || p.permit_claims.cpu_set.len() > 4096
        || p.permit_claims.registered_cpu_scope.len() > 4096
        || !p
            .permit_claims
            .cpu_set
            .is_subset(&p.permit_claims.registered_cpu_scope)
        || p.permit_claims.memory_max_bytes == 0
        || p.permit_claims.max_tasks == 0
        || p.permit_claims.max_concurrency == 0
        || s.project_id != scope.project_id
        || s.root_id != scope.root_id
        || s.task_id != r.task_id
        || s.attempt_id != r.attempt_id
        || s.owner != r.owner
        || s.contract_digest != r.expected_contract_digest
        || s.contract_revision != task.contract.revision
        || s.authority_ref != task.contract.authority_ref
        || s.incarnation != p.permit_claims.host_incarnation
        || s.resource_claims_digest
            != native_resource_claims_digest(&p.permit_claims)
                .map_err(|_| LoopError::InvalidBinding)?
        || !p.reservation.outstanding
        || p.reservation.stop_requested
        || p.reservation.revision == 0
        || p.reservation.original.get() == 0
        || p.reservation.settled.get() >= p.reservation.original.get()
        || checked.project_id != scope.project_id
        || checked.root_id != scope.root_id
        || checked.node_id.as_str() != scope.node_id.as_str()
        || checked.workspace != p.launch_spec.cwd
        || prepared.project_id != scope.project_id
        || prepared.root_id != scope.root_id
        || prepared.node_id.as_str() != scope.node_id.as_str()
        || prepared.workspace != p.launch_spec.cwd
        || prepared.owned_paths != p.workspace.owned_paths
        || prepared.forbidden_paths != p.workspace.forbidden_paths
        || prepared.owned_paths != owned
        || prepared.forbidden_paths != forbidden
        || p.prepared_workspace.task_id() != &r.task_id
        || p.prepared_workspace.attempt_id() != &r.attempt_id
    {
        return Err(LoopError::InvalidBinding);
    }
    VerifiedResourcePermit::verify_current(
        p.permit_claims.clone(),
        Arc::clone(&p.permit_authority),
    )
    .map_err(|_| LoopError::InvalidBinding)?;
    Ok(())
}

#[cfg(test)]
#[path = "native_task_loop_budget_tests.rs"]
mod budget_tests;

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use admiralty_core::{
        AcceptanceCriterion, AttemptId, CheckSpec, DomainEvent, ProjectId, RootId, SourceIdentity,
        TaskContract, TaskId, nodes::NodeId,
    };
    use admiralty_runtime::{
        reservations::{AccountId, CurrencyUnit, MinorUnits, Reservation, ReservationScope},
        resource_gate::{AdmissionIdentity, ResourcePermitAuthority, ResourcePermitClaims},
        session::LaunchSpec,
        workspace::{
            preparation::{WorkspacePreparation, prepare_workspace},
            registry::{NodeRef, WorkspaceSpec},
        },
    };
    use std::{
        collections::BTreeSet,
        fs,
        os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt},
        path::Path,
    };

    // Explicit component-only issuer/admission doubles. ResourceGate is deliberately
    // unconfigured: no Linux enforcement, real provider or closure is qualified.
    struct CurrentPermit;
    impl ResourcePermitAuthority for CurrentPermit {
        fn is_current(&self, _: &ResourcePermitClaims) -> bool {
            true
        }
    }
    struct Permit;
    impl CurrentAdmission for Permit {
        fn authorize_current(&self, _: &TaskView, _: &LaunchRequest) -> Result<(), String> {
            Ok(())
        }
    }
    impl DispatchAdmission for Permit {
        fn authorize_dispatch(&self, _: &TaskView, _: &DispatchIntent) -> Result<(), String> {
            Ok(())
        }
    }
    struct Deny;
    impl CurrentAdmission for Deny {
        fn authorize_current(&self, _: &TaskView, _: &LaunchRequest) -> Result<(), String> {
            Err("test denial".into())
        }
    }
    impl DispatchAdmission for Deny {
        fn authorize_dispatch(&self, _: &TaskView, _: &DispatchIntent) -> Result<(), String> {
            Err("test denial".into())
        }
    }
    struct FixedClock(Option<u64>);
    impl DispatchClock for FixedClock {
        fn now_unix_seconds(&self) -> Option<u64> {
            self.0
        }
    }
    pub(super) struct TestFixture {
        pub(super) temp: PathBuf,
        pub(super) journal: PathBuf,
        pub(super) marker: PathBuf,
        pub(super) request: LaunchRequest,
        pub(super) plan: NativeExecutionPlan,
        pub(super) task_loop: NativeTaskLoop,
        pub(super) store: Store,
    }
    pub(super) fn fixture() -> TestFixture {
        fixture_with_authority(None)
    }
    pub(super) fn fixture_with_authority(
        authority: Option<Arc<dyn admiralty_store::admission_reservations::ReservationAuthority>>,
    ) -> TestFixture {
        let temp = private_fixture("native-executor-test");
        let source = temp.join("source");
        let managed_parent = temp.join("managed");
        let journal = temp.join("journal");
        fs::create_dir(&source).expect("source root");
        fs::create_dir(&managed_parent).expect("managed parent");
        fs::set_permissions(&managed_parent, fs::Permissions::from_mode(0o700))
            .expect("private managed parent");
        fs::create_dir(&journal).expect("journal root");
        fs::set_permissions(&journal, fs::Permissions::from_mode(0o700)).expect("private journal");
        let project_id = ProjectId::new("native-project").expect("project ID");
        let root_id = RootId::new("native-root").expect("root ID");
        let task_id = TaskId::new("native-task").expect("task ID");
        let attempt_id = AttemptId::new("native-attempt").expect("attempt ID");
        let owner = PrincipalId::new("native-owner").expect("owner ID");
        let node_id = NodeId::new("native-node").expect("node ID");
        fs::create_dir(source.join("src")).expect("source src directory");
        fs::write(source.join("src/seed.txt"), b"seed\n").expect("source seed");
        let git = |args: &[&str]| {
            let output = std::process::Command::new("/usr/bin/git")
                .env_clear()
                .env("PATH", "/usr/bin:/bin")
                .env("HOME", &temp)
                .env("TMPDIR", &temp)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_TERMINAL_PROMPT", "0")
                .args(["-c", "core.hooksPath=/dev/null"])
                .arg("-C")
                .arg(&source)
                .args(args)
                .output()
                .expect("run fixture git");
            assert!(
                output.status.success(),
                "git fixture command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8_lossy(&output.stdout).trim().to_owned()
        };
        git(&["init", "--quiet"]);
        git(&["config", "user.name", "Native Executor Test"]);
        git(&["config", "user.email", "native-executor@example.invalid"]);
        git(&["add", "src/seed.txt"]);
        git(&["commit", "--quiet", "-m", "fixture"]);
        let pinned_commit = git(&["rev-parse", "HEAD"]);
        make_fixture_metadata_private(&source.join(".git"));
        let source = fs::canonicalize(source).expect("canonical source");
        let managed_parent = fs::canonicalize(managed_parent).expect("canonical managed parent");
        let journal = fs::canonicalize(journal).expect("canonical journal");
        let prepared_workspace = prepare_workspace(WorkspacePreparation {
            source: source.clone(),
            managed_parent,
            destination_name: "prepared".into(),
            pinned_commit,
            node_id: NodeRef::new("native-node").expect("workspace node"),
            root_id: root_id.clone(),
            project_id: project_id.clone(),
            task_id: task_id.clone(),
            attempt_id: attempt_id.clone(),
            owned_paths: vec![PathBuf::from("src")],
            forbidden_paths: vec![],
        })
        .expect("actual pinned workspace preparation");
        let prepared_view = prepared_workspace.workspace();
        let workspace = prepared_view.workspace.clone();
        let marker = workspace.join("child-marker");
        let contract = TaskContract {
            schema_version: 1,
            project_id: project_id.clone(),
            root_id: root_id.clone(),
            task_id: task_id.clone(),
            revision: 1,
            title: "Native executor fixture".into(),
            objective: "Fail closed before a child when no backend is configured".into(),
            non_goals: vec![],
            owned_paths: vec!["src".into()],
            forbidden_paths: vec![],
            read_first: vec![],
            dependencies: vec![],
            criteria: vec![AcceptanceCriterion {
                id: "c1".into(),
                text: "bounded".into(),
                check_ids: vec!["check".into()],
            }],
            checks: vec![CheckSpec {
                id: "check".into(),
                program: "true".into(),
                args: vec![],
                cwd: ".".into(),
                timeout_secs: 1,
            }],
            authority_ref: "fixture:permit".into(),
            max_attempts: 1,
            simulated: false,
        };
        let contract_digest = content_digest(&contract).expect("contract digest");
        let context_digest = format!("sha256:{}", "a".repeat(64));
        let policy_digest = format!("sha256:{}", "b".repeat(64));
        let workspace_spec = WorkspaceSpec {
            node_id: prepared_view.node_id.clone(),
            root_id: prepared_view.root_id.clone(),
            project_id: prepared_view.project_id.clone(),
            workspace: prepared_view.workspace.clone(),
            owned_paths: prepared_view.owned_paths.clone(),
            forbidden_paths: prepared_view.forbidden_paths.clone(),
        };
        let launch_spec = LaunchSpec {
            run_id: "native-run-1".into(),
            program: std::env::current_exe().expect("test binary"),
            args: vec![
                "--exact".into(),
                "services::native_executor::tests::child_marker".into(),
                "--ignored".into(),
                "--nocapture".into(),
                "--".into(),
                marker.display().to_string(),
            ],
            cwd: workspace.clone(),
            spool_dir: workspace.join("spool"),
        };
        let mut reservation = Reservation {
            id: "native-reservation".into(),
            scope: ReservationScope {
                project_id: project_id.clone(),
                root_id: root_id.clone(),
                task_id: task_id.clone(),
                attempt_id: attempt_id.clone(),
                owner: owner.clone(),
                incarnation: "boot-1".into(),
                contract_revision: 1,
                contract_digest: contract_digest.clone(),
                authority_ref: contract.authority_ref.clone(),
                resource_claims_digest: "sha256:reservation".into(),
                account_id: AccountId::new("account").expect("account"),
                unit: CurrencyUnit::new("USD", 2).expect("unit"),
            },
            correlation_id: "native-attempt".into(),
            revision: 1,
            original: MinorUnits::new(1),
            settled: MinorUnits::new(0),
            stop_requested: false,
            outstanding: true,
        };
        let claims = ResourcePermitClaims {
            issuer_ref: "trusted-issuer".into(),
            permit_id: "permit-1".into(),
            reservation_ref: reservation.id.clone(),
            identity: AdmissionIdentity {
                project_id: project_id.clone(),
                task_id: task_id.clone(),
                attempt_id: attempt_id.clone(),
                node_id: node_id.clone(),
                lease_id: "lease-1".into(),
            },
            host_incarnation: "boot-1".into(),
            expires_at_unix_ms: u64::MAX,
            lease_expires_at_unix_ms: u64::MAX,
            registered_cpu_scope: BTreeSet::from([0]),
            cpu_set: BTreeSet::from([0]),
            memory_max_bytes: 1024,
            max_tasks: 1,
            max_concurrency: 1,
            launch_spec_digest: content_digest(&launch_spec).expect("actual launch binding"),
            worker_environment_digest: format!("sha256:{}", "c".repeat(64)),
            backend_config_digest: format!("sha256:{}", "d".repeat(64)),
        };
        reservation.scope.resource_claims_digest =
            native_resource_claims_digest(&claims).expect("full resource claims digest");
        let view = TaskView {
            contract,
            state: SchedulingState::Ready,
            attempt_id: None,
            owner: None,
            candidate_ref: None,
            lifecycle: None,
        };
        let intent = DispatchIntent {
            schema_version: 1,
            task_id: task_id.to_string(),
            contract_revision: 1,
            contract_digest: contract_digest.clone(),
            attempt_id: attempt_id.to_string(),
            owner: "native-owner".into(),
            run_id: "native-run-1".into(),
            context_digest,
            policy_digest,
            provider: "codex".into(),
            model: "declared-model".into(),
        };
        let plan = NativeExecutionPlan {
            task_id,
            attempt_id,
            contract_digest,
            context_digest: intent.context_digest.clone(),
            policy_digest: intent.policy_digest.clone(),
            provider_declared: intent.provider.clone(),
            model_declared: intent.model.clone(),
            run_id: intent.run_id.clone(),
            launch_spec,
            prepared_workspace,
            workspace: workspace_spec,
            reservation,
            permit_claims: claims,
            permit_authority: Arc::new(CurrentPermit),
        };
        let store = Store::open_with_startup_authorities(
            temp.join("loop.sqlite"),
            admiralty_store::StoreAuthorities {
                reservations: authority,
                ..Default::default()
            },
        )
        .expect("actual Store");
        store
            .append(
                admiralty_core::SourceIdentity {
                    producer_id: "native-loop-fixture".into(),
                    incarnation: "submit".into(),
                    sequence: "1".into(),
                },
                admiralty_core::DomainEvent::TaskSubmitted {
                    contract: view.contract.clone(),
                },
                "2026-10-18T00:00:00Z".into(),
            )
            .expect("durable task fixture");
        let request = LaunchRequest {
            source: admiralty_core::SourceIdentity {
                producer_id: "native-loop-fixture".into(),
                incarnation: "claim".into(),
                sequence: "1".into(),
            },
            task_id: view.contract.task_id.clone(),
            expected_contract_digest: intent.contract_digest.clone(),
            attempt_id: admiralty_core::AttemptId::new(intent.attempt_id).expect("attempt"),
            owner,
            run_id: intent.run_id,
            context_digest: intent.context_digest,
            policy_digest: intent.policy_digest,
            provider: intent.provider,
            model: intent.model,
            observed_utc: "2026-10-18T00:00:01Z".into(),
            not_before: 10,
        };
        let task_loop = NativeTaskLoop::new(
            store.clone(),
            ResourceGate::new(node_id.clone()),
            NativeTaskLoopConfig {
                scope: StartedScope {
                    project_id: view.contract.project_id.clone(),
                    root_id: view.contract.root_id.clone(),
                    node_id,
                    source_producer: "native-loop-started".into(),
                    source_incarnation: "service-one".into(),
                },
                task_owner: request.owner.clone(),
                dispatch_owner: "native-dispatch".into(),
                dispatch_lease_secs: 30,
                max_retained_runs: 4,
                max_buffered_completions: 1,
            },
            NativeTaskRuntimeConfig {
                journal_root: journal.clone(),
                deadline: Duration::from_millis(50),
                capture_limit: 1024,
            },
        )
        .expect("concrete scoped executor and Store sink");
        TestFixture {
            temp,
            journal,
            marker,
            request,
            plan,
            task_loop,
            store,
        }
    }
    fn make_fixture_metadata_private(path: &Path) {
        // A permissive caller umask must not make the fresh fixture shared.
        // Touch only this test's new Git metadata, never the process umask or
        // production custody rules. Reject aliases before recursive traversal.
        let metadata = fs::symlink_metadata(path).expect("fixture metadata");
        assert!(!metadata.file_type().is_symlink());
        assert_eq!(metadata.uid(), rustix::process::geteuid().as_raw());
        let mode = if metadata.is_dir() { 0o700 } else { 0o600 };
        assert!(metadata.is_dir() || metadata.is_file());
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
            .expect("private fixture metadata permissions");
        if metadata.is_dir() {
            for entry in fs::read_dir(path).expect("owned metadata children") {
                make_fixture_metadata_private(&entry.expect("owned child").path());
            }
        }
    }
    fn private_fixture(prefix: &str) -> PathBuf {
        // CI checkouts may have shared or differently owned ancestors. The
        // production journal/source custody guard must still reject them.
        // Create the fixture private at creation under the canonical temp root.
        let parent = fs::canonicalize(std::env::temp_dir()).expect("canonical temp root");
        let root = parent.join(format!("{prefix}-{}", uuid::Uuid::new_v4()));
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&root)
            .expect("create private fixture");
        root
    }

    #[test]
    fn full_preflight_rejects_stale_scope_identity_and_preserves_original_plan() {
        let mut f = fixture();
        assert!(f.task_loop.preflight(&f.request, &f.plan).is_ok());
        let original = f.request.clone();
        for field in [
            "contract", "context", "policy", "provider", "model", "run", "owner",
        ] {
            f.request = original.clone();
            match field {
                "contract" => {
                    f.request.expected_contract_digest = format!("sha256:{}", "f".repeat(64))
                }
                "context" => f.request.context_digest = format!("sha256:{}", "f".repeat(64)),
                "policy" => f.request.policy_digest = format!("sha256:{}", "f".repeat(64)),
                "provider" => f.request.provider = "other-provider".into(),
                "model" => f.request.model = "other-model".into(),
                "run" => f.request.run_id = "other-run".into(),
                _ => f.request.owner = PrincipalId::new("other-owner").expect("owner"),
            }
            assert_eq!(
                f.task_loop.preflight(&f.request, &f.plan),
                Err(LoopError::InvalidBinding),
                "{field}"
            );
        }
        f.request = original;
        let mut oversized_key = f.request.clone();
        oversized_key.task_id = TaskId::new("t".repeat(128)).expect("valid bounded task ID");
        oversized_key.attempt_id =
            AttemptId::new("a".repeat(128)).expect("valid bounded attempt ID");
        assert_eq!(
            validate_request(&oversized_key),
            Err(LoopError::InvalidBinding),
            "composite key must fit Store before registration"
        );
        let actual_head = f.plan.prepared_workspace.head_oid().to_owned();
        f.plan.permit_claims.identity.node_id = NodeId::new("foreign-node").expect("node");
        let rejected = f
            .task_loop
            .admit(f.request, f.plan, &Permit)
            .expect_err("foreign actual node binding");
        assert_eq!(rejected.reason, LoopError::InvalidBinding);
        assert_eq!(rejected.plan.prepared_workspace.head_oid(), actual_head);
        assert_eq!(
            f.store.snapshot().expect("unchanged task").cursor.event_seq,
            "1"
        );
        assert!(f.task_loop.runs.is_empty());
        drop(rejected);
        drop(f.task_loop);
        drop(f.store);
        fs::remove_dir_all(f.temp).expect("owned cleanup");
    }

    #[test]
    fn denied_admission_keeps_registration_and_exact_retry_claims_once() {
        let TestFixture {
            temp,
            request,
            plan,
            mut task_loop,
            store,
            ..
        } = fixture();
        assert_eq!(
            task_loop
                .admit(request.clone(), plan, &Deny)
                .expect("retained denial"),
            AdmissionStatus::Held(HoldReason::AdmissionUncertain)
        );
        assert_eq!(
            store.snapshot().expect("no denied write").cursor.event_seq,
            "1"
        );
        assert_eq!(task_loop.runs.len(), 1);
        let admitted = task_loop
            .retry_admission(&request.run_id, &Permit)
            .expect("same actual plan registration retained");
        let AdmissionStatus::Queued(first) = admitted else {
            panic!("expected durable claim")
        };
        assert!(!first.duplicate);
        assert_eq!(
            task_loop
                .retry_admission(&request.run_id, &Permit)
                .expect("cached exact retry"),
            AdmissionStatus::Queued(first)
        );
        assert_eq!(
            store.snapshot().expect("only one claim").cursor.event_seq,
            "2"
        );
        drop(task_loop);
        drop(store);
        fs::remove_dir_all(temp).expect("owned cleanup");
    }

    #[test]
    fn existing_recovery_evidence_rejects_registration_and_returns_actual_preparation() {
        let TestFixture {
            temp,
            request,
            plan,
            mut task_loop,
            store,
            journal,
            ..
        } = fixture();
        let head = plan.prepared_workspace.head_oid().to_owned();
        let evidence = journal.join(format!(
            "{}.prepared.json",
            content_digest(&request.run_id).expect("journal identity")
        ));
        fs::write(&evidence, b"retained prior recovery evidence")
            .expect("existing recovery barrier");
        fs::set_permissions(&evidence, fs::Permissions::from_mode(0o600))
            .expect("private fixture evidence");
        let rejected = task_loop
            .admit(request, plan, &Permit)
            .expect_err("no registration over prior launch evidence");
        assert_eq!(rejected.reason, LoopError::RegistrationRejected);
        assert_eq!(rejected.plan.prepared_workspace.head_oid(), head);
        assert_eq!(
            fs::read(&evidence).expect("evidence retained"),
            b"retained prior recovery evidence"
        );
        assert_eq!(
            store
                .snapshot()
                .expect("no durable admission")
                .cursor
                .event_seq,
            "1"
        );
        assert!(task_loop.runs.is_empty());
        drop(rejected);
        drop(task_loop);
        drop(store);
        fs::remove_dir_all(temp).expect("owned cleanup");
    }

    #[test]
    fn actual_durable_claim_with_lost_reply_replays_without_registering_or_spawning_again() {
        struct CommitThenLoseAck(Store);
        impl CurrentAdmission for CommitThenLoseAck {
            fn authorize_current(&self, _: &TaskView, r: &LaunchRequest) -> Result<(), String> {
                controller::admit_one(&self.0, &Permit, r)
                    .map_err(|_| "test transaction failed".to_owned())?;
                Err("simulated lost durable reply".into())
            }
        }
        let TestFixture {
            temp,
            request,
            plan,
            mut task_loop,
            store,
            marker,
            ..
        } = fixture();
        assert_eq!(
            task_loop
                .admit(request.clone(), plan, &CommitThenLoseAck(store.clone()))
                .expect("ambiguous admission retained"),
            AdmissionStatus::Held(HoldReason::AdmissionUncertain)
        );
        let before = store.snapshot().expect("actual committed claim");
        assert_eq!(
            before.projection.tasks[&request.task_id].state,
            SchedulingState::Claimed
        );
        let replay = task_loop
            .retry_admission(&request.run_id, &Permit)
            .expect("retry frozen source");
        assert!(matches!(
            replay,
            AdmissionStatus::Queued(DurableIntent {
                duplicate: true,
                ..
            })
        ));
        assert_eq!(store.snapshot().expect("same durable projection"), before);
        assert!(!marker.exists());
        assert!(matches!(
            task_loop
                .dispatch(&request.run_id, &FixedClock(Some(10)), &Permit)
                .expect("unconfigured real resource gate denies"),
            TaskDispatchStatus::Held {
                runtime_attempted: true,
                ..
            }
        ));
        assert!(
            !marker.exists(),
            "no provider or fake child bypasses ResourceGate"
        );
        assert_eq!(
            task_loop
                .reconcile_completion(&request.run_id)
                .expect("no closure invented"),
            CompletionStatus::WaitingForRuntime
        );
        assert!(task_loop.completion(&request.run_id).is_none());
        assert!(task_loop.take_completion(&request.run_id).is_none());
        assert_eq!(
            task_loop.effect_ack_state(&request.run_id),
            Ok(EffectAckState::Unconfirmed)
        );
        assert!(matches!(
            task_loop
                .retry_admission(&request.run_id, &Permit)
                .expect("cannot requeue unknown dispatch"),
            AdmissionStatus::Held(_)
        ));
        let state = store.snapshot().expect("no candidate or acceptance");
        assert_eq!(
            state.projection.tasks[&request.task_id].state,
            SchedulingState::Claimed
        );
        assert!(
            state.projection.tasks[&request.task_id]
                .candidate_ref
                .is_none()
        );
        drop(task_loop);
        drop(store);
        fs::remove_dir_all(temp).expect("owned cleanup");
    }

    #[test]
    fn exact_dispatch_key_leaves_foreign_pending_task_and_effect_untouched() {
        let TestFixture {
            temp,
            request,
            plan,
            mut task_loop,
            store,
            ..
        } = fixture();
        let mut foreign = store.snapshot().expect("task fixture").projection.tasks
            [&request.task_id]
            .contract
            .clone();
        foreign.project_id = ProjectId::new("foreign-project").expect("project");
        foreign.task_id = TaskId::new("foreign-task").expect("task");
        let foreign_task = foreign.task_id.clone();
        let mut foreign_request = request.clone();
        foreign_request.task_id = foreign_task.clone();
        foreign_request.expected_contract_digest =
            content_digest(&foreign).expect("foreign contract");
        foreign_request.attempt_id = AttemptId::new("foreign-attempt").expect("attempt");
        foreign_request.run_id = "foreign-run".into();
        foreign_request.not_before = 0;
        foreign_request.source.incarnation = "foreign-claim".into();
        store
            .append(
                SourceIdentity {
                    producer_id: "fixture-foreign".into(),
                    incarnation: "submit".into(),
                    sequence: "1".into(),
                },
                DomainEvent::TaskSubmitted { contract: foreign },
                "2026-10-18T00:00:00Z".into(),
            )
            .expect("foreign task");
        controller::admit_one(&store, &Permit, &foreign_request)
            .expect("foreign pending effect first");
        task_loop
            .admit(request.clone(), plan, &Permit)
            .expect("own scoped admission");
        assert!(matches!(
            task_loop
                .dispatch(&request.run_id, &FixedClock(Some(20)), &Deny)
                .expect("own dispatch denied"),
            TaskDispatchStatus::Held {
                runtime_attempted: false,
                ..
            }
        ));
        let untouched = store
            .claim_outbox_key(
                Some(21),
                "foreign-check".into(),
                30,
                launch_key(&foreign_request),
            )
            .expect("foreign key still pending")
            .expect("foreign effect was not consumed or marked unknown");
        assert_eq!(untouched.idempotency_key, launch_key(&foreign_request));
        assert_eq!(untouched.payload["task_id"], foreign_task.as_str());
        assert!(
            store
                .claim_outbox_key(Some(21), "own-check".into(), 30, launch_key(&request))
                .expect("own unknown remains held")
                .is_none()
        );
        assert_eq!(
            store
                .snapshot()
                .expect("foreign task retained")
                .projection
                .tasks[&foreign_task]
                .state,
            SchedulingState::Claimed
        );
        drop(task_loop);
        drop(store);
        fs::remove_dir_all(temp).expect("owned cleanup");
    }

    #[test]
    fn clock_unavailable_is_retryable_before_claim_but_expired_lease_never_launches() {
        struct MutableClock(Cell<Option<u64>>);
        impl DispatchClock for MutableClock {
            fn now_unix_seconds(&self) -> Option<u64> {
                self.0.get()
            }
        }
        struct Expire<'a>(&'a MutableClock);
        impl DispatchAdmission for Expire<'_> {
            fn authorize_dispatch(&self, _: &TaskView, _: &DispatchIntent) -> Result<(), String> {
                self.0.0.set(Some(40));
                Ok(())
            }
        }
        let TestFixture {
            temp,
            request,
            plan,
            mut task_loop,
            store,
            marker,
            ..
        } = fixture();
        task_loop
            .admit(request.clone(), plan, &Permit)
            .expect("admission");
        assert!(matches!(
            task_loop
                .dispatch(&request.run_id, &FixedClock(None), &Permit)
                .expect("unavailable clock"),
            TaskDispatchStatus::Held {
                runtime_attempted: false,
                ..
            }
        ));
        assert_eq!(task_loop.runs[&request.run_id].phase, Phase::Queued);
        let clock = MutableClock(Cell::new(Some(10)));
        assert!(matches!(
            task_loop
                .dispatch(&request.run_id, &clock, &Expire(&clock))
                .expect("expires while dispatch policy runs"),
            TaskDispatchStatus::Held {
                runtime_attempted: false,
                ..
            }
        ));
        assert_eq!(task_loop.runs[&request.run_id].phase, Phase::DispatchHeld);
        assert!(
            store
                .claim_outbox_key(Some(100), "no-retry".into(), 30, launch_key(&request))
                .expect("expired work held")
                .is_none()
        );
        assert!(!marker.exists());
        drop(task_loop);
        drop(store);
        fs::remove_dir_all(temp).expect("owned cleanup");
    }

    #[test]
    fn completion_backpressure_never_calls_one_shot_custody_transfer() {
        // Component-only generic owned token. It cannot construct ClosedAdmission
        // or ClosedExecution; actual Linux proof production remains unqualified.
        let calls = Cell::new(0);
        let mut buffer = BTreeMap::from([("first".into(), Box::new(17u32))]);
        let mut original = Some(Box::new(29u32));
        let result = buffer_completion(&mut buffer, 1, "second", || {
            calls.set(calls.get() + 1);
            Ok::<_, ()>(original.take())
        })
        .expect("bounded check");
        assert!(matches!(result, BufferOutcome::Full));
        assert_eq!(calls.get(), 0);
        assert_eq!(original.as_deref(), Some(&29));
        let first = buffer.remove("first").expect("receiver takes first");
        assert_eq!(*first, 17);
        assert!(buffer_completion::<Box<u32>, ()>(&mut buffer, 1, "second", || Err(())).is_err());
        assert!(buffer.is_empty());
        assert_eq!(
            original.as_deref(),
            Some(&29),
            "failed custody lookup does not consume the original"
        );
        assert!(matches!(
            buffer_completion(&mut buffer, 1, "second", || {
                calls.set(calls.get() + 1);
                Ok::<_, ()>(original.take())
            })
            .expect("handoff ready"),
            BufferOutcome::Buffered
        ));
        assert!(original.is_none());
        assert_eq!(calls.get(), 1);
        assert_eq!(buffer.get("second").map(|n| **n), Some(29));
        assert!(matches!(
            buffer_completion::<Box<u32>, ()>(&mut buffer, 1, "second", || panic!(
                "duplicate must not retake custody"
            ))
            .unwrap(),
            BufferOutcome::Buffered
        ));
        assert!(
            take_confirmed_completion(&mut buffer, "second", EffectAckState::Unconfirmed).is_none()
        );
        assert_eq!(
            buffer.get("second").map(|n| **n),
            Some(29),
            "unknown effect keeps the actual buffered ownership"
        );
        let transferred =
            take_confirmed_completion(&mut buffer, "second", EffectAckState::Recorded)
                .expect("confirmed ACK permits one-shot custody transfer");
        assert_eq!(*transferred, 29);
        assert!(buffer.is_empty());
        assert!(
            take_confirmed_completion(&mut buffer, "second", EffectAckState::Recorded).is_none()
        );
    }

    #[test]
    fn lease_bounds_and_retained_admission_capacity_reject_before_losing_plan() {
        let TestFixture {
            temp,
            request,
            plan,
            mut task_loop,
            store,
            journal,
            ..
        } = fixture();
        for (deadline, lease) in [
            (Duration::from_secs(30), 30),
            (Duration::from_millis(30_001), 31),
            (Duration::MAX, u64::MAX),
            (Duration::from_secs(1), MAX_LEASE_SECONDS + 1),
        ] {
            let config = NativeTaskLoopConfig {
                scope: task_loop.config.scope.clone(),
                task_owner: request.owner.clone(),
                dispatch_owner: "dispatch".into(),
                dispatch_lease_secs: lease,
                max_retained_runs: 2,
                max_buffered_completions: 1,
            };
            assert!(matches!(
                NativeTaskLoop::new(
                    store.clone(),
                    ResourceGate::new(task_loop.config.scope.node_id.clone()),
                    config,
                    NativeTaskRuntimeConfig {
                        journal_root: journal.clone(),
                        deadline,
                        capture_limit: 1024
                    }
                ),
                Err(LoopError::InvalidConfiguration)
            ));
        }
        task_loop.config.max_retained_runs = 1;
        task_loop
            .admit(request.clone(), plan, &Permit)
            .expect("one actual retained plan");
        let second = fixture();
        let head = second.plan.prepared_workspace.head_oid().to_owned();
        let rejected = task_loop
            .admit(second.request, second.plan, &Permit)
            .expect_err("no free retained slot");
        assert_eq!(rejected.reason, LoopError::Capacity);
        assert_eq!(rejected.plan.prepared_workspace.head_oid(), head);
        drop(rejected);
        drop(second.task_loop);
        drop(second.store);
        fs::remove_dir_all(second.temp).expect("second owned cleanup");
        drop(task_loop);
        drop(store);
        fs::remove_dir_all(temp).expect("owned cleanup");
    }
}
