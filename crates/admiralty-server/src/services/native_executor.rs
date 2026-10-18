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
        ResourcePermitAuthority, ResourcePermitClaims, RunningAdmission, VerifiedResourcePermit,
    },
    session::{CaptureState, LaunchSpec, SessionHost},
    workspace::{preparation::PreparedWorkspace, registry::WorkspaceSpec},
};
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::Write,
    os::unix::fs::MetadataExt,
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

const MAX_RUN_ID_BYTES: usize = 128;

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

pub struct NativeExecutor {
    gate: ResourceGate,
    journal: PrivateJournal,
    deadline: Duration,
    capture_limit: usize,
    max_pending: usize,
    plans: Mutex<BTreeMap<String, NativeExecutionPlan>>,
    pending: Mutex<BTreeMap<String, PendingRun>>,
    reserved_runs: Mutex<BTreeSet<String>>,
    completed: Mutex<BTreeMap<String, ClosedExecution>>,
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
    refresh_attempts: u32,
    refresh_failed: bool,
}

/// Opaque, bounded, process-local evidence. Only actual owned service closure
/// can create this value. Handoff is one-shot and does not authorize acceptance.
pub struct ClosedExecution {
    receipt: RuntimeReceipt,
    closure: ClosedAdmission,
    prepared_workspace: PreparedWorkspace,
    output: Vec<u8>,
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
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingDiagnostics {
    pub refresh_attempts: u32,
    pub refresh_failed: bool,
    pub observed_identity_persisted: bool,
}

#[derive(Serialize)]
struct ClosedEvidence<'a> {
    schema_version: u32,
    recovery: &'a RecoveryIdentity,
    closure: &'a ClosedAdmission,
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

impl NativeExecutor {
    /// Create the production adapter during trusted startup. `journal_root`
    /// must already exist as a private mode-0700 directory owned by this user.
    pub fn new(
        gate: ResourceGate,
        journal_root: impl AsRef<Path>,
        deadline: Duration,
        capture_limit: usize,
        max_pending: usize,
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
            completed: Mutex::new(BTreeMap::new()),
        })
    }

    /// Install a plan from trusted server composition. Duplicate run identities
    /// fail closed, including after a previous plan was consumed.
    pub fn register_plan(&self, plan: NativeExecutionPlan) -> Result<(), NativeExecutorError> {
        if !valid_run_id(&plan.run_id) || plan.launch_spec.run_id != plan.run_id {
            return Err(NativeExecutorError::InvalidBinding);
        }
        if self
            .journal
            .exists(&journal_name(&plan.run_id, "prepared")?)?
        {
            return Err(NativeExecutorError::DuplicateRun);
        }
        let mut plans = self
            .plans
            .lock()
            .map_err(|_| NativeExecutorError::Poisoned)?;
        let mut reserved = self
            .reserved_runs
            .lock()
            .map_err(|_| NativeExecutorError::Poisoned)?;
        if reserved.contains(&plan.run_id) {
            return Err(NativeExecutorError::DuplicateRun);
        }
        if reserved.len() >= self.max_pending {
            return Err(NativeExecutorError::Capacity);
        }
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
        if entry.running.observation().is_some() && !entry.observed_persisted {
            entry.recovery = entry.running.recovery_identity().clone();
            self.journal
                .write_same_or_new(&journal_name(run_id, "observed")?, &entry.recovery)?;
            entry.observed_persisted = true;
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
        let output = match collect_output(
            &entry.host,
            run_id,
            status.output_offset,
            self.capture_limit,
        ) {
            Ok(bytes) => bytes,
            Err(_) => return Ok(None),
        };
        let closure = match prove_closed(&mut entry.running, &entry.host) {
            Ok(proof) => proof,
            Err(_) => return Ok(None),
        };
        let output_digest = content_digest(&output).map_err(|_| NativeExecutorError::Digest)?;
        let outcome = write_closed_evidence(
            &self.journal,
            &entry.recovery,
            &closure,
            &output_digest,
            output.len() as u64,
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
                output,
            },
        );
        // Dropping this run's SessionHost is safe only after proven closure.
        Ok(Some(receipt))
    }

    fn launch_plan(
        &self,
        task: &TaskView,
        intent: &DispatchIntent,
    ) -> Result<RuntimeReceipt, NativeExecutorError> {
        let plan = self
            .plans
            .lock()
            .map_err(|_| NativeExecutorError::Poisoned)?
            .remove(&intent.run_id)
            .ok_or(NativeExecutorError::NoPreparedPlan)?;
        let result = self.launch_registered_plan(task, intent, plan);
        if result.is_err()
            && !self
                .pending
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .contains_key(&intent.run_id)
        {
            self.reserved_runs
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&intent.run_id);
        }
        result
    }

    fn launch_registered_plan(
        &self,
        task: &TaskView,
        intent: &DispatchIntent,
        plan: NativeExecutionPlan,
    ) -> Result<RuntimeReceipt, NativeExecutorError> {
        validate_bindings(task, intent, &plan)?;
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
        write_prelaunch_recovery(&self.journal, &recovery, intent, &plan)?;
        let host = SessionHost::new();
        let running = match admitted.launch(&host) {
            Ok(running) => running,
            Err(failure) => {
                if let Some(running) = failure.recovery {
                    let recovery = running.recovery_identity().clone();
                    self.insert_pending(intent, host, *running, recovery, plan.prepared_workspace)?;
                }
                return Err(NativeExecutorError::LaunchUnknown);
            }
        };
        let recovery = running.recovery_identity().clone();
        self.insert_pending(intent, host, running, recovery, plan.prepared_workspace)?;
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
                refresh_attempts: 0,
                refresh_failed: false,
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
        })
    }
}

impl std::error::Error for NativeExecutorError {}

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

fn collect_output(
    host: &SessionHost,
    run_id: &str,
    output_len: u64,
    maximum: usize,
) -> Result<Vec<u8>, NativeExecutorError> {
    if output_len > maximum as u64 {
        return Err(NativeExecutorError::Capacity);
    }
    let mut output = Vec::with_capacity(output_len as usize);
    let mut cursor = 0;
    while cursor < output_len {
        let tail = host
            .tail(run_id, cursor, (maximum - cursor as usize).min(64 * 1024))
            .map_err(|_| NativeExecutorError::Journal)?;
        if tail.next_offset <= cursor || tail.next_offset > output_len {
            return Err(NativeExecutorError::Journal);
        }
        for chunk in tail.chunks {
            output.extend_from_slice(&chunk.bytes);
        }
        cursor = tail.next_offset;
    }
    if output.len() != output_len as usize {
        return Err(NativeExecutorError::Journal);
    }
    Ok(output)
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
        closure,
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
    let evidence_digest = content_digest(&evidence).map_err(|_| NativeExecutorError::Digest)?;
    root.write_same_or_new(&journal_name(&recovery.run_id, "closed")?, &evidence)?;
    Ok((format!("native-evidence:{evidence_digest}"), success))
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

    #[test]
    fn unconfigured_resource_gate_denies_before_child_spawn_and_keeps_marker_absent() {
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
        )
        .expect("executor");
        let mut plan = NativeExecutionPlan {
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
        executor.register_plan(plan).expect("prepared trusted plan");
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
    #[ignore]
    fn child_marker() {
        if let Some(path) = std::env::args().next_back() {
            let _ = fs::write(path, b"spawned");
        }
    }
}
