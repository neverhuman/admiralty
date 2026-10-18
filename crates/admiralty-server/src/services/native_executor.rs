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
    fs::{self, File, OpenOptions},
    io::Write,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
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
    host: SessionHost,
    journal_root: PathBuf,
    deadline: Duration,
    capture_limit: usize,
    max_pending: usize,
    plans: Mutex<BTreeMap<String, NativeExecutionPlan>>,
    pending: Mutex<BTreeMap<String, PendingRun>>,
    reserved_runs: Mutex<BTreeSet<String>>,
}

struct PendingRun {
    running: RunningAdmission,
    task_id: TaskId,
    attempt_id: AttemptId,
    intent: DispatchIntent,
    recovery: RecoveryIdentity,
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
        let journal_root = validate_private_root(journal_root.as_ref())?;
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
            host: SessionHost::new(),
            journal_root,
            deadline,
            capture_limit,
            max_pending,
            plans: Mutex::new(BTreeMap::new()),
            pending: Mutex::new(BTreeMap::new()),
            reserved_runs: Mutex::new(BTreeSet::new()),
        })
    }

    /// Install a plan from trusted server composition. Duplicate run identities
    /// fail closed, including after a previous plan was consumed.
    pub fn register_plan(&self, plan: NativeExecutionPlan) -> Result<(), NativeExecutorError> {
        if !valid_run_id(&plan.run_id) || plan.launch_spec.run_id != plan.run_id {
            return Err(NativeExecutorError::InvalidBinding);
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

    /// Reconcile a previously uncertain run. This never launches or signals a
    /// child. If status, closure, or bounded capture is not conclusive, the
    /// `RunningAdmission` stays in the private registry with its capacity held.
    pub fn observe_pending(
        &self,
        run_id: &str,
    ) -> Result<Option<RuntimeReceipt>, NativeExecutorError> {
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(entry) = pending.get_mut(run_id) else {
            return Ok(None);
        };
        let status = match self.host.status(run_id) {
            Ok(status) => status,
            Err(_) => return Ok(None),
        };
        if status.exit.is_none() || status.capture_state == CaptureState::Capturing {
            return Ok(None);
        }
        if status.capture_failed
            || status.capture_state != CaptureState::Complete
            || status.final_output_offset.is_none_or(|length| {
                length > self.capture_limit as u64 || length != status.output_offset
            })
        {
            return Ok(None);
        }

        let output =
            match collect_output(&self.host, run_id, status.output_offset, self.capture_limit) {
                Ok(output) => output,
                Err(_) => return Ok(None),
            };
        let closure = match entry.running.prove_closed(&self.host) {
            Ok(proof) => proof,
            Err(_) => return Ok(None),
        };
        let output_digest = content_digest(&output).map_err(|_| NativeExecutorError::Digest)?;
        let recovery = entry.recovery.clone();
        let outcome = write_closed_evidence(
            &self.journal_root,
            &recovery,
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
        pending.remove(run_id);
        self.reserved_runs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(run_id);
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
        write_prelaunch_recovery(&self.journal_root, &recovery)?;
        let running = match admitted.launch(&self.host) {
            Ok(running) => running,
            Err(failure) => {
                if let Some(running) = failure.recovery {
                    let recovery = running.recovery_identity().clone();
                    self.insert_pending(intent, task, *running, recovery)?;
                }
                return Err(NativeExecutorError::LaunchUnknown);
            }
        };
        self.insert_pending(intent, task, running, recovery)?;
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
        task: &TaskView,
        running: RunningAdmission,
        recovery: RecoveryIdentity,
    ) -> Result<(), NativeExecutorError> {
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !self
            .reserved_runs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(&intent.run_id)
            || pending.contains_key(&intent.run_id)
        {
            return Err(NativeExecutorError::Capacity);
        }
        pending.insert(
            intent.run_id.clone(),
            PendingRun {
                running,
                task_id: task.contract.task_id.clone(),
                attempt_id: AttemptId::new(intent.attempt_id.clone())
                    .map_err(|_| NativeExecutorError::InvalidBinding)?,
                intent: intent.clone(),
                recovery,
            },
        );
        Ok(())
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
    if intent.schema_version != 1
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

fn validate_private_root(path: &Path) -> Result<PathBuf, NativeExecutorError> {
    if !path.is_absolute() {
        return Err(NativeExecutorError::Journal);
    }
    let canonical = fs::canonicalize(path).map_err(|_| NativeExecutorError::Journal)?;
    if canonical != path {
        return Err(NativeExecutorError::Journal);
    }
    let mut prefix = PathBuf::new();
    for component in path.components() {
        match component {
            Component::RootDir => prefix.push(component.as_os_str()),
            Component::Normal(name) => {
                prefix.push(name);
                if fs::symlink_metadata(&prefix)
                    .map_err(|_| NativeExecutorError::Journal)?
                    .file_type()
                    .is_symlink()
                {
                    return Err(NativeExecutorError::Journal);
                }
            }
            _ => return Err(NativeExecutorError::Journal),
        }
    }
    let metadata = fs::symlink_metadata(path).map_err(|_| NativeExecutorError::Journal)?;
    let mode = metadata.permissions().mode();
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || mode & 0o077 != 0
    {
        return Err(NativeExecutorError::Journal);
    }
    Ok(canonical)
}

fn write_prelaunch_recovery(
    root: &Path,
    recovery: &RecoveryIdentity,
) -> Result<(), NativeExecutorError> {
    let id = content_digest(&recovery.run_id).map_err(|_| NativeExecutorError::Digest)?;
    let path = root.join(format!("{id}.prepared.json"));
    write_create_only(&path, recovery)
}

fn write_closed_evidence(
    root: &Path,
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
    let id = content_digest(&recovery.run_id).map_err(|_| NativeExecutorError::Digest)?;
    let path = root.join(format!("{id}.closed.json"));
    write_create_only(&path, &evidence)?;
    Ok((format!("native-evidence:{evidence_digest}"), success))
}

fn write_create_only<T: Serialize>(path: &Path, value: &T) -> Result<(), NativeExecutorError> {
    let bytes = serde_json::to_vec(value).map_err(|_| NativeExecutorError::Journal)?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true).mode(0o600);
    let mut file = options
        .open(path)
        .map_err(|_| NativeExecutorError::Journal)?;
    file.write_all(&bytes)
        .map_err(|_| NativeExecutorError::Journal)?;
    file.sync_all().map_err(|_| NativeExecutorError::Journal)?;
    drop(file);
    File::open(path.parent().ok_or(NativeExecutorError::Journal)?)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| NativeExecutorError::Journal)
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
    use std::{collections::BTreeSet, fs, os::unix::fs::PermissionsExt};

    struct CurrentPermit;
    impl ResourcePermitAuthority for CurrentPermit {
        fn is_current(&self, _claims: &ResourcePermitClaims) -> bool {
            true
        }
    }

    #[test]
    fn unconfigured_resource_gate_denies_before_child_spawn_and_keeps_marker_absent() {
        let temp = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target")
            .join(format!("native-executor-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(temp.parent().expect("target parent")).expect("lane target directory");
        fs::create_dir(&temp).expect("private fixture");
        fs::set_permissions(&temp, fs::Permissions::from_mode(0o700)).expect("private root");
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
            let output = std::process::Command::new("git")
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
        fs::set_permissions(source.join(".git"), fs::Permissions::from_mode(0o700))
            .expect("private source metadata");
        fs::set_permissions(
            source.join(".git/config"),
            fs::Permissions::from_mode(0o600),
        )
        .expect("private source config");
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
        let reservation = Reservation {
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
        executor
            .register_plan(NativeExecutionPlan {
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
            })
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
        fs::remove_dir_all(temp).expect("remove private fixture");
    }

    #[test]
    #[ignore]
    fn child_marker() {
        if let Some(path) = std::env::args().next_back() {
            let _ = fs::write(path, b"spawned");
        }
    }
}
