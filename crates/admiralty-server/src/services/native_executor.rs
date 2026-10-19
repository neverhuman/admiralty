//! Concrete native execution adapter over the configured resource gate.
//!
//! Plans and permit authorities enter only through trusted process-local
//! startup code. This module accepts no execution grants from JSON or worker
//! payloads. A successful child produces a candidate receipt only; acceptance
//! and trusted checks remain separate lifecycle decisions.

use super::controller::{
    DispatchIntent, NativeRuntimeExecutor, RuntimeDisposition, RuntimeReceipt,
};
use admiralty_core::TaskView;
use admiralty_core::{AttemptId, SchedulingState, TaskId, content_digest};
use admiralty_runtime::{
    reservations::Reservation,
    resource_gate::{
        AdmissionIdentity, ClosedAdmission, RecoveryIdentity, ResourceGate,
        ResourcePermitAuthority, ResourcePermitClaims, ResourceUsageDiagnostics, RunningAdmission,
        VerifiedResourcePermit,
    },
    session::{CaptureState, LaunchSpec, OutputChunk, SessionHost, SessionStatus},
    workspace::{preparation::PreparedWorkspace, registry::WorkspaceSpec},
};
use serde::Serialize;
use sha2::{Digest as _, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{Read, Write},
    os::unix::fs::MetadataExt,
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

const MAX_RUN_ID_BYTES: usize = 128;
const MAX_COMPLETED_CAPTURE_CHUNKS: usize = 16_384;
const RESOURCE_SAMPLE_INTERVAL: Duration = Duration::from_millis(250);

/// Trusted service-local persistence boundary. Called only with an actual
/// resource observation whose recovery identity was durably journaled first.
/// Implementations must not re-enter the executor or launch/authorize work.
/// An error is an unknown persistence outcome and must be safe to retry exactly.
pub trait StartedSink: Send + Sync {
    fn record_started(
        &self,
        intent: &DispatchIntent,
        recovery: &RecoveryIdentity,
    ) -> Result<(), StartedSinkError>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartedSinkError {
    InvalidBinding,
    Capacity,
    Unavailable,
}
impl std::fmt::Display for StartedSinkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidBinding => "started observation binding differs",
            Self::Capacity => "started observation registry is full",
            Self::Unavailable => "started observation persistence is unavailable",
        })
    }
}
impl std::error::Error for StartedSinkError {}

pub struct NativeExecutionPlan {
    /// Immutable contract/work-order and provider preparation bindings.
    pub task_id: TaskId,
    pub attempt_id: AttemptId,
    pub contract_digest: String,
    pub context_digest: String,
    pub policy_digest: String,
    pub provider_declared: String,
    pub model_declared: String,
    pub run_id: String,
    pub launch_spec: LaunchSpec,
    /// Opaque evidence returned only by create-only pinned workspace preparation.
    pub prepared_workspace: PreparedWorkspace,
    pub workspace: WorkspaceSpec,
    pub reservation: Reservation,
    /// Claims alone are inert. `permit_authority` is supplied by trusted startup.
    pub permit_claims: ResourcePermitClaims,
    pub permit_authority: Arc<dyn ResourcePermitAuthority>,
}

/// Registration rejection retains the original opaque workspace capability.
/// Callers must keep or explicitly reconcile the returned plan; reconstructing
/// it from a path or JSON cannot replace the actual preparation ownership.
#[must_use = "retain or reconcile the original plan returned on registration failure"]
pub struct PlanRegistrationFailure {
    error: NativeExecutorError,
    plan: Box<NativeExecutionPlan>,
}
impl PlanRegistrationFailure {
    pub fn error(&self) -> &NativeExecutorError {
        &self.error
    }
    pub fn into_parts(self) -> (NativeExecutorError, NativeExecutionPlan) {
        (self.error, *self.plan)
    }
}
impl std::fmt::Debug for PlanRegistrationFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Do not expose launch arguments, context or policy through an error.
        f.debug_struct("PlanRegistrationFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}
impl std::fmt::Display for PlanRegistrationFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.error.fmt(f)
    }
}
impl std::error::Error for PlanRegistrationFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

pub struct NativeExecutor {
    gate: ResourceGate,
    journal: PrivateJournal,
    deadline: Duration,
    capture_limit: usize,
    max_pending: usize,
    plans: Mutex<BTreeMap<String, NativeExecutionPlan>>,
    pending: Mutex<BTreeMap<String, PendingRun>>,
    reserved_runs: Mutex<BTreeSet<String>>,
    // A write attempt is an uncertainty boundary even when no file is visible.
    // Entries are a subset of reserved_runs and retire only after proven closure.
    launch_fenced_runs: Mutex<BTreeSet<String>>,
    completed: Mutex<BTreeMap<String, ClosedExecution>>,
    started_sink: Arc<dyn StartedSink>,
}

/// Fixed, opaque artifact category returned by [`NativeExecutor::inspect_run_journal`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunJournalArtifactKind {
    Prepared,
    Observed,
    Closed,
}

/// Bounded metadata for one raw per-run journal file. The journal bytes and
/// their interpretation are deliberately not exposed by this read-only API.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunJournalArtifact {
    pub kind: RunJournalArtifactKind,
    pub bytes: u64,
    pub sha256: String,
}

struct PendingRun {
    host: SessionHost,
    running: RunningAdmission,
    prepared_workspace: PreparedWorkspace,
    task_id: TaskId,
    attempt_id: AttemptId,
    intent: DispatchIntent,
    recovery: RecoveryIdentity,
    observed_persisted: bool,
    started_persisted: bool,
    refresh_attempts: u32,
    refresh_failed: bool,
    last_resource_sample: Option<Instant>,
}

/// Opaque, bounded, process-local evidence. Only actual owned service closure
/// can create this value. Handoff is one-shot and does not authorize acceptance.
pub struct ClosedExecution {
    receipt: RuntimeReceipt,
    closure: ClosedAdmission,
    prepared_workspace: PreparedWorkspace,
    output: Vec<u8>,
    capture: Option<CompletedCapture>,
}

/// Actual bounded session output, retaining its original stream tags and byte
/// offsets through the producer's final EOF watermark. Only the owned host's
/// complete, exited capture can create this process-local value; no serialized
/// status or caller-supplied chunks can reconstruct it. It does not assert a
/// successful exit, provider identity, changed source, or acceptance.
pub struct CompletedCapture {
    run_id: String,
    chunks: Vec<OutputChunk>,
    final_status: SessionStatus,
}
impl CompletedCapture {
    /// Owned SessionHost run that produced these chunks. Callers must bind it
    /// to the accompanying completion receipt and prepared provider launch.
    pub fn run_id(&self) -> &str {
        &self.run_id
    }
    pub fn chunks(&self) -> &[OutputChunk] {
        &self.chunks
    }
    pub fn final_status(&self) -> &SessionStatus {
        &self.final_status
    }
}
impl ClosedExecution {
    pub fn receipt(&self) -> &RuntimeReceipt {
        &self.receipt
    }
    pub fn closure(&self) -> &ClosedAdmission {
        &self.closure
    }
    /// Original descriptor-backed preparation capability, retained through
    /// uncertain execution and actual service closure. Paths cannot recreate it.
    pub fn prepared_workspace(&self) -> &PreparedWorkspace {
        &self.prepared_workspace
    }
    /// Consume the one-shot handoff for trusted source-candidate composition.
    /// The receiver must validate the original preparation and closure together;
    /// neither transcript bytes nor this receipt establish changed source.
    pub fn into_completion_parts(
        self,
    ) -> (RuntimeReceipt, PreparedWorkspace, ClosedAdmission, Vec<u8>) {
        (
            self.receipt,
            self.prepared_workspace,
            self.closure,
            self.output,
        )
    }
    /// Captured transcript bytes; this is NOT a source/code candidate artifact.
    pub fn output(&self) -> &[u8] {
        &self.output
    }
    /// Nonempty authenticated transcript with the actual final host status.
    /// `None` is reserved for a silent completed child; it is not a successful
    /// empty provider result. Incomplete or uncertain captures remain pending.
    pub fn capture(&self) -> Option<&CompletedCapture> {
        self.capture.as_ref()
    }
    /// Transfer the original tagged capture alongside the existing one-shot
    /// workspace/closure handoff. Flattened bytes retain compatibility only;
    /// provider parsers must consume the tagged capture and its actual status.
    pub fn into_completion_with_capture(
        self,
    ) -> (
        RuntimeReceipt,
        PreparedWorkspace,
        ClosedAdmission,
        Vec<u8>,
        Option<CompletedCapture>,
    ) {
        (
            self.receipt,
            self.prepared_workspace,
            self.closure,
            self.output,
            self.capture,
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingDiagnostics {
    pub refresh_attempts: u32,
    pub refresh_failed: bool,
    pub observed_identity_persisted: bool,
    pub started_persisted: bool,
    pub resource_usage: ResourceUsageDiagnostics,
}

#[derive(Serialize)]
struct ClosedEvidence<'a, C> {
    schema_version: u32,
    recovery: &'a RecoveryIdentity,
    closure: C,
    closure_digest: &'a str,
    exit_code: Option<i32>,
    success: bool,
    output_digest: &'a str,
    output_bytes: u64,
    provider_declared: &'a str,
    model_declared: &'a str,
    provider_observed: Option<&'a str>,
    model_observed: Option<&'a str>,
}

/// V2 full-journal identity includes advisory observations. The legacy authority
/// digest covers only the original v1 binding, never these resource counters.
#[derive(Serialize)]
struct ClosedEvidenceJournal<'a, C> {
    schema_version: u32,
    authority_evidence: C,
    authority_digest: String,
    resource_usage_advisory: &'a ResourceUsageDiagnostics,
}

fn closed_evidence_journal<C: Serialize>(
    evidence: C,
    diagnostics: &ResourceUsageDiagnostics,
) -> Result<ClosedEvidenceJournal<'_, C>, NativeExecutorError> {
    let authority_digest = content_digest(&evidence).map_err(|_| NativeExecutorError::Digest)?;
    Ok(ClosedEvidenceJournal {
        schema_version: 2,
        authority_evidence: evidence,
        authority_digest,
        resource_usage_advisory: diagnostics,
    })
}

impl NativeExecutor {
    /// Create the production adapter during trusted startup. `journal_root`
    /// must already exist as a private mode-0700 directory owned by this user.
    pub fn new(
        gate: ResourceGate,
        journal_root: impl AsRef<Path>,
        deadline: Duration,
        capture_limit: usize,
        max_pending: usize,
        started_sink: Arc<dyn StartedSink>,
    ) -> Result<Self, NativeExecutorError> {
        let journal = PrivateJournal::open(journal_root.as_ref())?;
        if deadline.is_zero()
            || deadline > Duration::from_secs(3600)
            || capture_limit == 0
            || capture_limit > 16 * 1024 * 1024
            || max_pending == 0
            || max_pending > 1024
        {
            return Err(NativeExecutorError::InvalidConfiguration);
        }
        Ok(Self {
            gate,
            journal,
            deadline,
            capture_limit,
            max_pending,
            plans: Mutex::new(BTreeMap::new()),
            pending: Mutex::new(BTreeMap::new()),
            reserved_runs: Mutex::new(BTreeSet::new()),
            launch_fenced_runs: Mutex::new(BTreeSet::new()),
            completed: Mutex::new(BTreeMap::new()),
            started_sink,
        })
    }

    /// Inspect the fixed journal artifacts for a run without parsing or
    /// changing them. This reports bytes and raw SHA-256 only; it does not
    /// validate evidence, authorize recovery, or reconstruct capabilities.
    pub fn inspect_run_journal(
        &self,
        run_id: &str,
    ) -> Result<Vec<RunJournalArtifact>, NativeExecutorError> {
        if !valid_run_id(run_id) {
            return Err(NativeExecutorError::InvalidBinding);
        }
        self.journal.inspect_run(run_id)
    }

    /// Return the original preparation capability only before launch has taken
    /// the plan. Never erase journals or withdraw a pending/closed execution.
    /// Holding `plans` excludes launch's ownership transfer throughout the check.
    pub fn withdraw_unstarted_plan(
        &self,
        run_id: &str,
    ) -> Result<Option<NativeExecutionPlan>, NativeExecutorError> {
        if !valid_run_id(run_id) {
            return Err(NativeExecutorError::InvalidBinding);
        }
        let mut plans = self
            .plans
            .lock()
            .map_err(|_| NativeExecutorError::Poisoned)?;
        if self
            .launch_fenced_runs
            .lock()
            .map_err(|_| NativeExecutorError::Poisoned)?
            .contains(run_id)
        {
            return Err(NativeExecutorError::LaunchUnknown);
        }
        if !plans.contains_key(run_id) {
            return Ok(None);
        }
        for kind in ["prepared", "observed", "closed"] {
            if self.journal.exists(&journal_name(run_id, kind)?)? {
                return Err(NativeExecutorError::LaunchUnknown);
            }
        }
        let mut reserved = self
            .reserved_runs
            .lock()
            .map_err(|_| NativeExecutorError::Poisoned)?;
        if !reserved.contains(run_id) {
            return Err(NativeExecutorError::LaunchUnknown);
        }
        // No fallible operations after removing the actual owned plan.
        let result = plans.remove(run_id);
        reserved.remove(run_id);
        Ok(result)
    }

    /// Compatibility API: a rejected plan is consumed and its preparation
    /// capability is dropped. Ownership-aware coordinators must instead use
    /// `try_register_plan` and retain its returned plan on every error.
    pub fn register_plan(&self, plan: NativeExecutionPlan) -> Result<(), NativeExecutorError> {
        self.try_register_plan(plan).map_err(|failure| {
            let (error, discarded_plan) = failure.into_parts();
            drop(discarded_plan);
            error
        })
    }

    /// Install a trusted plan without losing it on rejection. All validation,
    /// journal and registry checks borrow the plan; its original preparation
    /// moves into the registry only after every fallible operation succeeds.
    pub fn try_register_plan(
        &self,
        plan: NativeExecutionPlan,
    ) -> Result<(), PlanRegistrationFailure> {
        let checked = (|| {
            if !valid_run_id(&plan.run_id) || plan.launch_spec.run_id != plan.run_id {
                return Err(NativeExecutorError::InvalidBinding);
            }
            let plans = self
                .plans
                .lock()
                .map_err(|_| NativeExecutorError::Poisoned)?;
            let reserved = self
                .reserved_runs
                .lock()
                .map_err(|_| NativeExecutorError::Poisoned)?;
            if plans.contains_key(&plan.run_id)
                || reserved.contains(&plan.run_id)
                || self
                    .launch_fenced_runs
                    .lock()
                    .map_err(|_| NativeExecutorError::Poisoned)?
                    .contains(&plan.run_id)
            {
                return Err(NativeExecutorError::DuplicateRun);
            }
            for kind in ["prepared", "observed", "closed"] {
                if self.journal.exists(&journal_name(&plan.run_id, kind)?)? {
                    return Err(NativeExecutorError::DuplicateRun);
                }
            }
            if reserved.len() >= self.max_pending {
                return Err(NativeExecutorError::Capacity);
            }
            Ok((plans, reserved))
        })();
        let (mut plans, mut reserved) = match checked {
            Ok(guards) => guards,
            Err(error) => {
                return Err(PlanRegistrationFailure {
                    error,
                    plan: Box::new(plan),
                });
            }
        };
        // No fallible operations after consuming the original plan.
        reserved.insert(plan.run_id.clone());
        plans.insert(plan.run_id.clone(), plan);
        Ok(())
    }

    /// Transfer the real opaque closure to trusted verification composition.
    /// Exact receipt equality is required; JSON evidence cannot reconstruct it.
    /// The receiver must keep it alive until lifecycle authorization completes.
    pub fn take_closed(
        &self,
        receipt: &RuntimeReceipt,
    ) -> Result<Option<ClosedExecution>, NativeExecutorError> {
        let mut completed = self
            .completed
            .lock()
            .map_err(|_| NativeExecutorError::Poisoned)?;
        let Some(found) = completed.get(&receipt.run_id) else {
            return Ok(None);
        };
        if &found.receipt != receipt {
            return Err(NativeExecutorError::InvalidBinding);
        }
        let result = completed.remove(&receipt.run_id);
        self.reserved_runs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&receipt.run_id);
        self.launch_fenced_runs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&receipt.run_id);
        Ok(result)
    }
    pub fn pending_diagnostics(&self, run_id: &str) -> Option<PendingDiagnostics> {
        self.pending
            .lock()
            .ok()?
            .get(run_id)
            .map(|entry| PendingDiagnostics {
                refresh_attempts: entry.refresh_attempts,
                refresh_failed: entry.refresh_failed,
                observed_identity_persisted: entry.observed_persisted,
                started_persisted: entry.started_persisted,
                resource_usage: entry
                    .running
                    .observation_diagnostics()
                    .resource_usage
                    .clone(),
            })
    }
    /// Reconcile without launching or signalling. Unknown runs retain their own
    /// SessionHost and resource handle; completed proofs remain bounded until
    /// trusted composition takes them. There is no restart/relaunch fallback.
    pub fn observe_pending(
        &self,
        run_id: &str,
    ) -> Result<Option<RuntimeReceipt>, NativeExecutorError> {
        if let Some(done) = self
            .completed
            .lock()
            .map_err(|_| NativeExecutorError::Poisoned)?
            .get(run_id)
        {
            return Ok(Some(done.receipt.clone()));
        }
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(entry) = pending.get_mut(run_id) else {
            return Ok(None);
        };
        // Observation must happen while the service still exists; waiting for
        // exit first makes an initially uncertain handle permanently unprovable.
        if entry.running.observation().is_none() {
            entry.refresh_attempts = entry.refresh_attempts.saturating_add(1);
            entry.refresh_failed = entry.running.refresh_observation().is_err();
        }
        // This bounded advisory read cannot admit, signal, release or qualify
        // anything. Keep samples while the owned cgroup still exists; a missing
        // final sample preserves the last known counters without inventing zero.
        let now = Instant::now();
        if entry.running.observation().is_some()
            && entry
                .last_resource_sample
                .is_none_or(|last| now.saturating_duration_since(last) >= RESOURCE_SAMPLE_INTERVAL)
        {
            entry.running.refresh_resource_diagnostics();
            entry.last_resource_sample = Some(now);
        }
        if entry.running.observation().is_some() && !entry.observed_persisted {
            entry.recovery = entry.running.recovery_identity().clone();
            self.journal
                .write_same_or_new(&journal_name(run_id, "observed")?, &entry.recovery)?;
            entry.observed_persisted = true;
        }
        persist_started_once(
            self.started_sink.as_ref(),
            &entry.intent,
            &entry.recovery,
            entry.observed_persisted && entry.running.observation().is_some(),
            &mut entry.started_persisted,
        )?;
        // Without an actual persisted observation there can be no start or
        // completion handoff. Keep every original handle for reconciliation.
        if !entry.started_persisted {
            return Ok(None);
        }
        let status = match entry.host.status(run_id) {
            Ok(status) => status,
            Err(_) => return Ok(None),
        };
        if entry.observed_persisted
            && !status.input_closed
            && entry.host.close_input(run_id).is_err()
        {
            return Ok(None);
        }
        if status.exit.is_none() || status.capture_state == CaptureState::Capturing {
            return Ok(None);
        }
        if status.capture_failed
            || status.capture_state != CaptureState::Complete
            || status
                .final_output_offset
                .is_none_or(|n| n > self.capture_limit as u64 || n != status.output_offset)
        {
            return Ok(None);
        }
        let collected = match collect_output(&entry.host, run_id, &status, self.capture_limit) {
            Ok(captured) => captured,
            Err(_) => return Ok(None),
        };
        let closure = match prove_closed(&mut entry.running, &entry.host) {
            Ok(proof) => proof,
            Err(_) => return Ok(None),
        };
        let output_digest =
            content_digest(&collected.output).map_err(|_| NativeExecutorError::Digest)?;
        let outcome = write_closed_evidence(
            &self.journal,
            &entry.recovery,
            &closure,
            &output_digest,
            collected.output.len() as u64,
            &entry.intent.provider,
            &entry.intent.model,
        )?;
        let receipt = RuntimeReceipt {
            task_id: entry.task_id.clone(),
            attempt_id: entry.attempt_id.clone(),
            run_id: entry.intent.run_id.clone(),
            receipt_ref: outcome.0,
            disposition: if outcome.1 {
                RuntimeDisposition::Candidate {
                    candidate_digest: output_digest,
                }
            } else {
                RuntimeDisposition::Failed {
                    evidence_ref: output_digest,
                }
            },
        };
        let mut completed = self
            .completed
            .lock()
            .map_err(|_| NativeExecutorError::Poisoned)?;
        if completed.len() >= self.max_pending {
            return Err(NativeExecutorError::Capacity);
        }
        // All fallible observation, durable evidence and capacity checks finish
        // before removing the pending owner. Unknown/error paths keep its
        // SessionHost, resource admission and original workspace capability.
        let closed_run = pending
            .remove(run_id)
            .ok_or(NativeExecutorError::LaunchUnknown)?;
        completed.insert(
            run_id.into(),
            ClosedExecution {
                receipt: receipt.clone(),
                closure,
                prepared_workspace: closed_run.prepared_workspace,
                output: collected.output,
                capture: collected.capture,
            },
        );
        // Dropping this run's SessionHost is safe only after proven closure.
        Ok(Some(receipt))
    }

    fn ensure_no_launch_evidence(&self, run_id: &str) -> Result<(), NativeExecutorError> {
        if !valid_run_id(run_id) {
            return Err(NativeExecutorError::InvalidBinding);
        }
        if self
            .launch_fenced_runs
            .lock()
            .map_err(|_| NativeExecutorError::Poisoned)?
            .contains(run_id)
        {
            return Err(NativeExecutorError::LaunchUnknown);
        }
        for kind in ["prepared", "observed", "closed"] {
            if self.journal.exists(&journal_name(run_id, kind)?)? {
                return Err(NativeExecutorError::LaunchUnknown);
            }
        }
        Ok(())
    }

    fn persist_prelaunch(
        &self,
        recovery: &RecoveryIdentity,
        intent: &DispatchIntent,
        plan: &NativeExecutionPlan,
    ) -> Result<(), NativeExecutorError> {
        // Establish a bounded process-local fence before the first journal
        // write. A failed create/write/sync cannot become an automatic retry
        // merely because no durable file can currently be observed.
        let reserved = self
            .reserved_runs
            .lock()
            .map_err(|_| NativeExecutorError::Poisoned)?;
        if !reserved.contains(&plan.run_id)
            || recovery.run_id != plan.run_id
            || intent.run_id != plan.run_id
        {
            return Err(NativeExecutorError::LaunchUnknown);
        }
        let mut fenced = self
            .launch_fenced_runs
            .lock()
            .map_err(|_| NativeExecutorError::Poisoned)?;
        if fenced.contains(&plan.run_id) {
            return Err(NativeExecutorError::LaunchUnknown);
        }
        if fenced.len() >= self.max_pending {
            return Err(NativeExecutorError::Capacity);
        }
        fenced.insert(plan.run_id.clone());
        drop(fenced);
        drop(reserved);
        write_prelaunch_recovery(&self.journal, recovery, intent, plan)
    }

    fn launch_plan(
        &self,
        task: &TaskView,
        intent: &DispatchIntent,
    ) -> Result<RuntimeReceipt, NativeExecutorError> {
        let mut plans = self
            .plans
            .lock()
            .map_err(|_| NativeExecutorError::Poisoned)?;
        self.ensure_no_launch_evidence(&intent.run_id)?;
        let original = plans
            .remove(&intent.run_id)
            .ok_or(NativeExecutorError::NoPreparedPlan)?;
        drop(plans);
        let mut plan = Some(original);
        let result = self.launch_registered_plan(task, intent, &mut plan);
        if let Some(plan) = plan {
            // The run reservation stays held while its plan is in flight, so
            // registration cannot insert a replacement and withdrawal cannot
            // remove it. Restore the actual capability on every known
            // pre-spawn failure, including an uncertain journal write.
            self.plans
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(intent.run_id.clone(), plan);
        }
        // A reservation retires only on safe prelaunch withdrawal or one-shot
        // proven closure handoff, never merely because launch returned an error.
        result
    }

    fn launch_registered_plan(
        &self,
        task: &TaskView,
        intent: &DispatchIntent,
        retained: &mut Option<NativeExecutionPlan>,
    ) -> Result<RuntimeReceipt, NativeExecutorError> {
        let plan = retained
            .as_ref()
            .ok_or(NativeExecutorError::NoPreparedPlan)?;
        validate_bindings(task, intent, plan)?;
        let permit = VerifiedResourcePermit::verify_current(
            plan.permit_claims.clone(),
            plan.permit_authority.clone(),
        )
        .map_err(|_| NativeExecutorError::PermitDenied)?;
        let admitted = self
            .gate
            .prepare(
                permit,
                plan.permit_claims.identity.clone(),
                plan.launch_spec.clone(),
            )
            .map_err(|_| NativeExecutorError::PermitDenied)?;
        let recovery = admitted.recovery_identity();
        self.persist_prelaunch(&recovery, intent, plan)?;
        // Move the plan only after all earlier fallible checks. If the gate
        // positively reports a pre-spawn failure, restore this exact value;
        // an actual/ambiguous spawn instead transfers its workspace to pending.
        let original = retained.take().ok_or(NativeExecutorError::NoPreparedPlan)?;
        let host = SessionHost::new();
        let running = match admitted.launch(&host, self.deadline) {
            Ok(running) => running,
            Err(failure) => {
                if let Some(running) = failure.recovery {
                    let recovery = running.recovery_identity().clone();
                    self.insert_pending(
                        intent,
                        host,
                        *running,
                        recovery,
                        original.prepared_workspace,
                    )?;
                } else {
                    *retained = Some(original);
                }
                return Err(NativeExecutorError::LaunchUnknown);
            }
        };
        let recovery = running.recovery_identity().clone();
        self.insert_pending(intent, host, running, recovery, original.prepared_workspace)?;
        // Noninteractive executor: no hidden open pipe can block provider EOF.
        // Interactive sessions require a separately authorized future interface.
        {
            let pending = self
                .pending
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let entry = pending
                .get(&intent.run_id)
                .ok_or(NativeExecutorError::LaunchUnknown)?;
            entry
                .host
                .close_input(&intent.run_id)
                .map_err(|_| NativeExecutorError::LaunchUnknown)?;
        }
        let deadline = Instant::now()
            .checked_add(self.deadline)
            .ok_or(NativeExecutorError::InvalidConfiguration)?;
        loop {
            if let Some(receipt) = self.observe_pending(&intent.run_id)? {
                return Ok(receipt);
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(NativeExecutorError::DeadlineUnknown);
            }
            thread::sleep(Duration::from_millis(10).min(deadline.saturating_duration_since(now)));
        }
    }

    fn insert_pending(
        &self,
        intent: &DispatchIntent,
        host: SessionHost,
        running: RunningAdmission,
        recovery: RecoveryIdentity,
        prepared_workspace: PreparedWorkspace,
    ) -> Result<(), NativeExecutorError> {
        // These identities were validated before launch. Never drop the host or
        // admission handle on a post-spawn bookkeeping failure.
        let attempt_id = running.recovery_identity().identity.attempt_id.clone();
        let task_id = prepared_workspace.task_id().clone();
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let reserved = self
            .reserved_runs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(&intent.run_id);
        let collision = pending.contains_key(&intent.run_id) || !reserved;
        let key = if collision {
            let key = format!("quarantine-{}", running.recovery_identity().unit);
            self.reserved_runs
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(key.clone());
            key
        } else {
            intent.run_id.clone()
        };
        let observed = running.observation().is_some();
        pending.insert(
            key.clone(),
            PendingRun {
                host,
                running,
                prepared_workspace,
                task_id,
                attempt_id,
                intent: intent.clone(),
                recovery,
                observed_persisted: false,
                started_persisted: false,
                refresh_attempts: 0,
                refresh_failed: false,
                last_resource_sample: None,
            },
        );
        let entry = pending
            .get_mut(&key)
            .ok_or(NativeExecutorError::LaunchUnknown)?;
        if observed {
            self.journal
                .write_same_or_new(&journal_name(&intent.run_id, "observed")?, &entry.recovery)?;
            entry.observed_persisted = true;
        }
        if collision {
            return Err(NativeExecutorError::LaunchUnknown);
        }
        Ok(())
    }
    /// Bounded operator inventory for retained unknown handles, including an
    /// internal bookkeeping quarantine. This never reconstructs or relaunches.
    pub fn pending_run_ids(&self) -> Vec<String> {
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .keys()
            .cloned()
            .collect()
    }
}

impl NativeRuntimeExecutor for NativeExecutor {
    fn execute(&self, task: &TaskView, intent: &DispatchIntent) -> Result<RuntimeReceipt, String> {
        self.launch_plan(task, intent)
            .map_err(|error| error.to_string())
    }
}

#[derive(Debug)]
pub enum NativeExecutorError {
    InvalidConfiguration,
    UnsupportedBackend,
    InvalidBinding,
    DuplicateRun,
    NoPreparedPlan,
    PermitDenied,
    LaunchUnknown,
    DeadlineUnknown,
    Capacity,
    Poisoned,
    Journal,
    Digest,
    StartedPersistence,
}

impl std::fmt::Display for NativeExecutorError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidConfiguration => "native executor configuration is outside bounds",
            Self::UnsupportedBackend => "actual native service closure requires Linux",
            Self::InvalidBinding => "native task identity or trusted plan binding differs",
            Self::DuplicateRun => "run identity is already prepared or executing",
            Self::NoPreparedPlan => "no process-local prepared native plan exists for this run",
            Self::PermitDenied => "resource permit is not currently authorized",
            Self::LaunchUnknown => {
                "resource-gated launch outcome is ambiguous; recovery handle retained"
            }
            Self::DeadlineUnknown => "execution deadline elapsed; recovery handle retained",
            Self::Capacity => "native execution capacity is exhausted",
            Self::Poisoned => "native executor registry lock is poisoned",
            Self::Journal => "private recovery evidence could not be durably written",
            Self::Digest => "native output or evidence digest could not be produced",
            Self::StartedPersistence => {
                "started persistence outcome is unknown; recovery handle retained"
            }
        })
    }
}

impl std::error::Error for NativeExecutorError {}

fn persist_started_once(
    sink: &dyn StartedSink,
    intent: &DispatchIntent,
    recovery: &RecoveryIdentity,
    observed_persisted: bool,
    started_persisted: &mut bool,
) -> Result<(), NativeExecutorError> {
    if !*started_persisted && observed_persisted {
        sink.record_started(intent, recovery)
            .map_err(|_| NativeExecutorError::StartedPersistence)?;
        *started_persisted = true;
    }
    Ok(())
}

fn validate_bindings(
    task: &TaskView,
    intent: &DispatchIntent,
    plan: &NativeExecutionPlan,
) -> Result<(), NativeExecutorError> {
    task.contract
        .validate()
        .map_err(|_| NativeExecutorError::InvalidBinding)?;
    let digest = content_digest(&task.contract).map_err(|_| NativeExecutorError::Digest)?;
    let validated_workspace = plan
        .workspace
        .validate()
        .map_err(|_| NativeExecutorError::InvalidBinding)?;
    let prepared = plan.prepared_workspace.workspace();
    let contract_owned: Vec<PathBuf> = task
        .contract
        .owned_paths
        .iter()
        .map(PathBuf::from)
        .collect();
    let contract_forbidden: Vec<PathBuf> = task
        .contract
        .forbidden_paths
        .iter()
        .map(PathBuf::from)
        .collect();
    let identity: &AdmissionIdentity = &plan.permit_claims.identity;
    let reservation = &plan.reservation;
    if task.contract.simulated
        || intent.schema_version != 1
        || task.state != SchedulingState::Claimed
        || task
            .attempt_id
            .as_ref()
            .is_none_or(|id| id.as_str() != intent.attempt_id)
        || task
            .owner
            .as_ref()
            .is_none_or(|owner| owner.as_str() != intent.owner)
        || task.contract.task_id != plan.task_id
        || task.contract.task_id.as_str() != intent.task_id
        || plan.task_id
            != TaskId::new(intent.task_id.clone())
                .map_err(|_| NativeExecutorError::InvalidBinding)?
        || plan.attempt_id
            != AttemptId::new(intent.attempt_id.clone())
                .map_err(|_| NativeExecutorError::InvalidBinding)?
        || plan.run_id != intent.run_id
        || plan.launch_spec.run_id != intent.run_id
        || plan.contract_digest != intent.contract_digest
        || digest != intent.contract_digest
        || plan.context_digest != intent.context_digest
        || plan.policy_digest != intent.policy_digest
        || plan.provider_declared != intent.provider
        || plan.model_declared != intent.model
        || task.contract.revision != intent.contract_revision
        || task.contract.project_id != identity.project_id
        || plan.workspace.node_id.as_str() != identity.node_id.as_str()
        || identity.task_id != plan.task_id
        || identity.attempt_id != plan.attempt_id
        || plan.permit_claims.reservation_ref != reservation.id
        || plan.permit_claims.identity.node_id.as_str() != plan.workspace.node_id.as_str()
        || reservation.scope.project_id != identity.project_id
        || reservation.scope.root_id != task.contract.root_id
        || reservation.scope.task_id != plan.task_id
        || reservation.scope.attempt_id != plan.attempt_id
        || reservation.scope.owner.as_str() != intent.owner
        || reservation.scope.contract_digest != intent.contract_digest
        || reservation.scope.contract_revision != intent.contract_revision
        || reservation.scope.authority_ref != task.contract.authority_ref
        || reservation.scope.incarnation != plan.permit_claims.host_incarnation
        || reservation.scope.resource_claims_digest
            != native_resource_claims_digest(&plan.permit_claims)?
        || !reservation.outstanding
        || reservation.stop_requested
        || plan.workspace.project_id != task.contract.project_id
        || plan.workspace.root_id != task.contract.root_id
        || plan.workspace.workspace != plan.launch_spec.cwd
        || plan.prepared_workspace.task_id() != &plan.task_id
        || plan.prepared_workspace.attempt_id() != &plan.attempt_id
        || prepared.project_id != task.contract.project_id
        || prepared.root_id != task.contract.root_id
        || prepared.workspace != plan.launch_spec.cwd
        || prepared.node_id.as_str() != plan.workspace.node_id.as_str()
        || prepared.owned_paths != plan.workspace.owned_paths
        || prepared.forbidden_paths != plan.workspace.forbidden_paths
        || contract_owned != prepared.owned_paths
        || contract_forbidden != prepared.forbidden_paths
        || validated_workspace.project_id != task.contract.project_id
        || validated_workspace.root_id != task.contract.root_id
        || validated_workspace.workspace != plan.launch_spec.cwd
    {
        return Err(NativeExecutorError::InvalidBinding);
    }
    Ok(())
}

struct CollectedOutput {
    output: Vec<u8>,
    capture: Option<CompletedCapture>,
}

fn collect_output(
    host: &SessionHost,
    run_id: &str,
    observed: &SessionStatus,
    maximum: usize,
) -> Result<CollectedOutput, NativeExecutorError> {
    let output_len = observed.output_offset;
    if output_len > maximum as u64 {
        return Err(NativeExecutorError::Capacity);
    }
    if observed.exit.is_none()
        || observed.capture_failed
        || observed.capture_state != CaptureState::Complete
        || observed.final_output_offset != Some(output_len)
        || observed.input_outcome_unknown
        || observed.pid == 0
    {
        return Err(NativeExecutorError::LaunchUnknown);
    }
    let mut output = Vec::with_capacity(output_len as usize);
    let mut chunks = Vec::new();
    let mut cursor = 0;
    if output_len == 0 {
        // Silent compatibility completions still validate the actual journal;
        // a corrupt empty source must not bypass capture checks via `None`.
        let tail = host
            .tail(run_id, 0, 1)
            .map_err(|_| NativeExecutorError::Journal)?;
        if tail.next_offset != 0 || !tail.chunks.is_empty() {
            return Err(NativeExecutorError::Journal);
        }
    }
    while cursor < output_len {
        let tail = host
            .tail(run_id, cursor, (maximum - cursor as usize).min(64 * 1024))
            .map_err(|_| NativeExecutorError::Journal)?;
        if tail.next_offset <= cursor || tail.next_offset > output_len {
            return Err(NativeExecutorError::Journal);
        }
        let mut chunk_cursor = cursor;
        for chunk in tail.chunks {
            if chunks.len() >= MAX_COMPLETED_CAPTURE_CHUNKS {
                return Err(NativeExecutorError::Capacity);
            }
            let next = chunk_cursor
                .checked_add(chunk.bytes.len() as u64)
                .ok_or(NativeExecutorError::Capacity)?;
            if chunk.bytes.is_empty() || chunk.offset != chunk_cursor || next > tail.next_offset {
                return Err(NativeExecutorError::Journal);
            }
            output.extend_from_slice(&chunk.bytes);
            chunks.push(chunk);
            chunk_cursor = next;
        }
        if chunk_cursor != tail.next_offset {
            return Err(NativeExecutorError::Journal);
        }
        cursor = tail.next_offset;
    }
    if output.len() != output_len as usize {
        return Err(NativeExecutorError::Journal);
    }
    // Re-read from the owned host after draining, rather than manufacturing a
    // status from byte counts or copying the earlier observation. A fixed EOF
    // boundary and exited process identity must still match the original run.
    let final_status = host
        .status(run_id)
        .map_err(|_| NativeExecutorError::LaunchUnknown)?;
    if final_status.pid != observed.pid
        || final_status.start_identity != observed.start_identity
        || final_status.exit != observed.exit
        || final_status.capture_failed
        || final_status.capture_state != CaptureState::Complete
        || final_status.input_outcome_unknown
        || final_status.output_offset != output_len
        || final_status.final_output_offset != Some(output_len)
    {
        return Err(NativeExecutorError::LaunchUnknown);
    }
    let capture = (!chunks.is_empty()).then_some(CompletedCapture {
        run_id: run_id.to_owned(),
        chunks,
        final_status,
    });
    Ok(CollectedOutput { output, capture })
}

#[cfg(target_os = "linux")]
fn prove_closed(
    running: &mut RunningAdmission,
    host: &SessionHost,
) -> Result<ClosedAdmission, NativeExecutorError> {
    running
        .prove_closed(host)
        .map_err(|_| NativeExecutorError::LaunchUnknown)
}
#[cfg(not(target_os = "linux"))]
fn prove_closed(
    _running: &mut RunningAdmission,
    _host: &SessionHost,
) -> Result<ClosedAdmission, NativeExecutorError> {
    Err(NativeExecutorError::UnsupportedBackend)
}

/// Canonical full claim binding for the existing reservation scope. Trusted
/// reservation issuance must use this exact encoding; a digest is not a grant.
pub fn native_resource_claims_digest(
    c: &ResourcePermitClaims,
) -> Result<String, NativeExecutorError> {
    content_digest(&serde_json::json!({"schema_version":1,"issuer_ref":c.issuer_ref,"permit_id":c.permit_id,"reservation_ref":c.reservation_ref,"identity":c.identity,"host_incarnation":c.host_incarnation,"expires_at_unix_ms":c.expires_at_unix_ms,"lease_expires_at_unix_ms":c.lease_expires_at_unix_ms,"registered_cpu_scope":c.registered_cpu_scope,"cpu_set":c.cpu_set,"memory_max_bytes":c.memory_max_bytes,"max_tasks":c.max_tasks,"max_concurrency":c.max_concurrency,"launch_spec_digest":c.launch_spec_digest,"worker_environment_digest":c.worker_environment_digest,"backend_config_digest":c.backend_config_digest})).map_err(|_|NativeExecutorError::Digest)
}

/// Journal directory capability, pinned once with no-follow descriptor-relative
/// traversal. Later path renames/symlink substitution cannot redirect writes.
struct PrivateJournal {
    directory: File,
}
impl PrivateJournal {
    fn open(path: &Path) -> Result<Self, NativeExecutorError> {
        use rustix::fs::{Mode, OFlags, fstat, open, openat};
        if !path.is_absolute() {
            return Err(NativeExecutorError::Journal);
        }
        let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        let mut directory =
            open("/", flags, Mode::empty()).map_err(|_| NativeExecutorError::Journal)?;
        let uid = rustix::process::geteuid().as_raw();
        for component in path.components() {
            let m = fstat(&directory).map_err(|_| NativeExecutorError::Journal)?;
            if (m.st_uid != 0 && m.st_uid != uid)
                || (m.st_mode & 0o022 != 0 && m.st_mode & 0o1000 == 0)
            {
                return Err(NativeExecutorError::Journal);
            }
            match component {
                Component::RootDir => {}
                Component::Normal(name) => {
                    directory = openat(&directory, name, flags, Mode::empty())
                        .map_err(|_| NativeExecutorError::Journal)?
                }
                _ => return Err(NativeExecutorError::Journal),
            }
        }
        let file = File::from(directory);
        let m = file.metadata().map_err(|_| NativeExecutorError::Journal)?;
        if !m.is_dir() || m.uid() != uid || m.mode() & 0o777 != 0o700 {
            return Err(NativeExecutorError::Journal);
        }
        Ok(Self { directory: file })
    }
    fn validate_current(&self) -> Result<(), NativeExecutorError> {
        let m = self
            .directory
            .metadata()
            .map_err(|_| NativeExecutorError::Journal)?;
        if !m.is_dir()
            || m.uid() != rustix::process::geteuid().as_raw()
            || m.mode() & 0o777 != 0o700
        {
            return Err(NativeExecutorError::Journal);
        }
        Ok(())
    }
    fn exists(&self, name: &str) -> Result<bool, NativeExecutorError> {
        self.validate_current()?;
        match rustix::fs::statat(&self.directory, name, rustix::fs::AtFlags::SYMLINK_NOFOLLOW) {
            Ok(_) => Ok(true),
            Err(rustix::io::Errno::NOENT) => Ok(false),
            Err(_) => Err(NativeExecutorError::Journal),
        }
    }
    fn inspect_run(&self, run_id: &str) -> Result<Vec<RunJournalArtifact>, NativeExecutorError> {
        self.validate_current()?;
        let mut artifacts = Vec::with_capacity(3);
        for (kind, suffix) in [
            (RunJournalArtifactKind::Prepared, "prepared"),
            (RunJournalArtifactKind::Observed, "observed"),
            (RunJournalArtifactKind::Closed, "closed"),
        ] {
            let name = journal_name(run_id, suffix)?;
            if let Some((bytes, sha256)) = self.read_artifact(&name)? {
                artifacts.push(RunJournalArtifact {
                    kind,
                    bytes,
                    sha256,
                });
            }
        }
        self.validate_current()?;
        Ok(artifacts)
    }
    fn read_artifact(&self, name: &str) -> Result<Option<(u64, String)>, NativeExecutorError> {
        use rustix::fs::{AtFlags, FileType, OFlags, openat, statat};
        self.validate_current()?;
        if Path::new(name).components().count() != 1
            || !matches!(
                Path::new(name).components().next(),
                Some(Component::Normal(_))
            )
        {
            return Err(NativeExecutorError::Journal);
        }
        // Reject links and special files before opening. NONBLOCK also ensures
        // a raced-in FIFO cannot stall inspection; NOFOLLOW prevents links.
        let entry = match statat(&self.directory, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(entry) => entry,
            Err(rustix::io::Errno::NOENT) => return Ok(None),
            Err(_) => return Err(NativeExecutorError::Journal),
        };
        if FileType::from_raw_mode(entry.st_mode) != FileType::RegularFile
            || entry.st_uid != rustix::process::geteuid().as_raw()
            || entry.st_nlink != 1
            || entry.st_mode & 0o077 != 0
            || entry.st_size < 0
            || entry.st_size as u64 > 512 * 1024
        {
            return Err(NativeExecutorError::Journal);
        }
        let fd = match openat(
            &self.directory,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        ) {
            Ok(fd) => fd,
            Err(rustix::io::Errno::NOENT) => return Ok(None),
            Err(_) => return Err(NativeExecutorError::Journal),
        };
        let mut file = File::from(fd);
        let before = file.metadata().map_err(|_| NativeExecutorError::Journal)?;
        if !before.is_file()
            || before.dev() != entry.st_dev as u64
            || before.ino() != entry.st_ino as u64
            || before.uid() != rustix::process::geteuid().as_raw()
            || before.nlink() != 1
            || before.mode() & 0o077 != 0
            || before.len() > 512 * 1024
        {
            return Err(NativeExecutorError::Journal);
        }
        let mut contents = Vec::with_capacity(before.len() as usize);
        Read::by_ref(&mut file)
            .take(512 * 1024 + 1)
            .read_to_end(&mut contents)
            .map_err(|_| NativeExecutorError::Journal)?;
        let after = file.metadata().map_err(|_| NativeExecutorError::Journal)?;
        if contents.len() as u64 != before.len()
            || before.dev() != after.dev()
            || before.ino() != after.ino()
            || before.uid() != after.uid()
            || before.gid() != after.gid()
            || before.mode() != after.mode()
            || before.nlink() != after.nlink()
            || before.len() != after.len()
            || before.mtime() != after.mtime()
            || before.mtime_nsec() != after.mtime_nsec()
            || before.ctime() != after.ctime()
            || before.ctime_nsec() != after.ctime_nsec()
        {
            return Err(NativeExecutorError::Journal);
        }
        let after_entry = statat(&self.directory, name, AtFlags::SYMLINK_NOFOLLOW)
            .map_err(|_| NativeExecutorError::Journal)?;
        if FileType::from_raw_mode(after_entry.st_mode) != FileType::RegularFile
            || after_entry.st_dev as u64 != before.dev()
            || after_entry.st_ino as u64 != before.ino()
            || after_entry.st_uid != before.uid()
            || after_entry.st_nlink as u64 != before.nlink()
            || after_entry.st_mode as u32 != before.mode()
            || after_entry.st_size < 0
            || after_entry.st_size as u64 != before.len()
        {
            return Err(NativeExecutorError::Journal);
        }
        self.validate_current()?;
        let digest = Sha256::digest(&contents);
        let sha256 = digest.iter().map(|byte| format!("{byte:02x}")).collect();
        Ok(Some((contents.len() as u64, sha256)))
    }
    fn write_new<T: Serialize>(&self, name: &str, value: &T) -> Result<(), NativeExecutorError> {
        self.write(name, value, false)
    }
    fn write_same_or_new<T: Serialize>(
        &self,
        name: &str,
        value: &T,
    ) -> Result<(), NativeExecutorError> {
        self.write(name, value, true)
    }
    fn write<T: Serialize>(
        &self,
        name: &str,
        value: &T,
        allow_same: bool,
    ) -> Result<(), NativeExecutorError> {
        use rustix::fs::{Mode, OFlags, openat};
        use std::io::Read;
        self.validate_current()?;
        if Path::new(name).components().count() != 1
            || !matches!(
                Path::new(name).components().next(),
                Some(Component::Normal(_))
            )
        {
            return Err(NativeExecutorError::Journal);
        }
        let bytes = serde_json::to_vec(value).map_err(|_| NativeExecutorError::Journal)?;
        if bytes.len() > 512 * 1024 {
            return Err(NativeExecutorError::Journal);
        }
        let descriptor = openat(
            &self.directory,
            name,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_bits_truncate(0o600),
        );
        let mut file = match descriptor {
            Ok(fd) => File::from(fd),
            Err(rustix::io::Errno::EXIST) if allow_same => {
                let fd = openat(
                    &self.directory,
                    name,
                    OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
                    Mode::empty(),
                )
                .map_err(|_| NativeExecutorError::Journal)?;
                let mut f = File::from(fd);
                let m = f.metadata().map_err(|_| NativeExecutorError::Journal)?;
                if !m.is_file()
                    || m.uid() != rustix::process::geteuid().as_raw()
                    || m.nlink() != 1
                    || m.mode() & 0o077 != 0
                    || m.len() != bytes.len() as u64
                {
                    return Err(NativeExecutorError::Journal);
                }
                let mut previous = Vec::new();
                std::io::Read::by_ref(&mut f)
                    .take(bytes.len() as u64 + 1)
                    .read_to_end(&mut previous)
                    .map_err(|_| NativeExecutorError::Journal)?;
                if previous != bytes {
                    return Err(NativeExecutorError::Journal);
                }
                f.sync_all()
                    .and_then(|()| self.directory.sync_all())
                    .map_err(|_| NativeExecutorError::Journal)?;
                return Ok(());
            }
            Err(_) => return Err(NativeExecutorError::Journal),
        };
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .and_then(|()| self.directory.sync_all())
            .map_err(|_| NativeExecutorError::Journal)
    }
}
fn journal_name(run_id: &str, kind: &str) -> Result<String, NativeExecutorError> {
    Ok(format!(
        "{}.{}.json",
        content_digest(&run_id).map_err(|_| NativeExecutorError::Digest)?,
        kind
    ))
}
fn write_prelaunch_recovery(
    root: &PrivateJournal,
    recovery: &RecoveryIdentity,
    intent: &DispatchIntent,
    plan: &NativeExecutionPlan,
) -> Result<(), NativeExecutorError> {
    root.write_new(&journal_name(&recovery.run_id,"prepared")?,&serde_json::json!({"schema_version":1,"recovery":recovery,"launch_spec":plan.launch_spec,"workspace_head":plan.prepared_workspace.head_oid(),"workspace_tree":plan.prepared_workspace.tree_oid(),"reservation_scope":plan.reservation.scope,"reservation_id":plan.reservation.id,"task_id":intent.task_id,"attempt_id":intent.attempt_id,"owner":intent.owner,"contract_revision":intent.contract_revision,"contract_digest":intent.contract_digest,"context_digest":intent.context_digest,"policy_digest":intent.policy_digest,"provider_declared":intent.provider,"model_declared":intent.model}))
}

fn write_closed_evidence(
    root: &PrivateJournal,
    recovery: &RecoveryIdentity,
    closure: &ClosedAdmission,
    output_digest: &str,
    output_bytes: u64,
    provider_declared: &str,
    model_declared: &str,
) -> Result<(String, bool), NativeExecutorError> {
    let exit_code = closure.exit_code();
    let success = closure.success();
    let closure_digest = closure.closure_digest();
    let evidence = ClosedEvidence {
        schema_version: 1,
        recovery,
        closure: closure.authoritative_binding(),
        closure_digest,
        exit_code,
        success,
        output_digest,
        output_bytes,
        provider_declared,
        model_declared,
        provider_observed: None,
        model_observed: None,
    };
    let journal = closed_evidence_journal(evidence, closure.resource_usage_diagnostics())?;
    let journal_digest = content_digest(&journal).map_err(|_| NativeExecutorError::Digest)?;
    root.write_same_or_new(&journal_name(&recovery.run_id, "closed")?, &journal)?;
    Ok((format!("native-evidence-v2:{journal_digest}"), success))
}

fn valid_run_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_RUN_ID_BYTES
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use admiralty_core::nodes::NodeId;
    use admiralty_core::{
        AcceptanceCriterion, CheckSpec, PrincipalId, ProjectId, RootId, SchedulingState,
        TaskContract, TaskView,
    };
    use admiralty_runtime::{
        reservations::{AccountId, CurrencyUnit, MinorUnits, ReservationScope},
        resource_gate::ResourcePermitAuthority,
        workspace::preparation::{WorkspacePreparation, prepare_workspace},
        workspace::registry::NodeRef,
    };
    use std::{
        collections::BTreeSet,
        fs,
        os::unix::fs::{DirBuilderExt, PermissionsExt},
    };

    struct CurrentPermit;
    impl ResourcePermitAuthority for CurrentPermit {
        fn is_current(&self, _claims: &ResourcePermitClaims) -> bool {
            true
        }
    }

    struct RejectStarted;
    impl StartedSink for RejectStarted {
        fn record_started(
            &self,
            _: &DispatchIntent,
            _: &RecoveryIdentity,
        ) -> Result<(), StartedSinkError> {
            Err(StartedSinkError::Unavailable)
        }
    }
    struct Fixture {
        temp: PathBuf,
        journal: PathBuf,
        marker: PathBuf,
        view: TaskView,
        intent: DispatchIntent,
        plan: NativeExecutionPlan,
        executor: NativeExecutor,
    }
    fn fixture() -> Fixture {
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
            launch_spec_digest: "sha256:launch".into(),
            worker_environment_digest: "sha256:environment".into(),
            backend_config_digest: "sha256:backend".into(),
        };
        reservation.scope.resource_claims_digest =
            native_resource_claims_digest(&claims).expect("full resource claims digest");
        let view = TaskView {
            contract,
            state: SchedulingState::Claimed,
            attempt_id: Some(attempt_id.clone()),
            owner: Some(owner),
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
        let executor = NativeExecutor::new(
            ResourceGate::new(node_id),
            &journal,
            Duration::from_millis(50),
            1024,
            1,
            Arc::new(RejectStarted),
        )
        .expect("executor");
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
        Fixture {
            temp,
            journal,
            marker,
            view,
            intent,
            plan,
            executor,
        }
    }

    fn preparation_identity(plan: &NativeExecutionPlan) -> (PathBuf, u64, u64, String, String) {
        let path = plan.prepared_workspace.workspace().workspace.clone();
        let metadata = fs::metadata(&path).expect("original workspace inode");
        (
            path,
            metadata.dev(),
            metadata.ino(),
            plan.prepared_workspace.head_oid().into(),
            plan.prepared_workspace.tree_oid().into(),
        )
    }

    #[test]
    fn rejected_registration_returns_original_preparation_for_invalid_and_journal_errors() {
        for fault in ["invalid", "journal"] {
            let Fixture {
                temp,
                journal,
                view,
                intent,
                mut plan,
                executor,
                ..
            } = fixture();
            let identity = preparation_identity(&plan);
            if fault == "invalid" {
                plan.launch_spec.run_id = "different-run".into();
            } else {
                fs::set_permissions(&journal, fs::Permissions::from_mode(0o755))
                    .expect("revoke private journal");
            }
            let rejected = executor
                .try_register_plan(plan)
                .expect_err("registration must fail");
            assert!(matches!(
                (fault, rejected.error()),
                ("invalid", NativeExecutorError::InvalidBinding)
                    | ("journal", NativeExecutorError::Journal)
            ));
            let (_, mut returned) = rejected.into_parts();
            assert_eq!(
                preparation_identity(&returned),
                identity,
                "actual original workspace retained"
            );
            assert!(executor.plans.lock().expect("plans").is_empty());
            assert!(executor.reserved_runs.lock().expect("reserved").is_empty());
            returned.launch_spec.run_id = intent.run_id.clone();
            fs::set_permissions(&journal, fs::Permissions::from_mode(0o700))
                .expect("restore owned protection");
            assert!(validate_bindings(&view, &intent, &returned).is_ok());
            executor
                .try_register_plan(returned)
                .expect("original capability remains usable");
            let returned = executor
                .withdraw_unstarted_plan(&intent.run_id)
                .expect("withdraw")
                .expect("actual plan");
            assert_eq!(preparation_identity(&returned), identity);
            fs::remove_dir_all(temp).expect("owned fixture cleanup");
        }
    }

    #[test]
    fn duplicate_and_capacity_rejections_preserve_both_original_preparations() {
        let first = fixture();
        let first_identity = preparation_identity(&first.plan);
        first
            .executor
            .try_register_plan(first.plan)
            .expect("first actual plan");
        let mut second = fixture();
        let second_identity = preparation_identity(&second.plan);
        let rejected = first
            .executor
            .try_register_plan(second.plan)
            .expect_err("duplicate run rejected");
        assert!(matches!(
            rejected.error(),
            NativeExecutorError::DuplicateRun
        ));
        let (_, mut returned) = rejected.into_parts();
        assert_eq!(preparation_identity(&returned), second_identity);
        assert_eq!(
            preparation_identity(
                first
                    .executor
                    .plans
                    .lock()
                    .expect("plans")
                    .get(&first.intent.run_id)
                    .expect("first retained")
            ),
            first_identity
        );
        returned.run_id = "native-run-2".into();
        returned.launch_spec.run_id = returned.run_id.clone();
        second.intent.run_id = returned.run_id.clone();
        let rejected = first
            .executor
            .try_register_plan(returned)
            .expect_err("capacity rejects without consuming");
        assert!(matches!(rejected.error(), NativeExecutorError::Capacity));
        let (_, returned) = rejected.into_parts();
        assert_eq!(preparation_identity(&returned), second_identity);
        let original = first
            .executor
            .withdraw_unstarted_plan(&first.intent.run_id)
            .expect("safe withdraw")
            .expect("first plan");
        assert_eq!(preparation_identity(&original), first_identity);
        assert!(validate_bindings(&second.view, &second.intent, &returned).is_ok());
        first
            .executor
            .try_register_plan(returned)
            .expect("returned capability can register after capacity frees");
        let original_second = first
            .executor
            .withdraw_unstarted_plan(&second.intent.run_id)
            .expect("second withdraw")
            .expect("second plan");
        assert_eq!(preparation_identity(&original_second), second_identity);
        assert!(first.executor.pending_run_ids().is_empty());
        fs::remove_dir_all(first.temp).expect("first owned cleanup");
        fs::remove_dir_all(second.temp).expect("second owned cleanup");
    }

    #[test]
    fn existing_journal_rejects_registration_without_consuming_preparation() {
        let Fixture {
            temp,
            intent,
            plan,
            executor,
            ..
        } = fixture();
        let identity = preparation_identity(&plan);
        let name = journal_name(&intent.run_id, "prepared").expect("journal name");
        let evidence = serde_json::json!({"prior":"unknown"});
        executor
            .journal
            .write_new(&name, &evidence)
            .expect("existing durable barrier");
        let rejected = executor
            .try_register_plan(plan)
            .expect_err("existing identity is not retryable");
        assert!(matches!(
            rejected.error(),
            NativeExecutorError::DuplicateRun
        ));
        let (_, returned) = rejected.into_parts();
        assert_eq!(preparation_identity(&returned), identity);
        assert!(executor.journal.exists(&name).expect("evidence preserved"));
        assert!(executor.plans.lock().expect("plans").is_empty());
        assert!(executor.reserved_runs.lock().expect("reserved").is_empty());
        fs::remove_dir_all(temp).expect("owned cleanup");
    }

    #[test]
    fn known_binding_permit_and_gate_failures_retain_original_unstarted_plan() {
        struct DenyPermit;
        impl ResourcePermitAuthority for DenyPermit {
            fn is_current(&self, _: &ResourcePermitClaims) -> bool {
                false
            }
        }
        for fault in ["binding", "permit", "gate"] {
            let Fixture {
                temp,
                journal,
                marker,
                mut view,
                intent,
                mut plan,
                executor,
            } = fixture();
            let identity = preparation_identity(&plan);
            if fault == "binding" {
                view.state = SchedulingState::Ready;
            }
            if fault == "permit" {
                plan.permit_authority = Arc::new(DenyPermit);
            }
            executor
                .try_register_plan(plan)
                .expect("register original plan");
            let failed = executor
                .launch_plan(&view, &intent)
                .expect_err("known pre-spawn denial");
            assert!(matches!(
                (fault, failed),
                ("binding", NativeExecutorError::InvalidBinding)
                    | ("permit" | "gate", NativeExecutorError::PermitDenied)
            ));
            assert_eq!(
                preparation_identity(
                    executor
                        .plans
                        .lock()
                        .expect("plans")
                        .get(&intent.run_id)
                        .expect("retained original")
                ),
                identity
            );
            assert!(
                executor
                    .reserved_runs
                    .lock()
                    .expect("reserved")
                    .contains(&intent.run_id)
            );
            assert!(
                executor
                    .launch_fenced_runs
                    .lock()
                    .expect("fenced")
                    .is_empty()
            );
            assert!(executor.pending_run_ids().is_empty());
            assert!(!marker.exists());
            assert_eq!(fs::read_dir(&journal).expect("journal").count(), 0);
            let returned = executor
                .withdraw_unstarted_plan(&intent.run_id)
                .expect("known unstarted withdrawal")
                .expect("original plan");
            assert_eq!(preparation_identity(&returned), identity);
            fs::remove_dir_all(temp).expect("owned cleanup");
        }
    }

    fn prelaunch_fixture_identity(plan: &NativeExecutionPlan) -> RecoveryIdentity {
        // Persistence-failure fixture only. This creates no gate admission,
        // RunningAdmission, child, accepted evidence or native qualification.
        RecoveryIdentity {
            identity: plan.permit_claims.identity.clone(),
            launch_spec_digest: plan.permit_claims.launch_spec_digest.clone(),
            worker_environment_digest: plan.permit_claims.worker_environment_digest.clone(),
            unit: "fixture-prelaunch.service".into(),
            run_id: plan.run_id.clone(),
            permit_id: plan.permit_claims.permit_id.clone(),
            reservation_ref: plan.reservation.id.clone(),
            host_incarnation: plan.permit_claims.host_incarnation.clone(),
            backend_config_digest: plan.permit_claims.backend_config_digest.clone(),
            invocation_id: None,
            main_pid: None,
            process_start: None,
        }
    }

    #[test]
    fn prelaunch_write_failures_keep_plan_and_fence_without_silent_retry_or_withdrawal() {
        for fault in ["before-create", "create-collision"] {
            let Fixture {
                temp,
                journal,
                marker,
                view,
                intent,
                plan,
                executor,
            } = fixture();
            let identity = preparation_identity(&plan);
            let recovery = prelaunch_fixture_identity(&plan);
            executor
                .try_register_plan(plan)
                .expect("register actual preparation");
            let name = journal_name(&intent.run_id, "prepared").expect("journal name");
            if fault == "before-create" {
                fs::set_permissions(&journal, fs::Permissions::from_mode(0o755))
                    .expect("reject before create");
            } else {
                fs::write(journal.join(&name), b"existing-uncertain-write")
                    .expect("existing write barrier");
            }
            {
                let plans = executor.plans.lock().expect("original retained");
                assert!(matches!(
                    executor.persist_prelaunch(
                        &recovery,
                        &intent,
                        plans.get(&intent.run_id).expect("plan")
                    ),
                    Err(NativeExecutorError::Journal)
                ));
            }
            fs::set_permissions(&journal, fs::Permissions::from_mode(0o700))
                .expect("restore owned protection");
            assert_eq!(
                executor.journal.exists(&name).expect("query evidence"),
                fault == "create-collision"
            );
            assert_eq!(
                preparation_identity(
                    executor
                        .plans
                        .lock()
                        .expect("plans")
                        .get(&intent.run_id)
                        .expect("retained plan")
                ),
                identity
            );
            assert!(
                executor
                    .reserved_runs
                    .lock()
                    .expect("reserved")
                    .contains(&intent.run_id)
            );
            assert!(
                executor
                    .launch_fenced_runs
                    .lock()
                    .expect("fenced")
                    .contains(&intent.run_id)
            );
            assert!(matches!(
                executor.withdraw_unstarted_plan(&intent.run_id),
                Err(NativeExecutorError::LaunchUnknown)
            ));
            assert!(matches!(
                executor.launch_plan(&view, &intent),
                Err(NativeExecutorError::LaunchUnknown)
            ));
            assert!(executor.pending_run_ids().is_empty());
            assert!(executor.completed.lock().expect("completed").is_empty());
            assert!(!marker.exists());
            if fault == "create-collision" {
                assert_eq!(
                    fs::read(journal.join(&name)).expect("preserved collision"),
                    b"existing-uncertain-write"
                );
            } else {
                assert_eq!(
                    fs::read_dir(&journal).expect("no automatic retry").count(),
                    0
                );
            }
            fs::remove_dir_all(temp).expect("owned cleanup");
        }
    }

    #[test]
    fn prelaunch_fence_requires_an_existing_run_reservation() {
        let Fixture {
            temp,
            journal,
            plan,
            intent,
            executor,
            ..
        } = fixture();
        let recovery = prelaunch_fixture_identity(&plan);
        assert!(matches!(
            executor.persist_prelaunch(&recovery, &intent, &plan),
            Err(NativeExecutorError::LaunchUnknown)
        ));
        assert!(
            executor
                .launch_fenced_runs
                .lock()
                .expect("fences")
                .is_empty()
        );
        assert_eq!(fs::read_dir(journal).expect("journal").count(), 0);
        fs::remove_dir_all(temp).expect("owned cleanup");
    }

    #[test]
    fn unconfigured_resource_gate_denies_before_child_spawn_and_keeps_marker_absent() {
        let Fixture {
            temp,
            journal,
            marker,
            view,
            intent,
            mut plan,
            executor,
        } = fixture();
        assert!(
            validate_bindings(&view, &intent, &plan).is_ok(),
            "actual prepared workspace and full reservation binding"
        );
        let original = plan.reservation.scope.clone();
        for mutation in ["revision", "authority", "boot", "claims"] {
            plan.reservation.scope = original.clone();
            match mutation {
                "revision" => plan.reservation.scope.contract_revision += 1,
                "authority" => plan.reservation.scope.authority_ref = "other-authority".into(),
                "boot" => plan.reservation.scope.incarnation = "other-boot".into(),
                _ => {
                    plan.reservation.scope.resource_claims_digest =
                        format!("sha256:{}", "f".repeat(64))
                }
            }
            assert!(
                matches!(
                    validate_bindings(&view, &intent, &plan),
                    Err(NativeExecutorError::InvalidBinding)
                ),
                "reservation {mutation} must bind before launch"
            );
        }
        plan.reservation.scope = original;
        let original_claim_digest =
            native_resource_claims_digest(&plan.permit_claims).expect("claims digest");
        plan.permit_claims.lease_expires_at_unix_ms -= 1;
        assert_ne!(
            original_claim_digest,
            native_resource_claims_digest(&plan.permit_claims).expect("changed claims digest")
        );
        assert!(
            validate_bindings(&view, &intent, &plan).is_err(),
            "lease mutation invalidates reservation binding"
        );
        plan.permit_claims.lease_expires_at_unix_ms += 1;
        let original_preparation = preparation_identity(&plan);
        executor
            .try_register_plan(plan)
            .expect("prepared trusted plan");
        let result = executor.execute(&view, &intent);
        assert!(result.is_err(), "the unconfigured backend must deny launch");
        assert!(!marker.exists(), "the child must not have started");
        assert_eq!(
            fs::read_dir(&journal)
                .expect("journal remains readable")
                .count(),
            0,
            "no recovery journal is needed before a gate-admitted launch"
        );
        assert!(
            executor
                .reserved_runs
                .lock()
                .expect("reservation retained")
                .contains(&intent.run_id)
        );
        let original = executor
            .withdraw_unstarted_plan(&intent.run_id)
            .expect("known pre-spawn failure is withdrawable")
            .expect("original opaque preparation");
        assert_eq!(preparation_identity(&original), original_preparation);
        assert!(validate_bindings(&view, &intent, &original).is_ok());
        executor
            .try_register_plan(original)
            .expect("actual original capability is reusable");
        fs::remove_dir_all(temp).expect("remove private fixture");
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

    #[test]
    fn withdraw_returns_actual_unstarted_preparation_and_releases_only_its_reservation() {
        let Fixture {
            temp,
            journal,
            marker,
            view,
            intent,
            plan,
            executor,
        } = fixture();
        let head = plan.prepared_workspace.head_oid().to_owned();
        let tree = plan.prepared_workspace.tree_oid().to_owned();
        executor.register_plan(plan).expect("register actual plan");
        let original = executor
            .withdraw_unstarted_plan(&intent.run_id)
            .expect("withdraw safely")
            .expect("actual plan returned");
        assert_eq!(original.prepared_workspace.head_oid(), head);
        assert_eq!(original.prepared_workspace.tree_oid(), tree);
        assert!(validate_bindings(&view, &intent, &original).is_ok());
        assert!(
            executor
                .withdraw_unstarted_plan(&intent.run_id)
                .expect("already withdrawn")
                .is_none()
        );
        assert!(executor.pending_run_ids().is_empty());
        assert!(!marker.exists());
        assert_eq!(
            fs::read_dir(&journal)
                .expect("no journal deletion or writes")
                .count(),
            0
        );
        executor
            .register_plan(original)
            .expect("reservation released, original capability reusable");
        assert!(
            executor
                .withdraw_unstarted_plan(&intent.run_id)
                .expect("second safe withdrawal")
                .is_some()
        );
        fs::remove_dir_all(temp).expect("owned fixture cleanup");
    }

    #[test]
    fn withdraw_refuses_durable_launch_evidence_without_losing_plan_or_evidence() {
        let Fixture {
            temp,
            marker,
            intent,
            plan,
            executor,
            ..
        } = fixture();
        let head = plan.prepared_workspace.head_oid().to_owned();
        executor.register_plan(plan).expect("register actual plan");
        let name = journal_name(&intent.run_id, "prepared").expect("journal name");
        executor
            .journal
            .write_new(&name, &serde_json::json!({"retained":"prepared"}))
            .expect("durable barrier");
        assert!(matches!(
            executor.withdraw_unstarted_plan(&intent.run_id),
            Err(NativeExecutorError::LaunchUnknown)
        ));
        assert!(
            executor
                .journal
                .exists(&name)
                .expect("original evidence retained")
        );
        assert!(
            executor
                .reserved_runs
                .lock()
                .expect("reservations")
                .contains(&intent.run_id)
        );
        assert_eq!(
            executor
                .plans
                .lock()
                .expect("plans")
                .get(&intent.run_id)
                .expect("original plan retained")
                .prepared_workspace
                .head_oid(),
            head
        );
        assert!(!marker.exists());
        assert!(executor.pending_run_ids().is_empty());
        fs::remove_dir_all(temp).expect("owned fixture cleanup");
    }

    #[test]
    fn started_sink_requires_durable_observation_and_retries_without_marking_rejection_done() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct LoseFirstAck {
            calls: AtomicUsize,
        }
        impl StartedSink for LoseFirstAck {
            fn record_started(
                &self,
                _: &DispatchIntent,
                _: &RecoveryIdentity,
            ) -> Result<(), StartedSinkError> {
                if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    Err(StartedSinkError::Unavailable)
                } else {
                    Ok(())
                }
            }
        }
        let Fixture {
            temp, intent, plan, ..
        } = fixture();
        let recovery = RecoveryIdentity {
            identity: plan.permit_claims.identity.clone(),
            launch_spec_digest: plan.permit_claims.launch_spec_digest.clone(),
            worker_environment_digest: plan.permit_claims.worker_environment_digest.clone(),
            unit: "observed.service".into(),
            run_id: intent.run_id.clone(),
            permit_id: plan.permit_claims.permit_id.clone(),
            reservation_ref: plan.reservation.id.clone(),
            host_incarnation: plan.permit_claims.host_incarnation.clone(),
            backend_config_digest: plan.permit_claims.backend_config_digest.clone(),
            invocation_id: Some("actual-invocation".into()),
            main_pid: Some(1234),
            process_start: Some("observed-start".into()),
        };
        let sink = LoseFirstAck {
            calls: AtomicUsize::new(0),
        };
        let mut persisted = false;
        persist_started_once(&sink, &intent, &recovery, false, &mut persisted)
            .expect("no unobserved callback");
        assert_eq!(sink.calls.load(Ordering::SeqCst), 0);
        assert!(!persisted);
        assert!(matches!(
            persist_started_once(&sink, &intent, &recovery, true, &mut persisted),
            Err(NativeExecutorError::StartedPersistence)
        ));
        assert!(!persisted, "a rejection cannot advance pending state");
        persist_started_once(&sink, &intent, &recovery, true, &mut persisted)
            .expect("retry acknowledgement");
        assert!(persisted);
        persist_started_once(&sink, &intent, &recovery, true, &mut persisted)
            .expect("no repeated acknowledged callback");
        assert_eq!(sink.calls.load(Ordering::SeqCst), 2);
        fs::remove_dir_all(temp).expect("owned fixture cleanup");
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
    fn journal_fixture() -> PathBuf {
        private_fixture("native-journal-test")
    }

    #[test]
    fn resource_journal_versions_full_bytes_without_changing_authority_binding() {
        // Inert serialization fixture only; this cannot construct an opaque
        // admission proof or qualify a Linux run.
        let authority = serde_json::json!({"schema_version":1,"closure":{"identity":"fixture",
            "observation":"fixture","exit_code":0,"success":true,"closure_digest":"fixed"}});
        let before = ResourceUsageDiagnostics::default();
        let after: ResourceUsageDiagnostics = serde_json::from_value(serde_json::json!({
            "sample_attempts": 1,
            "final_sample": {"state":"unavailable","reason":"missing"}
        }))
        .expect("inert advisory data cannot reconstruct closure");
        let first = closed_evidence_journal(&authority, &before).expect("first record");
        let last = closed_evidence_journal(&authority, &after).expect("last record");
        assert_eq!(first.schema_version, 2);
        assert_eq!(first.authority_digest, content_digest(&authority).unwrap());
        assert_eq!(first.authority_digest, last.authority_digest);
        assert_ne!(
            content_digest(&first).unwrap(),
            content_digest(&last).unwrap()
        );
        let root = journal_fixture();
        let journal = PrivateJournal::open(&root).expect("actual private journal");
        journal
            .write_same_or_new("closed.json", &last)
            .expect("actual write");
        journal
            .write_same_or_new("closed.json", &last)
            .expect("exact retry");
        assert!(matches!(
            journal.write_same_or_new("closed.json", &first),
            Err(NativeExecutorError::Journal)
        ));
        let actual: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join("closed.json")).unwrap()).unwrap();
        assert_eq!(actual["authority_evidence"], authority);
        assert_eq!(
            actual["resource_usage_advisory"]["final_sample"]["state"],
            "unavailable"
        );
        assert_eq!(
            actual["resource_usage_advisory"]["final_sample"]["reason"],
            "missing"
        );
        fs::remove_dir_all(root).expect("owned fixture cleanup");
    }

    fn capture_child(root: &Path, run: &str, script: &str) -> SessionHost {
        let host = SessionHost::new();
        host.launch_with_environment(
            LaunchSpec {
                run_id: run.into(),
                program: PathBuf::from("/bin/sh"),
                args: vec!["-c".into(), script.into()],
                cwd: root.into(),
                spool_dir: root.join(run),
            },
            &[],
        )
        .expect("actual controlled child with empty environment");
        host
    }

    fn completed_status(host: &SessionHost, run: &str) -> SessionStatus {
        completed_status_with_timeout(host, run, Duration::from_secs(5))
    }

    fn completed_status_with_timeout(
        host: &SessionHost,
        run: &str,
        timeout: Duration,
    ) -> SessionStatus {
        let deadline = Instant::now() + timeout;
        loop {
            let observed = host.status(run);
            if let Ok(status) = &observed
                && status.exit.is_some()
                && status.capture_state != CaptureState::Capturing
            {
                return status.clone();
            }
            assert!(
                Instant::now() < deadline,
                "controlled child capture deadline after {timeout:?}: {observed:?}"
            );
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn completed_capture_retains_actual_tagged_chunks_and_final_host_watermark() {
        use admiralty_runtime::session::OutputStream;
        let root = private_fixture("native-capture-tags");
        let host = capture_child(
            &root,
            "tagged",
            "printf 'actual stdout'; printf 'actual stderr' >&2",
        );
        host.close_input("tagged").expect("close actual input");
        let observed = completed_status(&host, "tagged");
        let captured = collect_output(&host, "tagged", &observed, 1024).expect("actual capture");
        let proof = captured.capture.expect("nonempty complete tagged capture");
        assert_eq!(proof.run_id(), "tagged");
        assert_eq!(
            proof.final_status(),
            &host.status("tagged").expect("actual final status")
        );
        assert_eq!(proof.final_status().exit, Some((Some(0), true)));
        assert_eq!(proof.final_status().final_output_offset, Some(26));
        assert!(proof.final_status().input_closed);
        // SessionHost explicitly reports no start identity off Linux; retain
        // that exact observation rather than inventing a portable substitute.
        assert_eq!(proof.final_status().start_identity, observed.start_identity);
        let original = host.tail("tagged", 0, 1024).expect("original host chunks");
        assert_eq!(proof.chunks(), original.chunks);
        let mut cursor = 0;
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut flattened = Vec::new();
        for chunk in proof.chunks() {
            assert_eq!(chunk.offset, cursor);
            cursor += chunk.bytes.len() as u64;
            flattened.extend_from_slice(&chunk.bytes);
            match chunk.stream {
                OutputStream::Stdout => stdout.extend_from_slice(&chunk.bytes),
                OutputStream::Stderr => stderr.extend_from_slice(&chunk.bytes),
            }
        }
        assert_eq!(stdout, b"actual stdout");
        assert_eq!(stderr, b"actual stderr");
        assert_eq!(flattened, captured.output);
        assert_eq!(cursor, proof.final_status().output_offset);
        drop(host);
        fs::remove_dir_all(root).expect("owned capture cleanup");
    }

    #[test]
    fn capture_rejects_actual_incomplete_overlimit_truncated_and_unknown_sources() {
        let root = private_fixture("native-capture-deny");
        let waiting = capture_child(&root, "waiting", "read ignored");
        let incomplete = waiting.status("waiting").expect("actual live status");
        assert!(incomplete.exit.is_none());
        assert!(matches!(
            collect_output(&waiting, "waiting", &incomplete, 1024),
            Err(NativeExecutorError::LaunchUnknown)
        ));
        waiting
            .close_input("waiting")
            .expect("finish actual waiting child");
        let _ = completed_status(&waiting, "waiting");
        drop(waiting);

        let host = capture_child(&root, "bounded", "printf 'bounded bytes'");
        host.close_input("bounded").expect("close actual input");
        let observed = completed_status(&host, "bounded");
        assert!(matches!(
            collect_output(&host, "bounded", &observed, 3),
            Err(NativeExecutorError::Capacity)
        ));
        assert!(collect_output(&host, "missing", &observed, 1024).is_err());
        let mut changed_identity = observed.clone();
        changed_identity.pid += 1;
        assert!(matches!(
            collect_output(&host, "bounded", &changed_identity, 1024),
            Err(NativeExecutorError::LaunchUnknown)
        ));
        File::options()
            .write(true)
            .open(root.join("bounded/output.journal"))
            .expect("owned journal")
            .set_len(1)
            .expect("simulate actual truncated capture");
        assert!(matches!(
            collect_output(&host, "bounded", &observed, 1024),
            Err(NativeExecutorError::Journal)
        ));
        drop(host);
        fs::remove_dir_all(root).expect("owned capture cleanup");
    }

    #[test]
    fn silent_completed_child_has_legacy_output_but_no_authenticated_provider_transcript() {
        let root = private_fixture("native-capture-silent");
        let host = capture_child(&root, "silent", "exit 0");
        host.close_input("silent").expect("close actual input");
        let observed = completed_status(&host, "silent");
        let collected =
            collect_output(&host, "silent", &observed, 1024).expect("silent completion");
        assert!(collected.output.is_empty());
        assert!(collected.capture.is_none());
        File::options()
            .write(true)
            .open(root.join("silent/output.journal"))
            .expect("owned empty journal")
            .set_len(1)
            .expect("simulate inconsistent silent capture");
        assert!(matches!(
            collect_output(&host, "silent", &observed, 1024),
            Err(NativeExecutorError::Journal)
        ));
        drop(host);
        fs::remove_dir_all(root).expect("owned capture cleanup");
    }

    #[test]
    fn actual_producer_capture_failure_cannot_yield_typed_transcript() {
        let root = private_fixture("native-capture-failed");
        // Exceed SessionHost's bounded journal with an actual local producer;
        // no synthesized CaptureState or completion flag can qualify this run.
        let host = capture_child(&root, "failed", "/usr/bin/head -c 9000000 /dev/zero");
        host.close_input("failed").expect("close actual input");
        // This crosses 8 MiB through >1000 actual sync_data durability
        // boundaries. It tests capture failure, not a five-second storage SLA.
        // Keep all real byte limits, failure assertions and child-exit checks.
        let observed = completed_status_with_timeout(&host, "failed", Duration::from_secs(30));
        assert!(observed.capture_failed);
        assert_eq!(observed.capture_state, CaptureState::Failed);
        assert!(collect_output(&host, "failed", &observed, 16 * 1024 * 1024).is_err());
        drop(host);
        fs::remove_dir_all(root).expect("owned capture cleanup");
    }

    #[test]
    fn journal_descriptor_stays_with_original_directory_after_path_substitution() {
        let root = journal_fixture();
        let named = root.join("named");
        let retained = root.join("retained");
        let trap = root.join("trap");
        for path in [&named, &trap] {
            fs::create_dir(path).expect("create journal");
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).expect("private journal");
        }
        let journal = PrivateJournal::open(&named).expect("pin directory capability");
        fs::rename(&named, &retained).expect("rename directory");
        std::os::unix::fs::symlink(&trap, &named).expect("substitute symlink");
        journal
            .write_new("evidence.json", &serde_json::json!({"actual":"retained"}))
            .expect("descriptor-relative durable write");
        assert!(retained.join("evidence.json").is_file());
        assert!(!trap.join("evidence.json").exists());
        assert!(
            PrivateJournal::open(&named).is_err(),
            "new symlink configuration rejected"
        );
        let m = fs::metadata(retained.join("evidence.json")).expect("metadata");
        assert_eq!(m.mode() & 0o777, 0o600);
        drop(journal);
        fs::remove_dir_all(root).expect("remove owned fixture");
    }
    #[test]
    fn journal_preserves_collision_evidence_and_allows_only_exact_idempotent_record() {
        let root = journal_fixture();
        let journal = PrivateJournal::open(&root).expect("private journal");
        let first = serde_json::json!({"proof":"one"});
        let changed = serde_json::json!({"proof":"two"});
        journal
            .write_new("proof.json", &first)
            .expect("first durable write");
        assert!(
            journal.write_new("proof.json", &first).is_err(),
            "prelaunch must never reuse a record"
        );
        journal
            .write_same_or_new("proof.json", &first)
            .expect("exact lost-write reply can reconcile");
        assert!(
            journal.write_same_or_new("proof.json", &changed).is_err(),
            "different evidence rejected"
        );
        assert_eq!(
            fs::read(root.join("proof.json")).expect("read retained evidence"),
            serde_json::to_vec(&first).expect("serialize expected")
        );
        assert!(journal.exists("proof.json").expect("existing tombstone"));
        assert!(!journal.exists("absent.json").expect("absent record"));
        fs::set_permissions(&root, fs::Permissions::from_mode(0o755))
            .expect("simulate revoked private root");
        assert!(
            journal.write_new("exposed.json", &first).is_err(),
            "changed directory protection must deny writes"
        );
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700))
            .expect("restore owned fixture");
        drop(journal);
        fs::remove_dir_all(root).expect("remove owned fixture");
    }

    #[test]
    fn inspect_run_journal_reports_ordered_raw_lengths_and_digests_only() {
        let Fixture {
            temp,
            journal,
            executor,
            ..
        } = fixture();
        let run_id = "inspect-opaque-1";
        let raw = [
            ("prepared", b"not-json prepared\n".as_slice()),
            ("observed", b"{unparsed observed".as_slice()),
            ("closed", b"closed\0bytes".as_slice()),
        ];
        for (kind, bytes) in raw {
            let path = journal.join(journal_name(run_id, kind).expect("fixed artifact name"));
            fs::write(&path, bytes).expect("seed opaque artifact");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
                .expect("private artifact mode");
        }
        let before: Vec<_> = ["prepared", "observed", "closed"]
            .into_iter()
            .map(|kind| {
                let path = journal.join(journal_name(run_id, kind).expect("fixed artifact name"));
                (path.clone(), fs::read(path).expect("original bytes"))
            })
            .collect();

        let reports = executor
            .inspect_run_journal(run_id)
            .expect("inspect opaque fixed artifacts");
        assert_eq!(
            reports
                .iter()
                .map(|artifact| artifact.kind)
                .collect::<Vec<_>>(),
            vec![
                RunJournalArtifactKind::Prepared,
                RunJournalArtifactKind::Observed,
                RunJournalArtifactKind::Closed,
            ]
        );
        for (artifact, (_, original)) in reports.iter().zip(&before) {
            assert_eq!(artifact.bytes, original.len() as u64);
            assert_eq!(
                artifact.sha256,
                Sha256::digest(original)
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>()
            );
        }
        for (path, bytes) in &before {
            assert_eq!(
                fs::read(path).expect("inspection leaves bytes unchanged"),
                *bytes
            );
        }
        assert!(matches!(
            executor.inspect_run_journal("bad/run"),
            Err(NativeExecutorError::InvalidBinding)
        ));
        assert!(matches!(
            executor.inspect_run_journal(&"r".repeat(MAX_RUN_ID_BYTES + 1)),
            Err(NativeExecutorError::InvalidBinding)
        ));
        drop(executor);
        fs::remove_dir_all(temp).expect("owned fixture cleanup");
    }

    #[test]
    fn inspect_run_journal_returns_empty_for_absent_run_artifacts() {
        let Fixture { temp, executor, .. } = fixture();
        assert!(
            executor
                .inspect_run_journal("inspect-empty")
                .expect("empty inspection")
                .is_empty()
        );
        drop(executor);
        fs::remove_dir_all(temp).expect("owned fixture cleanup");
    }

    #[test]
    fn inspect_run_journal_fails_closed_on_link_and_oversized_artifact() {
        let Fixture {
            temp,
            journal,
            executor,
            ..
        } = fixture();
        let run_id = "inspect-hostile-1";
        let prepared = journal_name(run_id, "prepared").expect("fixed artifact name");
        let target = journal.join("outside-target");
        fs::write(&target, b"do not follow").expect("target");
        std::os::unix::fs::symlink(&target, journal.join(&prepared)).expect("substitute link");
        assert!(matches!(
            executor.inspect_run_journal(run_id),
            Err(NativeExecutorError::Journal)
        ));
        fs::remove_file(journal.join(&prepared)).expect("remove test link");
        let socket_path = std::env::temp_dir().join(format!("admj-{}", std::process::id()));
        let _socket =
            std::os::unix::net::UnixListener::bind(&socket_path).expect("create socket fixture");
        fs::rename(&socket_path, journal.join(&prepared)).expect("place nonregular artifact");
        assert!(matches!(
            executor.inspect_run_journal(run_id),
            Err(NativeExecutorError::Journal)
        ));
        drop(_socket);
        fs::remove_file(journal.join(&prepared)).expect("remove socket fixture");
        let private = journal.join(&prepared);
        fs::write(&private, b"private evidence").expect("private test artifact");
        fs::set_permissions(&private, fs::Permissions::from_mode(0o600))
            .expect("private artifact mode");
        fs::set_permissions(&private, fs::Permissions::from_mode(0o644))
            .expect("simulate exposed artifact");
        assert!(matches!(
            executor.inspect_run_journal(run_id),
            Err(NativeExecutorError::Journal)
        ));
        fs::set_permissions(&private, fs::Permissions::from_mode(0o600))
            .expect("restore private artifact");
        let alias = journal.join("hardlink-alias");
        fs::hard_link(&private, &alias).expect("create hardlink");
        assert!(matches!(
            executor.inspect_run_journal(run_id),
            Err(NativeExecutorError::Journal)
        ));
        fs::remove_file(alias).expect("remove hardlink");
        fs::remove_file(private).expect("remove intermediate artifact");
        let oversized = journal.join(&prepared);
        fs::write(&oversized, vec![b'x'; 512 * 1024 + 1]).expect("oversized file");
        fs::set_permissions(&oversized, fs::Permissions::from_mode(0o600))
            .expect("private oversized artifact");
        assert!(matches!(
            executor.inspect_run_journal(run_id),
            Err(NativeExecutorError::Journal)
        ));
        drop(executor);
        fs::remove_dir_all(temp).expect("owned fixture cleanup");
    }

    #[test]
    #[ignore]
    fn child_marker() {
        if let Some(path) = std::env::args().next_back() {
            let _ = fs::write(path, b"spawned");
        }
    }
}
