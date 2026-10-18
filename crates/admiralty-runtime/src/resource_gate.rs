//! Linux user-systemd resource admission; not a filesystem/credential sandbox.
//!
//! Trusted startup supplies the local bus, existing delegated slice, closed task
//! environment and an authenticated permit issuer. Default construction cannot
//! launch. Persist `recovery_identity()` before launch in the caller's durable
//! admission transaction. In-process slots are not a durable capacity ledger.
//! Uncertain launches retain custody and return their recovery handle; this
//! module never signals a service or historical PID. Native qualification is a
//! separate real-backend gate.
use crate::session::{LaunchSpec, SessionError, SessionHost, validate_launch_envelope};
use admiralty_core::{AttemptId, ProjectId, TaskId, content_digest, nodes::NodeId};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    fs,
    io::{self, Read},
    os::unix::fs::{FileTypeExt, MetadataExt},
    path::{Component, Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use thiserror::Error;

const CGROUP_ROOT: &str = "/sys/fs/cgroup";
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const OUTPUT_LIMIT: usize = 64 * 1024;
const OBSERVATION_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct AdmissionIdentity {
    pub project_id: ProjectId,
    pub task_id: TaskId,
    pub attempt_id: AttemptId,
    pub node_id: NodeId,
    pub lease_id: String,
}

/// Claims are not a grant. Only the configured issuer may construct the opaque
/// permit below, after signature/reservation/authority-generation validation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourcePermitClaims {
    pub issuer_ref: String,
    pub permit_id: String,
    pub reservation_ref: String,
    pub identity: AdmissionIdentity,
    pub host_incarnation: String,
    pub expires_at_unix_ms: u64,
    pub lease_expires_at_unix_ms: u64,
    pub registered_cpu_scope: BTreeSet<u32>,
    pub cpu_set: BTreeSet<u32>,
    pub memory_max_bytes: u64,
    pub max_tasks: u32,
    pub max_concurrency: u32,
    pub launch_spec_digest: String,
    pub worker_environment_digest: String,
    pub backend_config_digest: String,
}

/// Trusted startup adapter; never selected or instantiated by agent JSON.
/// Must recheck current revocation, reservation, lease and authority generation
/// without blocking. A string/digest comparison alone is not authentication.
pub trait ResourcePermitAuthority: Send + Sync {
    fn is_current(&self, claims: &ResourcePermitClaims) -> bool;
}
#[derive(Clone)]
pub struct VerifiedResourcePermit {
    claims: ResourcePermitClaims,
    authority: Arc<dyn ResourcePermitAuthority>,
}
impl VerifiedResourcePermit {
    /// Explicit trusted startup composition seam. The configured authority must
    /// authenticate signature/reservation/generation now; prepare and launch
    /// recheck it. Agent JSON alone cannot create this value.
    pub fn verify_current(
        claims: ResourcePermitClaims,
        authority: Arc<dyn ResourcePermitAuthority>,
    ) -> Result<Self, ResourceGateError> {
        if !authority.is_current(&claims) {
            return Err(ResourceGateError::PermitUnavailable);
        }
        Ok(Self { claims, authority })
    }
    #[allow(dead_code)] // Centralized authenticated issuer integration is separate.
    pub(crate) fn from_authenticated_issuer(
        claims: ResourcePermitClaims,
        authority: Arc<dyn ResourcePermitAuthority>,
    ) -> Self {
        Self { claims, authority }
    }
}

/// Explicit local user-bus identity and an ALREADY active delegated slice.
/// No SSH cgroup, environment variable, username or host is inferred.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SystemdBackendConfig {
    pub runtime_directory: PathBuf,
    pub delegated_slice: String,
}
impl SystemdBackendConfig {
    pub fn digest(&self) -> Result<String, ResourceGateError> {
        content_digest(&(
            self.runtime_directory
                .to_str()
                .ok_or(ResourceGateError::IdentityMismatch)?,
            &self.delegated_slice,
        ))
        .map_err(|_| ResourceGateError::IdentityMismatch)
    }
}

/// Explicit nonsecret environment only. Values become service argv/manifest
/// data; credentials must use a separately qualified private delivery boundary.
/// `env -i` removes ALL user-manager environment before exec of the task.
#[derive(Clone)]
pub struct QualifiedEnvironment(BTreeMap<String, String>);
impl QualifiedEnvironment {
    pub fn new(values: BTreeMap<String, String>) -> Result<Self, ResourceGateError> {
        const KEYS: &[&str] = &[
            "PATH",
            "HOME",
            "TMPDIR",
            "LANG",
            "LC_ALL",
            "TZ",
            "XDG_CACHE_HOME",
            "XDG_CONFIG_HOME",
            "CARGO_HOME",
            "RUSTUP_HOME",
        ];
        if values.len() > KEYS.len()
            || ["PATH", "HOME", "TMPDIR"]
                .iter()
                .any(|k| !values.contains_key(*k))
            || values.iter().any(|(k, v)| {
                !KEYS.contains(&k.as_str())
                    || v.is_empty()
                    || v.len() > 4096
                    || v.chars().any(char::is_control)
            })
        {
            return Err(ResourceGateError::InvalidEnvironment);
        }
        for (key, value) in &values {
            if key == "PATH" {
                if value.split(':').any(|p| !portable_absolute(Path::new(p))) {
                    return Err(ResourceGateError::InvalidEnvironment);
                }
            } else if matches!(
                key.as_str(),
                "HOME"
                    | "TMPDIR"
                    | "XDG_CACHE_HOME"
                    | "XDG_CONFIG_HOME"
                    | "CARGO_HOME"
                    | "RUSTUP_HOME"
            ) && !portable_absolute(Path::new(value))
            {
                return Err(ResourceGateError::InvalidEnvironment);
            }
        }
        Ok(Self(values))
    }
    pub fn digest(&self) -> Result<String, ResourceGateError> {
        content_digest(&self.0).map_err(|_| ResourceGateError::IdentityMismatch)
    }
}

#[derive(Debug, Error)]
pub enum ResourceGateError {
    #[error("resource permit is unavailable or no longer current")]
    PermitUnavailable,
    #[error("permit identity or exact launch/environment/backend binding differs")]
    IdentityMismatch,
    #[error("permit or lease is expired")]
    Expired,
    #[error("capacity is invalid or reserved concurrency is exhausted")]
    CapacityUnavailable,
    #[error("CPU set is outside registered, inherited or effective scope")]
    CpuScope,
    #[error("explicit nonsecret worker environment is invalid")]
    InvalidEnvironment,
    #[error("configured systemd cgroup backend unavailable: {0}")]
    UnsupportedBackend(&'static str),
    #[error("observed launch/enforcement differs: {0}")]
    EnforcementMismatch(&'static str),
    #[error("service outcome remains unknown; retain recovery identity")]
    UnknownOutcome,
    #[error("session host failed: {0}")]
    Session(#[from] SessionError),
    #[error("resource gate I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("resource gate lock was poisoned")]
    Poisoned,
}

/// Bounded diagnostic labels, never environment/argv values, paths, or raw
/// backend error text. Diagnostics confer no launch or closure authority.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct ObservationFailure {
    pub stage: &'static str,
    pub kind: &'static str,
    pub io_kind: Option<&'static str>,
}

/// One bounded numeric observation, for private failure receipts only. CPU
/// prefixes are truncated to 16 entries; counts retain the complete set size.
#[derive(Clone, Debug, serde::Serialize)]
pub struct ObservedCgroupLimits {
    pub cpu_count: usize,
    pub cpu_prefix: Vec<u32>,
    pub expected_cpu_count: usize,
    pub expected_cpu_prefix: Vec<u32>,
    pub memory_max_bytes: u64,
    pub expected_memory_max_bytes: u64,
    pub tasks_max: u64,
    pub expected_tasks_max: u32,
    pub main_pid_present: bool,
    pub process_count: usize,
    pub pids_current: Option<u64>,
}

#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct ObservationDiagnostics {
    pub attempts: u32,
    pub first_failure: Option<ObservationFailure>,
    pub first_significant_failure: Option<ObservationFailure>,
    pub last_failure: Option<ObservationFailure>,
    pub first_cgroup_limits: Option<ObservedCgroupLimits>,
}

impl ObservationDiagnostics {
    fn record(&mut self, stage: &'static str, error: Option<&ResourceGateError>) {
        self.attempts = self.attempts.saturating_add(1);
        let Some(error) = error else { return };
        let kind = match error {
            ResourceGateError::UnknownOutcome => "unknown_outcome",
            ResourceGateError::EnforcementMismatch(_) => "enforcement_mismatch",
            ResourceGateError::Io(_) => "io",
            ResourceGateError::CpuScope => "cpu_scope",
            ResourceGateError::IdentityMismatch => "identity_mismatch",
            ResourceGateError::InvalidEnvironment => "invalid_environment",
            ResourceGateError::UnsupportedBackend(_) => "unsupported_backend",
            ResourceGateError::Expired => "expired",
            ResourceGateError::PermitUnavailable => "permit_unavailable",
            ResourceGateError::CapacityUnavailable => "capacity_unavailable",
            ResourceGateError::Session(_) => "session",
            ResourceGateError::Poisoned => "poisoned",
        };
        let io_kind = match error {
            ResourceGateError::Io(error) => Some(match error.kind() {
                io::ErrorKind::NotFound => "not_found",
                io::ErrorKind::PermissionDenied => "permission_denied",
                io::ErrorKind::InvalidData => "invalid_data",
                io::ErrorKind::TimedOut => "timed_out",
                _ => "other",
            }),
            _ => None,
        };
        let failure = ObservationFailure {
            stage,
            kind,
            io_kind,
        };
        self.first_failure.get_or_insert_with(|| failure.clone());
        if !matches!(error, ResourceGateError::UnknownOutcome)
            || matches!(
                stage,
                "unit_identity"
                    | "unit_slice"
                    | "process_start_recheck"
                    | "systemd_identity_recheck"
            )
        {
            self.first_significant_failure
                .get_or_insert_with(|| failure.clone());
        }
        self.last_failure = Some(failure);
    }
}
#[derive(Default)]
struct GateState {
    active_by_permit: BTreeMap<String, u32>,
}
#[derive(Clone)]
pub struct ResourceGate {
    node_id: NodeId,
    configured: Option<(SystemdBackendConfig, QualifiedEnvironment)>,
    state: Arc<Mutex<GateState>>,
}
impl ResourceGate {
    pub fn new(node_id: NodeId) -> Self {
        Self {
            node_id,
            configured: None,
            state: Arc::new(Mutex::new(GateState::default())),
        }
    }
    pub fn configured(
        node_id: NodeId,
        config: SystemdBackendConfig,
        environment: QualifiedEnvironment,
    ) -> Self {
        Self {
            node_id,
            configured: Some((config, environment)),
            state: Arc::new(Mutex::new(GateState::default())),
        }
    }
    pub fn prepare(
        &self,
        permit: VerifiedResourcePermit,
        identity: AdmissionIdentity,
        spec: LaunchSpec,
    ) -> Result<AdmittedLaunch, ResourceGateError> {
        let (config, environment) =
            self.configured
                .as_ref()
                .ok_or(ResourceGateError::UnsupportedBackend(
                    "explicit local bus, delegated slice and environment required",
                ))?;
        validate_launch_envelope(&spec)?;
        validate_claims(
            &permit.claims,
            &identity,
            &spec,
            &self.node_id,
            config,
            environment,
            unix_time_ms()?,
            &read_host_incarnation()?,
            &read_current_cpu_set()?,
        )?;
        if !permit.authority.is_current(&permit.claims) {
            return Err(ResourceGateError::PermitUnavailable);
        }
        let backend = SystemdCgroupBackend::discover(config.clone())?;
        backend.validate_parent_scope(&permit.claims.cpu_set)?;
        validate_task_paths(&spec)?;
        let unit = fresh_unit()?;
        backend.require_absent(&unit)?;
        let mut state = self.state.lock().map_err(|_| ResourceGateError::Poisoned)?;
        let active = state
            .active_by_permit
            .entry(permit.claims.permit_id.clone())
            .or_default();
        if *active >= permit.claims.max_concurrency {
            return Err(ResourceGateError::CapacityUnavailable);
        }
        *active += 1;
        let slot = CapacitySlot {
            state: Arc::clone(&self.state),
            permit_id: permit.claims.permit_id.clone(),
            retained: false,
        };
        Ok(AdmittedLaunch {
            permit,
            identity,
            spec,
            environment: environment.clone(),
            backend,
            unit,
            slot: Some(slot),
        })
    }
}

/// Persist this identity in the caller's durable admission journal BEFORE exec.
/// It is observational data, not a constructor for authenticated permits.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct RecoveryIdentity {
    pub identity: AdmissionIdentity,
    pub launch_spec_digest: String,
    pub worker_environment_digest: String,
    pub unit: String,
    pub run_id: String,
    pub permit_id: String,
    pub reservation_ref: String,
    pub host_incarnation: String,
    pub backend_config_digest: String,
    pub invocation_id: Option<String>,
    pub main_pid: Option<u32>,
    pub process_start: Option<String>,
}
pub struct AdmittedLaunch {
    permit: VerifiedResourcePermit,
    identity: AdmissionIdentity,
    spec: LaunchSpec,
    environment: QualifiedEnvironment,
    backend: SystemdCgroupBackend,
    unit: String,
    slot: Option<CapacitySlot>,
}
/// An uncertain post-spawn failure keeps its recovery handle and capacity slot.
/// Do not discard `recovery` or turn the error into a retryable launch failure.
pub struct LaunchFailure {
    pub error: ResourceGateError,
    pub recovery: Option<Box<RunningAdmission>>,
}
impl std::fmt::Debug for LaunchFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LaunchFailure")
            .field("error", &self.error)
            .field(
                "recovery",
                &self.recovery.as_ref().map(|h| h.recovery_identity()),
            )
            .finish()
    }
}
impl std::fmt::Display for LaunchFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(handle) = &self.recovery {
            let diagnostics = handle.observation_diagnostics();
            if let Some(failure) = diagnostics
                .first_significant_failure
                .as_ref()
                .or(diagnostics.first_failure.as_ref())
            {
                return write!(
                    f,
                    "resource observation failed: stage={} kind={} io_kind={} attempts={}",
                    failure.stage,
                    failure.kind,
                    failure.io_kind.unwrap_or("none"),
                    diagnostics.attempts,
                );
            }
        }
        self.error.fmt(f)
    }
}
impl std::error::Error for LaunchFailure {}
impl AdmittedLaunch {
    pub fn recovery_identity(&self) -> RecoveryIdentity {
        recovery_identity(&self.unit, &self.spec, &self.permit.claims)
    }
    pub fn launch(mut self, host: &SessionHost) -> Result<RunningAdmission, LaunchFailure> {
        let preflight = (|| {
            let now = unix_time_ms()?;
            validate_claims(
                &self.permit.claims,
                &self.identity,
                &self.spec,
                &self.identity.node_id,
                &self.backend.config,
                &self.environment,
                now,
                &read_host_incarnation()?,
                &read_current_cpu_set()?,
            )?;
            if !self.permit.authority.is_current(&self.permit.claims) {
                return Err(ResourceGateError::PermitUnavailable);
            }
            self.backend = SystemdCgroupBackend::discover(self.backend.config.clone())?;
            self.backend
                .validate_parent_scope(&self.permit.claims.cpu_set)?;
            self.backend.require_absent(&self.unit)?;
            validate_task_paths(&self.spec)?;
            self.backend.wrap(
                &self.spec,
                &self.environment,
                &self.permit.claims,
                &self.unit,
                unix_time_ms()?,
            )
        })();
        let wrapped = preflight.map_err(|error| LaunchFailure {
            error,
            recovery: None,
        })?;
        let mut handle = RunningAdmission {
            recovery: self.recovery_identity(),
            backend: self.backend,
            claims: self.permit.claims,
            original: self.spec,
            environment: self.environment,
            observation: None,
            observation_diagnostics: ObservationDiagnostics::default(),
            slot: self.slot.take(),
        };
        // Set before calling SessionHost: it can fail after spawning the client.
        if let Some(slot) = handle.slot.as_mut() {
            slot.retained = true;
        }
        let result = host.launch_with_environment(wrapped, &handle.backend.client_environment());
        if let Err(error) = result {
            return Err(LaunchFailure {
                error: ResourceGateError::Session(error),
                recovery: Some(Box::new(handle)),
            });
        }
        let deadline = Instant::now() + OBSERVATION_TIMEOUT;
        loop {
            match handle.backend.observe(
                &handle.recovery.unit,
                &handle.claims,
                &handle.original,
                &handle.environment,
                &mut handle.observation_diagnostics,
            ) {
                Ok(observation) => {
                    handle.recovery.invocation_id = Some(observation.invocation_id.clone());
                    handle.recovery.main_pid = Some(observation.main_pid);
                    handle.recovery.process_start = Some(observation.process_start.clone());
                    handle.observation = Some(observation);
                    return Ok(handle);
                }
                Err(_) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
                Err(error) => {
                    return Err(LaunchFailure {
                        error,
                        recovery: Some(Box::new(handle)),
                    });
                }
            }
        }
    }
}
fn recovery_identity(
    unit: &str,
    spec: &LaunchSpec,
    claims: &ResourcePermitClaims,
) -> RecoveryIdentity {
    RecoveryIdentity {
        identity: claims.identity.clone(),
        launch_spec_digest: claims.launch_spec_digest.clone(),
        worker_environment_digest: claims.worker_environment_digest.clone(),
        unit: unit.into(),
        run_id: spec.run_id.clone(),
        permit_id: claims.permit_id.clone(),
        reservation_ref: claims.reservation_ref.clone(),
        host_incarnation: claims.host_incarnation.clone(),
        backend_config_digest: claims.backend_config_digest.clone(),
        invocation_id: None,
        main_pid: None,
        process_start: None,
    }
}
#[derive(Clone, Debug, serde::Serialize)]
pub struct EnforcementObservation {
    pub unit: String,
    pub invocation_id: String,
    pub main_pid: u32,
    pub process_start: String,
    pub process_ids: Vec<u32>,
    pub cpu_set: BTreeSet<u32>,
    pub memory_max_bytes: u64,
    pub tasks_max: u32,
    pub actual_executable: PathBuf,
    pub actual_cwd: PathBuf,
    pub actual_argv_digest: String,
    pub actual_environment_digest: String,
    pub cgroup_path: PathBuf,
}
/// Actual owned service closure, minted only after live reconciliation. This
/// serialization-only value cannot be reconstructed from an agent's JSON.
#[derive(Clone, Debug, serde::Serialize)]
pub struct ClosedAdmission {
    identity: RecoveryIdentity,
    observation: EnforcementObservation,
    exit_code: Option<i32>,
    success: bool,
    closure_digest: String,
}
impl ClosedAdmission {
    pub fn identity(&self) -> &RecoveryIdentity {
        &self.identity
    }
    pub fn observation(&self) -> &EnforcementObservation {
        &self.observation
    }
    pub fn exit_code(&self) -> Option<i32> {
        self.exit_code
    }
    pub fn success(&self) -> bool {
        self.success
    }
    pub fn closure_digest(&self) -> &str {
        &self.closure_digest
    }
}
pub struct RunningAdmission {
    recovery: RecoveryIdentity,
    backend: SystemdCgroupBackend,
    claims: ResourcePermitClaims,
    original: LaunchSpec,
    environment: QualifiedEnvironment,
    observation: Option<EnforcementObservation>,
    observation_diagnostics: ObservationDiagnostics,
    slot: Option<CapacitySlot>,
}
impl RunningAdmission {
    pub fn observation_diagnostics(&self) -> &ObservationDiagnostics {
        &self.observation_diagnostics
    }
    /// A proof is produced only from this handle's real, owned Linux service.
    /// Unknown/missing observations and failed probes cannot mint closure.
    #[cfg(target_os = "linux")]
    pub fn prove_closed(
        &mut self,
        host: &SessionHost,
    ) -> Result<ClosedAdmission, ResourceGateError> {
        let exit = self.reconcile_complete(host)?;
        let observation = self
            .observation
            .clone()
            .ok_or(ResourceGateError::UnknownOutcome)?;
        let closure_digest = content_digest(&(
            "closed-admission-v1",
            &self.recovery,
            &observation,
            exit.code(),
            exit.success(),
        ))
        .map_err(|_| ResourceGateError::IdentityMismatch)?;
        Ok(ClosedAdmission {
            identity: self.recovery.clone(),
            observation,
            exit_code: exit.code(),
            success: exit.success(),
            closure_digest,
        })
    }
    pub fn observation(&self) -> Option<&EnforcementObservation> {
        self.observation.as_ref()
    }
    pub fn recovery_identity(&self) -> &RecoveryIdentity {
        &self.recovery
    }
    /// Retry identity observation after an uncertain startup. This does not
    /// authorize a new launch, signal a process, or release capacity.
    pub fn refresh_observation(&mut self) -> Result<&EnforcementObservation, ResourceGateError> {
        if read_host_incarnation()? != self.recovery.host_incarnation {
            return Err(ResourceGateError::UnknownOutcome);
        }
        let observation = self.backend.observe(
            &self.recovery.unit,
            &self.claims,
            &self.original,
            &self.environment,
            &mut self.observation_diagnostics,
        )?;
        if self
            .recovery
            .invocation_id
            .as_ref()
            .is_some_and(|id| id != &observation.invocation_id)
            || self
                .recovery
                .main_pid
                .is_some_and(|pid| pid != observation.main_pid)
            || self
                .recovery
                .process_start
                .as_ref()
                .is_some_and(|start| start != &observation.process_start)
        {
            return Err(ResourceGateError::UnknownOutcome);
        }
        self.recovery.invocation_id = Some(observation.invocation_id.clone());
        self.recovery.main_pid = Some(observation.main_pid);
        self.recovery.process_start = Some(observation.process_start.clone());
        self.observation = Some(observation);
        self.observation
            .as_ref()
            .ok_or(ResourceGateError::UnknownOutcome)
    }
    /// Does not consume an uncertain handle. No failed probe, missing service,
    /// or wrapper exit alone can release a reservation. Fast services that exit
    /// before identity observation require separate durable reconciliation.
    pub fn reconcile_complete(
        &mut self,
        host: &SessionHost,
    ) -> Result<ExitStatus, ResourceGateError> {
        let observation = self
            .observation
            .as_ref()
            .ok_or(ResourceGateError::UnknownOutcome)?;
        if read_host_incarnation()? != self.recovery.host_incarnation
            || host.status(&self.recovery.run_id)?.exit.is_none()
        {
            return Err(ResourceGateError::UnknownOutcome);
        }
        let service = self.backend.show(&self.recovery.unit);
        let absent = match service {
            Ok(ref properties) => {
                if field(properties, "InvocationID")? != observation.invocation_id
                    || !matches!(field(properties, "ActiveState")?, "inactive" | "failed")
                    || !no_pending_job(properties)?
                {
                    return Err(ResourceGateError::UnknownOutcome);
                }
                false
            }
            Err(_) => {
                self.backend.require_absent(&self.recovery.unit)?;
                true
            }
        };
        // The previously observed owned kernel cgroup must be empty or gone.
        // Missing files are accepted only here, with positive manager absence or
        // exact inactive invocation plus an already exited owned client.
        match read_bounded(&observation.cgroup_path.join("cgroup.procs"), OUTPUT_LIMIT) {
            Ok(pids) if pids.is_empty() => {}
            Err(ResourceGateError::Io(e)) if e.kind() == io::ErrorKind::NotFound => {}
            _ => return Err(ResourceGateError::UnknownOutcome),
        }
        if absent {
            self.backend.require_absent(&self.recovery.unit)?;
        } else if self.backend.show(&self.recovery.unit)? != service? {
            return Err(ResourceGateError::UnknownOutcome);
        }
        if read_host_incarnation()? != self.recovery.host_incarnation {
            return Err(ResourceGateError::UnknownOutcome);
        }
        let exit = host.wait(&self.recovery.run_id)?;
        if let Some(slot) = self.slot.as_mut() {
            slot.release()?;
        }
        self.slot.take();
        Ok(exit)
    }
}
struct CapacitySlot {
    state: Arc<Mutex<GateState>>,
    permit_id: String,
    retained: bool,
}
impl CapacitySlot {
    fn release(&mut self) -> Result<(), ResourceGateError> {
        let mut state = self.state.lock().map_err(|_| ResourceGateError::Poisoned)?;
        if let Some(active) = state.active_by_permit.get_mut(&self.permit_id) {
            *active = active.saturating_sub(1);
            if *active == 0 {
                state.active_by_permit.remove(&self.permit_id);
            }
        }
        self.retained = true;
        Ok(())
    }
}
impl Drop for CapacitySlot {
    fn drop(&mut self) {
        if !self.retained {
            let _ = self.release();
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn validate_claims(
    claims: &ResourcePermitClaims,
    identity: &AdmissionIdentity,
    spec: &LaunchSpec,
    node_id: &NodeId,
    config: &SystemdBackendConfig,
    environment: &QualifiedEnvironment,
    now: u64,
    host: &str,
    inherited: &BTreeSet<u32>,
) -> Result<(), ResourceGateError> {
    if claims.identity != *identity
        || claims.identity.node_id != *node_id
        || claims.launch_spec_digest
            != content_digest(spec).map_err(|_| ResourceGateError::IdentityMismatch)?
        || claims.worker_environment_digest != environment.digest()?
        || claims.backend_config_digest != config.digest()?
        || !valid_identity(&claims.issuer_ref)
        || !valid_identity(&claims.permit_id)
        || !valid_identity(&claims.reservation_ref)
        || !valid_identity(&identity.lease_id)
        || claims.host_incarnation != host
    {
        return Err(ResourceGateError::IdentityMismatch);
    }
    if now >= claims.expires_at_unix_ms || now >= claims.lease_expires_at_unix_ms {
        return Err(ResourceGateError::Expired);
    }
    if claims.cpu_set.is_empty()
        || claims.cpu_set.len() > 4096
        || !claims.cpu_set.is_subset(&claims.registered_cpu_scope)
        || claims.memory_max_bytes == 0
        || claims.max_tasks == 0
        || claims.max_concurrency == 0
    {
        return Err(ResourceGateError::CapacityUnavailable);
    }
    if !claims.cpu_set.is_subset(inherited) {
        return Err(ResourceGateError::CpuScope);
    }
    Ok(())
}
#[derive(Clone)]
struct SystemdCgroupBackend {
    config: SystemdBackendConfig,
    systemd_run: PathBuf,
    systemctl: PathBuf,
    clean_env: PathBuf,
    slice_cgroup: PathBuf,
}
impl SystemdCgroupBackend {
    fn discover(config: SystemdBackendConfig) -> Result<Self, ResourceGateError> {
        if !cfg!(target_os = "linux") {
            return Err(ResourceGateError::UnsupportedBackend(
                "Linux cgroup v2 required",
            ));
        }
        validate_bus(&config)?;
        let mut backend = Self {
            config,
            systemd_run: trusted_executable("systemd-run")?,
            systemctl: trusted_executable("systemctl")?,
            clean_env: trusted_executable("env")?,
            slice_cgroup: PathBuf::new(),
        };
        let slice = backend.show(&backend.config.delegated_slice)?;
        if field(&slice, "Id")? != backend.config.delegated_slice
            || field(&slice, "LoadState")? != "loaded"
            || field(&slice, "ActiveState")? != "active"
        {
            return Err(ResourceGateError::UnsupportedBackend(
                "configured delegated slice must already be active",
            ));
        }
        backend.slice_cgroup = canonical_cgroup(field(&slice, "ControlGroup")?)?;
        let controllers = read_bounded(
            &backend.slice_cgroup.join("cgroup.controllers"),
            OUTPUT_LIMIT,
        )?;
        let text = std::str::from_utf8(&controllers)
            .map_err(|_| ResourceGateError::UnsupportedBackend("invalid cgroup controllers"))?;
        let names: BTreeSet<_> = text.split_ascii_whitespace().collect();
        if ["cpuset", "memory", "pids"]
            .iter()
            .any(|s| !names.contains(s))
        {
            return Err(ResourceGateError::UnsupportedBackend(
                "configured slice lacks required delegated controllers",
            ));
        }
        Ok(backend)
    }
    fn client_environment(&self) -> Vec<(OsString, OsString)> {
        [
            ("PATH", "/usr/bin:/bin".to_owned()),
            ("LANG", "C".to_owned()),
            ("LC_ALL", "C".to_owned()),
            ("HOME", "/nonexistent".to_owned()),
            (
                "XDG_RUNTIME_DIR",
                self.config.runtime_directory.to_string_lossy().into_owned(),
            ),
            (
                "DBUS_SESSION_BUS_ADDRESS",
                format!(
                    "unix:path={}",
                    dbus_escape(&self.config.runtime_directory.join("bus"))
                ),
            ),
            ("SYSTEMD_PAGER", "".to_owned()),
            ("SYSTEMD_COLORS", "0".to_owned()),
        ]
        .into_iter()
        .map(|(k, v)| (OsString::from(k), OsString::from(v)))
        .collect()
    }
    fn show(&self, unit: &str) -> Result<BTreeMap<String, String>, ResourceGateError> {
        let output = bounded_output(
            &self.systemctl,
            &[
                "--user",
                "--no-pager",
                "show",
                "--property=Id,LoadState,ActiveState,ControlGroup,InvocationID,MainPID,ExecMainPID,Job,Slice",
                "--",
                unit,
            ],
            &self.client_environment(),
            PROBE_TIMEOUT,
        )?;
        if !output.status.success() {
            return Err(ResourceGateError::UnknownOutcome);
        }
        parse_properties(&output.stdout)
    }
    fn require_absent(&self, unit: &str) -> Result<(), ResourceGateError> {
        // show(nonexistent) can exit nonzero, indistinguishable from a bus error.
        // A successful exact-name list with no rows is affirmative absence.
        let output = bounded_output(
            &self.systemctl,
            &[
                "--user",
                "--no-pager",
                "list-units",
                "--all",
                "--no-legend",
                "--plain",
                "--full",
                "--",
                unit,
            ],
            &self.client_environment(),
            PROBE_TIMEOUT,
        )?;
        if !output.status.success() || !output.stdout.is_empty() {
            return Err(ResourceGateError::UnknownOutcome);
        }
        Ok(())
    }
    fn validate_parent_scope(&self, cpus: &BTreeSet<u32>) -> Result<(), ResourceGateError> {
        let inherited = parse_cpu_set(&read_text(
            &self.slice_cgroup.join("cpuset.cpus.effective"),
        )?)?;
        if !cpus.is_subset(&inherited) {
            return Err(ResourceGateError::CpuScope);
        }
        for key in ["memory.max", "pids.max", "cgroup.subtree_control"] {
            read_bounded(&self.slice_cgroup.join(key), OUTPUT_LIMIT)?;
        }
        Ok(())
    }
    fn wrap(
        &self,
        spec: &LaunchSpec,
        environment: &QualifiedEnvironment,
        claims: &ResourcePermitClaims,
        unit: &str,
        now: u64,
    ) -> Result<LaunchSpec, ResourceGateError> {
        let ms = claims
            .expires_at_unix_ms
            .min(claims.lease_expires_at_unix_ms)
            .checked_sub(now)
            .filter(|n| *n > 0)
            .ok_or(ResourceGateError::Expired)?;
        let cpus = claims
            .cpu_set
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let mut args = vec![
            "--user".into(),
            "--quiet".into(),
            "--pipe".into(),
            "--wait".into(),
            "--expand-environment=no".into(),
            format!("--unit={unit}"),
            format!("--slice={}", self.config.delegated_slice),
            format!("--working-directory={}", spec.cwd.display()),
            format!("--property=AllowedCPUs={cpus}"),
            format!("--property=MemoryMax={}B", claims.memory_max_bytes),
            format!("--property=TasksMax={}", claims.max_tasks),
            format!("--property=RuntimeMaxSec={}.{:03}s", ms / 1000, ms % 1000),
            "--property=KillMode=control-group".into(),
            "--service-type=exec".into(),
            "--".into(),
            self.clean_env.to_string_lossy().into_owned(),
            "-i".into(),
        ];
        args.extend(environment.0.iter().map(|(k, v)| format!("{k}={v}")));
        args.push(spec.program.to_string_lossy().into_owned());
        args.extend(spec.args.clone());
        Ok(LaunchSpec {
            run_id: spec.run_id.clone(),
            program: self.systemd_run.clone(),
            args,
            cwd: spec.cwd.clone(),
            spool_dir: spec.spool_dir.clone(),
        })
    }
    fn service_cgroup(&self, path: &str) -> Result<PathBuf, ResourceGateError> {
        let group = canonical_cgroup(path)?;
        if group == self.slice_cgroup || !group.starts_with(&self.slice_cgroup) {
            return Err(ResourceGateError::EnforcementMismatch(
                "service outside configured slice",
            ));
        }
        Ok(group)
    }
    fn observe(
        &self,
        unit: &str,
        claims: &ResourcePermitClaims,
        spec: &LaunchSpec,
        environment: &QualifiedEnvironment,
        diagnostics: &mut ObservationDiagnostics,
    ) -> Result<EnforcementObservation, ResourceGateError> {
        let mut stage = "systemd_initial_show";
        let result = (|| {
            let first = self.show(unit)?;
            stage = "unit_identity";
            if field(&first, "Id")? != unit {
                return Err(ResourceGateError::UnknownOutcome);
            }
            stage = "unit_load_state";
            if field(&first, "LoadState")? != "loaded" {
                return Err(ResourceGateError::UnknownOutcome);
            }
            stage = "unit_active_state";
            if field(&first, "ActiveState")? != "active" {
                return Err(ResourceGateError::UnknownOutcome);
            }
            stage = "unit_slice";
            if field(&first, "Slice")? != self.config.delegated_slice {
                return Err(ResourceGateError::UnknownOutcome);
            }
            stage = "invocation_identity";
            let invocation = field(&first, "InvocationID")?;
            if invocation.len() != 32
                || !invocation.bytes().all(|b| b.is_ascii_hexdigit())
                || invocation.bytes().all(|b| b == b'0')
            {
                return Err(ResourceGateError::UnknownOutcome);
            }
            stage = "main_pid";
            let pid = field(&first, "MainPID")?
                .parse::<u32>()
                .ok()
                .filter(|p| *p > 0)
                .ok_or(ResourceGateError::UnknownOutcome)?;
            let proc = PathBuf::from(format!("/proc/{pid}"));
            stage = "process_start_initial";
            let start = process_start(&read_text(&proc.join("stat"))?)?;
            stage = "process_executable";
            let executable = fs::read_link(proc.join("exe"))?;
            stage = "process_working_directory";
            let cwd = fs::read_link(proc.join("cwd"))?;
            stage = "process_argv";
            let actual_argv = read_bounded(&proc.join("cmdline"), OUTPUT_LIMIT)?;
            stage = "process_environment_read";
            let actual_environment =
                parse_environment(&read_bounded(&proc.join("environ"), OUTPUT_LIMIT)?)?;
            stage = "process_environment_match";
            if actual_environment != environment.0 {
                return Err(ResourceGateError::EnforcementMismatch(
                    "actual worker environment differs from qualified environment",
                ));
            }
            stage = "qualified_executable";
            let expected_executable = fs::canonicalize(&spec.program)?;
            stage = "process_launch_match";
            if executable != expected_executable
                || cwd != spec.cwd
                || actual_argv != argv_bytes(spec)?
            {
                return Err(ResourceGateError::EnforcementMismatch(
                    "actual executable/cwd/argv differs from qualified launch",
                ));
            }
            stage = "service_cgroup";
            let group = self.service_cgroup(field(&first, "ControlGroup")?)?;
            stage = "process_cgroup_membership";
            let membership = read_text(&proc.join("cgroup"))?;
            let member = membership
                .lines()
                .find_map(|l| l.strip_prefix("0::"))
                .ok_or(ResourceGateError::UnknownOutcome)?;
            if canonical_cgroup(member)? != group {
                return Err(ResourceGateError::EnforcementMismatch(
                    "MainPID outside owned service cgroup",
                ));
            }
            stage = "effective_cpu_set";
            let cpus = parse_cpu_set(&read_text(&group.join("cpuset.cpus.effective"))?)?;
            stage = "effective_memory_limit";
            let memory = parse_limit(&read_text(&group.join("memory.max"))?)?;
            stage = "effective_task_limit";
            let tasks = parse_limit(&read_text(&group.join("pids.max"))?)?;
            stage = "cgroup_processes";
            let process_ids = parse_pids(&read_text(&group.join("cgroup.procs"))?)?;
            let mut observed_limits = ObservedCgroupLimits {
                cpu_count: cpus.len(),
                cpu_prefix: cpus.iter().copied().take(16).collect(),
                expected_cpu_count: claims.cpu_set.len(),
                expected_cpu_prefix: claims.cpu_set.iter().copied().take(16).collect(),
                memory_max_bytes: memory,
                expected_memory_max_bytes: claims.memory_max_bytes,
                tasks_max: tasks,
                expected_tasks_max: claims.max_tasks,
                main_pid_present: process_ids.contains(&pid),
                process_count: process_ids.len(),
                pids_current: None,
            };
            if let Some(failed_stage) =
                cgroup_limit_failure(&cpus, memory, tasks, &process_ids, pid, claims)
            {
                stage = failed_stage;
                diagnostics
                    .first_cgroup_limits
                    .get_or_insert(observed_limits);
                return Err(ResourceGateError::EnforcementMismatch(
                    "effective cgroup limits differ from permit",
                ));
            }
            stage = "cgroup_pids_current_read";
            let current_tasks =
                match read_text(&group.join("pids.current")).and_then(|text| parse_limit(&text)) {
                    Ok(current) => current,
                    Err(error) => {
                        diagnostics
                            .first_cgroup_limits
                            .get_or_insert(observed_limits);
                        return Err(error);
                    }
                };
            observed_limits.pids_current = Some(current_tasks);
            diagnostics
                .first_cgroup_limits
                .get_or_insert(observed_limits);
            stage = "cgroup_pids_current_match";
            if current_tasks > tasks {
                return Err(ResourceGateError::EnforcementMismatch(
                    "effective cgroup task usage exceeds task limit",
                ));
            }
            stage = "process_start_recheck";
            if process_start(&read_text(&proc.join("stat"))?)? != start {
                return Err(ResourceGateError::UnknownOutcome);
            }
            stage = "systemd_identity_recheck";
            if self.show(unit)? != first {
                return Err(ResourceGateError::UnknownOutcome);
            }
            Ok(EnforcementObservation {
                unit: unit.into(),
                invocation_id: invocation.into(),
                main_pid: pid,
                process_start: start,
                process_ids,
                cpu_set: cpus,
                memory_max_bytes: memory,
                tasks_max: u32::try_from(tasks).map_err(|_| ResourceGateError::UnknownOutcome)?,
                actual_executable: executable,
                actual_cwd: cwd,
                actual_argv_digest: content_digest(&actual_argv)
                    .map_err(|_| ResourceGateError::IdentityMismatch)?,
                actual_environment_digest: content_digest(&actual_environment)
                    .map_err(|_| ResourceGateError::IdentityMismatch)?,
                cgroup_path: group,
            })
        })();
        diagnostics.record(stage, result.as_ref().err());
        result
    }
}

/// Preserve the original enforcement order and exact comparisons while
/// identifying the particular mismatch; diagnostic labels never grant admission.
fn cgroup_limit_failure(
    cpus: &BTreeSet<u32>,
    memory: u64,
    tasks: u64,
    process_ids: &[u32],
    pid: u32,
    claims: &ResourcePermitClaims,
) -> Option<&'static str> {
    if cpus != &claims.cpu_set {
        Some("cgroup_cpu_set_match")
    } else if memory == 0 || memory > claims.memory_max_bytes {
        Some("cgroup_memory_limit_match")
    } else if tasks == 0 || tasks > u64::from(claims.max_tasks) {
        Some("cgroup_task_limit_match")
    } else if !process_ids.contains(&pid) {
        Some("cgroup_main_pid_match")
    } else {
        None
    }
}

fn no_pending_job(properties: &BTreeMap<String, String>) -> Result<bool, ResourceGateError> {
    // systemctl renders a missing job as an explicit empty value on the
    // qualified host; some versions render zero. Missing/malformed fields or
    // any actual job identity remain uncertain and never prove closure.
    Ok(matches!(field(properties, "Job")?, "" | "0"))
}

fn portable_absolute(path: &Path) -> bool {
    path.is_absolute()
        && path
            .to_str()
            .is_some_and(|s| s.len() <= 4096 && !s.chars().any(char::is_control))
        && path
            .components()
            .all(|c| matches!(c, Component::RootDir | Component::Normal(_)))
}
fn validate_task_paths(spec: &LaunchSpec) -> Result<(), ResourceGateError> {
    for p in [&spec.program, &spec.cwd] {
        if !portable_absolute(p) || fs::canonicalize(p)? != *p {
            return Err(ResourceGateError::IdentityMismatch);
        }
    }
    if !fs::metadata(&spec.program)?.is_file() || !fs::metadata(&spec.cwd)?.is_dir() {
        return Err(ResourceGateError::IdentityMismatch);
    }
    Ok(())
}
fn validate_bus(config: &SystemdBackendConfig) -> Result<(), ResourceGateError> {
    if !valid_identity(&config.delegated_slice)
        || !config.delegated_slice.ends_with(".slice")
        || !portable_absolute(&config.runtime_directory)
    {
        return Err(ResourceGateError::UnsupportedBackend(
            "invalid explicit slice/runtime directory",
        ));
    }
    no_symlinks(&config.runtime_directory)?;
    let metadata = fs::symlink_metadata(&config.runtime_directory)?;
    let uid = rustix::process::geteuid().as_raw();
    if !metadata.is_dir() || metadata.uid() != uid || metadata.mode() & 0o777 != 0o700 {
        return Err(ResourceGateError::UnsupportedBackend(
            "runtime directory is not private and owned",
        ));
    }
    let bus = fs::symlink_metadata(config.runtime_directory.join("bus"))?;
    if !bus.file_type().is_socket() || bus.uid() != uid {
        return Err(ResourceGateError::UnsupportedBackend(
            "configured bus is not an owned local socket",
        ));
    }
    Ok(())
}
fn no_symlinks(path: &Path) -> Result<(), ResourceGateError> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component);
        if fs::symlink_metadata(&current)?.file_type().is_symlink() {
            return Err(ResourceGateError::IdentityMismatch);
        }
    }
    Ok(())
}
fn canonical_cgroup(relative: &str) -> Result<PathBuf, ResourceGateError> {
    if !portable_absolute(Path::new(relative)) {
        return Err(ResourceGateError::UnknownOutcome);
    }
    let group = Path::new(CGROUP_ROOT).join(relative.trim_start_matches('/'));
    no_symlinks(&group)?;
    let canonical = fs::canonicalize(group)?;
    if canonical == Path::new(CGROUP_ROOT) || !canonical.starts_with(CGROUP_ROOT) {
        return Err(ResourceGateError::UnknownOutcome);
    }
    Ok(canonical)
}
fn valid_identity(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
}
fn fresh_unit() -> Result<String, ResourceGateError> {
    let mut bytes = [0u8; 16];
    fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(format!(
        "adm-resource-{}.service",
        bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
    ))
}
fn unix_time_ms() -> Result<u64, ResourceGateError> {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| ResourceGateError::Expired)?
            .as_millis(),
    )
    .map_err(|_| ResourceGateError::Expired)
}
fn read_host_incarnation() -> Result<String, ResourceGateError> {
    Ok(read_text(Path::new("/proc/sys/kernel/random/boot_id"))?
        .trim()
        .into())
}
fn read_current_cpu_set() -> Result<BTreeSet<u32>, ResourceGateError> {
    let text = read_text(Path::new("/proc/self/status"))?;
    parse_cpu_set(
        text.lines()
            .find_map(|l| l.strip_prefix("Cpus_allowed_list:"))
            .ok_or(ResourceGateError::CpuScope)?,
    )
}
fn parse_cpu_set(value: &str) -> Result<BTreeSet<u32>, ResourceGateError> {
    let mut cpus = BTreeSet::new();
    for field in value.trim().split(',') {
        let (a, b) = match field.split_once('-') {
            Some((a, b)) => (a.parse::<u32>(), b.parse::<u32>()),
            None => (field.parse::<u32>(), field.parse::<u32>()),
        };
        let (a, b) = (
            a.map_err(|_| ResourceGateError::CpuScope)?,
            b.map_err(|_| ResourceGateError::CpuScope)?,
        );
        if b < a || b.saturating_sub(a) >= 4096 {
            return Err(ResourceGateError::CpuScope);
        }
        for n in a..=b {
            if cpus.len() >= 4096 || !cpus.insert(n) {
                return Err(ResourceGateError::CpuScope);
            }
        }
    }
    if cpus.is_empty() {
        return Err(ResourceGateError::CpuScope);
    }
    Ok(cpus)
}
fn parse_limit(value: &str) -> Result<u64, ResourceGateError> {
    value
        .trim()
        .parse()
        .map_err(|_| ResourceGateError::EnforcementMismatch("missing or unbounded cgroup control"))
}
fn parse_pids(value: &str) -> Result<Vec<u32>, ResourceGateError> {
    let mut pids = Vec::new();
    for line in value.lines() {
        if pids.len() >= 4096 {
            return Err(ResourceGateError::UnknownOutcome);
        }
        pids.push(
            line.parse::<u32>()
                .ok()
                .filter(|p| *p > 0)
                .ok_or(ResourceGateError::UnknownOutcome)?,
        );
    }
    Ok(pids)
}
fn process_start(stat: &str) -> Result<String, ResourceGateError> {
    stat.rsplit_once(") ")
        .and_then(|(_, s)| s.split_ascii_whitespace().nth(19))
        .filter(|s| s.bytes().all(|b| b.is_ascii_digit()))
        .map(str::to_owned)
        .ok_or(ResourceGateError::UnknownOutcome)
}
fn argv_bytes(spec: &LaunchSpec) -> Result<Vec<u8>, ResourceGateError> {
    let program = spec
        .program
        .to_str()
        .ok_or(ResourceGateError::IdentityMismatch)?;
    let mut bytes = Vec::new();
    for arg in std::iter::once(program).chain(spec.args.iter().map(String::as_str)) {
        if arg.contains('\0') || bytes.len().saturating_add(arg.len() + 1) > OUTPUT_LIMIT {
            return Err(ResourceGateError::IdentityMismatch);
        }
        bytes.extend_from_slice(arg.as_bytes());
        bytes.push(0);
    }
    Ok(bytes)
}
fn parse_environment(bytes: &[u8]) -> Result<BTreeMap<String, String>, ResourceGateError> {
    if bytes.len() > OUTPUT_LIMIT || bytes.last() != Some(&0) {
        return Err(ResourceGateError::UnknownOutcome);
    }
    let mut environment = BTreeMap::new();
    for entry in bytes[..bytes.len() - 1].split(|b| *b == 0) {
        let entry = std::str::from_utf8(entry).map_err(|_| ResourceGateError::UnknownOutcome)?;
        let (key, value) = entry
            .split_once('=')
            .ok_or(ResourceGateError::UnknownOutcome)?;
        if environment.len() >= 32 || environment.insert(key.into(), value.into()).is_some() {
            return Err(ResourceGateError::UnknownOutcome);
        }
    }
    Ok(environment)
}
fn field<'a>(
    properties: &'a BTreeMap<String, String>,
    key: &str,
) -> Result<&'a str, ResourceGateError> {
    properties
        .get(key)
        .map(String::as_str)
        .ok_or(ResourceGateError::UnknownOutcome)
}
fn parse_properties(output: &[u8]) -> Result<BTreeMap<String, String>, ResourceGateError> {
    let text = std::str::from_utf8(output).map_err(|_| ResourceGateError::UnknownOutcome)?;
    let mut values = BTreeMap::new();
    for line in text.lines() {
        let (k, v) = line
            .split_once('=')
            .ok_or(ResourceGateError::UnknownOutcome)?;
        if !matches!(
            k,
            "Id" | "LoadState"
                | "ActiveState"
                | "ControlGroup"
                | "InvocationID"
                | "MainPID"
                | "ExecMainPID"
                | "Job"
                | "Slice"
        ) || values.insert(k.into(), v.into()).is_some()
        {
            return Err(ResourceGateError::UnknownOutcome);
        }
    }
    Ok(values)
}
fn trusted_executable(name: &str) -> Result<PathBuf, ResourceGateError> {
    for directory in ["/usr/bin", "/bin"] {
        let candidate = Path::new(directory).join(name);
        let Ok(canonical) = fs::canonicalize(candidate) else {
            continue;
        };
        no_symlinks(&canonical)?;
        let mut current = PathBuf::new();
        let mut trusted = true;
        for component in canonical.components() {
            current.push(component);
            let metadata = fs::symlink_metadata(&current)?;
            if metadata.uid() != 0 || metadata.mode() & 0o022 != 0 {
                trusted = false;
                break;
            }
        }
        let metadata = fs::metadata(&canonical)?;
        if trusted && metadata.is_file() && metadata.mode() & 0o111 != 0 {
            return Ok(canonical);
        }
    }
    Err(ResourceGateError::UnsupportedBackend(
        "trusted system executable unavailable",
    ))
}
fn dbus_escape(path: &Path) -> String {
    path.as_os_str()
        .as_encoded_bytes()
        .iter()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"_-/\\.".contains(b) {
                (*b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
fn read_bounded(path: &Path, limit: usize) -> Result<Vec<u8>, ResourceGateError> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags((rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(ResourceGateError::UnknownOutcome);
    }
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(ResourceGateError::UnknownOutcome);
    }
    Ok(bytes)
}
fn read_text(path: &Path) -> Result<String, ResourceGateError> {
    String::from_utf8(read_bounded(path, OUTPUT_LIMIT)?)
        .map_err(|_| ResourceGateError::UnknownOutcome)
}
struct ProbeChild(Child);
impl Drop for ProbeChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
struct BoundedOutput {
    status: ExitStatus,
    stdout: Vec<u8>,
}
fn bounded_output(
    program: &Path,
    args: &[&str],
    environment: &[(OsString, OsString)],
    timeout: Duration,
) -> Result<BoundedOutput, ResourceGateError> {
    let mut child = ProbeChild(
        Command::new(program)
            .args(args)
            .env_clear()
            .envs(environment.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?,
    );
    let mut pipe = child
        .0
        .stdout
        .take()
        .ok_or(ResourceGateError::UnknownOutcome)?;
    let flags = rustix::fs::fcntl_getfl(&pipe).map_err(io::Error::from)?;
    rustix::fs::fcntl_setfl(&pipe, flags | rustix::fs::OFlags::NONBLOCK)
        .map_err(io::Error::from)?;
    let deadline = Instant::now() + timeout;
    let mut bytes = Vec::new();
    let mut status = None;
    let mut eof = false;
    loop {
        let mut chunk = [0; 4096];
        match pipe.read(&mut chunk) {
            Ok(0) => eof = true,
            Ok(n) => {
                if bytes.len() + n > OUTPUT_LIMIT {
                    return Err(ResourceGateError::UnsupportedBackend(
                        "probe output exceeded bound",
                    ));
                }
                bytes.extend_from_slice(&chunk[..n]);
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        }
        if status.is_none() {
            status = child.0.try_wait()?;
        }
        if eof && let Some(status) = status {
            return Ok(BoundedOutput {
                status,
                stdout: bytes,
            });
        }
        if Instant::now() >= deadline {
            return Err(ResourceGateError::UnsupportedBackend(
                "probe deadline elapsed",
            ));
        }
        thread::sleep(Duration::from_millis(2));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cgroup_diagnostics_distinguish_every_enforcement_comparison() {
        let claims = claims();
        let check = |cpus: BTreeSet<u32>, memory, tasks, pids: &[u32]| {
            cgroup_limit_failure(&cpus, memory, tasks, pids, 42, &claims)
        };
        assert_eq!(check(BTreeSet::from([2]), 1024, 4, &[42]), None);
        // Stricter memory/task ceilings retain the original admission semantics.
        assert_eq!(check(BTreeSet::from([2]), 1, 1, &[42]), None);
        for cpus in [BTreeSet::from([1, 2]), BTreeSet::from([1]), BTreeSet::new()] {
            assert_eq!(check(cpus, 1024, 4, &[42]), Some("cgroup_cpu_set_match"));
        }
        for memory in [0, 1025] {
            assert_eq!(
                check(BTreeSet::from([2]), memory, 4, &[42]),
                Some("cgroup_memory_limit_match")
            );
        }
        for tasks in [0, 5] {
            assert_eq!(
                check(BTreeSet::from([2]), 1024, tasks, &[42]),
                Some("cgroup_task_limit_match")
            );
        }
        assert_eq!(
            check(BTreeSet::from([2]), 1024, 4, &[41]),
            Some("cgroup_main_pid_match")
        );
        // Preserve the previous first-failure order when multiple checks differ.
        assert_eq!(
            check(BTreeSet::new(), 0, 0, &[]),
            Some("cgroup_cpu_set_match")
        );
    }

    #[test]
    fn actual_systemctl_empty_job_format_means_no_pending_job_only_when_present() {
        let properties = parse_properties(b"Id=admiralty-native-probe.slice\nJob=\n")
            .expect("observed systemctl property format");
        assert!(no_pending_job(&properties).expect("explicit empty Job"));
        for value in ["0", "7", "0 ", "[0 /]", "(0, /)"] {
            let properties = BTreeMap::from([("Job".into(), value.into())]);
            assert_eq!(
                no_pending_job(&properties).expect("explicit Job property"),
                value == "0"
            );
        }
        assert!(no_pending_job(&BTreeMap::new()).is_err());
    }

    #[test]
    fn observation_trace_retains_first_significant_error_and_never_error_contents() {
        let mut diagnostics = ObservationDiagnostics::default();
        diagnostics.record(
            "systemd_initial_show",
            Some(&ResourceGateError::UnknownOutcome),
        );
        diagnostics.record(
            "process_environment_match",
            Some(&ResourceGateError::EnforcementMismatch(
                "private-environment-value",
            )),
        );
        for _ in 0..1000 {
            diagnostics.record(
                "process_environment_read",
                Some(&ResourceGateError::Io(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "private-token-path-and-value",
                ))),
            );
        }
        let encoded = serde_json::to_string(&diagnostics).expect("safe bounded diagnostic labels");
        assert!(encoded.contains("permission_denied"));
        assert!(!encoded.contains("private-environment-value"));
        assert!(!encoded.contains("private-token-path-and-value"));
        diagnostics.record(
            "systemd_identity_recheck",
            Some(&ResourceGateError::UnknownOutcome),
        );
        assert_eq!(diagnostics.attempts, 1003);
        assert_eq!(
            diagnostics
                .first_failure
                .as_ref()
                .expect("first error")
                .stage,
            "systemd_initial_show"
        );
        assert_eq!(
            diagnostics
                .first_significant_failure
                .as_ref()
                .expect("significant error")
                .stage,
            "process_environment_match"
        );
        assert_eq!(
            diagnostics
                .last_failure
                .as_ref()
                .expect("latest error")
                .stage,
            "systemd_identity_recheck"
        );
        assert!(
            serde_json::to_vec(&diagnostics)
                .expect("bounded trace")
                .len()
                < 1024
        );
    }
    fn environment() -> QualifiedEnvironment {
        QualifiedEnvironment::new(BTreeMap::from([
            ("PATH".into(), "/usr/bin:/bin".into()),
            ("HOME".into(), "/nonexistent".into()),
            ("TMPDIR".into(), "/tmp".into()),
        ]))
        .expect("closed env")
    }
    fn config() -> SystemdBackendConfig {
        SystemdBackendConfig {
            runtime_directory: "/run/user/example".into(),
            delegated_slice: "admiralty.slice".into(),
        }
    }
    fn spec() -> LaunchSpec {
        LaunchSpec {
            run_id: "resource-test".into(),
            program: "/usr/bin/true".into(),
            args: vec!["literal $PATH %n $(x)".into()],
            cwd: "/owned/work".into(),
            spool_dir: "/owned/spool".into(),
        }
    }
    fn identity() -> AdmissionIdentity {
        AdmissionIdentity {
            project_id: ProjectId::new("project").expect("id"),
            task_id: TaskId::new("task").expect("id"),
            attempt_id: AttemptId::new("attempt").expect("id"),
            node_id: NodeId::new("node").expect("id"),
            lease_id: "lease".into(),
        }
    }
    fn claims() -> ResourcePermitClaims {
        ResourcePermitClaims {
            issuer_ref: "issuer".into(),
            permit_id: "permit".into(),
            reservation_ref: "reservation".into(),
            identity: identity(),
            host_incarnation: "boot".into(),
            expires_at_unix_ms: 1000,
            lease_expires_at_unix_ms: 900,
            registered_cpu_scope: BTreeSet::from([1, 2, 3]),
            cpu_set: BTreeSet::from([2]),
            memory_max_bytes: 1024,
            max_tasks: 4,
            max_concurrency: 1,
            launch_spec_digest: content_digest(&spec()).expect("digest"),
            worker_environment_digest: environment().digest().expect("digest"),
            backend_config_digest: config().digest().expect("digest"),
        }
    }
    fn validate(
        c: &ResourcePermitClaims,
        now: u64,
        cpus: BTreeSet<u32>,
    ) -> Result<(), ResourceGateError> {
        validate_claims(
            c,
            &identity(),
            &spec(),
            &identity().node_id,
            &config(),
            &environment(),
            now,
            "boot",
            &cpus,
        )
    }
    #[test]
    fn permitted_cpu_subset_and_expiry_are_checked_at_launch_time() {
        assert!(validate(&claims(), 1, BTreeSet::from([1, 2, 3])).is_ok());
        assert!(matches!(
            validate(&claims(), 1, BTreeSet::from([1, 3])),
            Err(ResourceGateError::CpuScope)
        ));
        assert!(matches!(
            validate(&claims(), 900, BTreeSet::from([2])),
            Err(ResourceGateError::Expired)
        ));
        let mut changed = claims();
        changed.worker_environment_digest = "other".into();
        assert!(matches!(
            validate(&changed, 1, BTreeSet::from([2])),
            Err(ResourceGateError::IdentityMismatch)
        ));
        let mut changed = claims();
        changed.backend_config_digest = "other".into();
        assert!(validate(&changed, 1, BTreeSet::from([2])).is_err());
    }
    #[test]
    fn wrapper_sets_actual_working_directory_slice_clean_environment_and_remaining_time() {
        let backend = SystemdCgroupBackend {
            config: config(),
            systemd_run: "/usr/bin/systemd-run".into(),
            systemctl: "/usr/bin/systemctl".into(),
            clean_env: "/usr/bin/env".into(),
            slice_cgroup: "/sys/fs/cgroup/configured".into(),
        };
        let wrapped = backend
            .wrap(
                &spec(),
                &environment(),
                &claims(),
                "adm-resource-fresh.service",
                500,
            )
            .expect("wrapper");
        for arg in [
            "--working-directory=/owned/work",
            "--slice=admiralty.slice",
            "--expand-environment=no",
            "--property=RuntimeMaxSec=0.400s",
        ] {
            assert!(wrapped.args.iter().any(|s| s == arg));
        }
        let env = wrapped
            .args
            .iter()
            .position(|s| s == "/usr/bin/env")
            .expect("env");
        assert_eq!(wrapped.args[env + 1], "-i");
        assert_eq!(wrapped.args.last(), spec().args.last());
        assert!(matches!(
            backend.wrap(&spec(), &environment(), &claims(), "unit", 900),
            Err(ResourceGateError::Expired)
        ));
    }
    #[test]
    fn environment_rejects_secrets_loaders_and_implicit_path_search() {
        for key in [
            "OPENAI_API_KEY",
            "LD_PRELOAD",
            "DBUS_SESSION_BUS_ADDRESS",
            "GIT_CONFIG_COUNT",
        ] {
            let mut values = environment().0;
            values.insert(key.into(), "hidden".into());
            assert!(QualifiedEnvironment::new(values).is_err());
        }
        let mut values = environment().0;
        values.insert("PATH".into(), ".:/usr/bin".into());
        assert!(QualifiedEnvironment::new(values).is_err());
    }
    #[test]
    fn failed_or_incomplete_observations_never_mean_inactive() {
        assert!(parse_properties(b"ActiveState=inactive\nActiveState=failed\n").is_err());
        assert!(
            field(
                &parse_properties(b"LoadState=not-found\n").expect("properties"),
                "InvocationID"
            )
            .is_err()
        );
        assert!(parse_limit("max").is_err());
        assert!(parse_cpu_set("0-4294967295").is_err());
        assert!(parse_pids("12\nnot-a-pid").is_err());
        assert_eq!(
            process_start(&format!(
                "12 (name with ) spaces) S {} 987",
                vec!["0"; 18].join(" ")
            ))
            .expect("stat"),
            "987"
        );
    }
    #[test]
    fn observation_environment_is_exact_and_local_bus_addresses_are_escaped() {
        assert_eq!(
            dbus_escape(Path::new("/run/user/name,with;delimiters/bus")),
            "/run/user/name%2Cwith%3Bdelimiters/bus"
        );
        assert_eq!(
            parse_environment(b"PATH=/usr/bin:/bin\0HOME=/nonexistent\0TMPDIR=/tmp\0")
                .expect("env"),
            environment().0
        );
        assert!(parse_environment(b"PATH=x\0PATH=y\0").is_err());
        assert!(parse_environment(b"PATH=x").is_err());
    }
    #[test]
    fn unresolved_slots_survive_handle_drop_and_known_slots_release_once() {
        let state = Arc::new(Mutex::new(GateState {
            active_by_permit: BTreeMap::from([("permit".into(), 1)]),
        }));
        drop(CapacitySlot {
            state: state.clone(),
            permit_id: "permit".into(),
            retained: true,
        });
        assert_eq!(state.lock().expect("state").active_by_permit["permit"], 1);
        drop(CapacitySlot {
            state: state.clone(),
            permit_id: "permit".into(),
            retained: false,
        });
        assert!(state.lock().expect("state").active_by_permit.is_empty());
    }
    #[test]
    fn owned_probe_output_and_runtime_are_bounded_without_waiting_for_pipe_drain() {
        let env = vec![(OsString::from("PATH"), OsString::from("/usr/bin:/bin"))];
        let start = Instant::now();
        assert!(
            bounded_output(
                Path::new("/bin/sh"),
                &["-c", "while :; do :; done"],
                &env,
                Duration::from_millis(50)
            )
            .is_err()
        );
        assert!(start.elapsed() < Duration::from_secs(2));
        assert!(bounded_output(Path::new("/bin/sh"),&["-c","while :; do printf '0123456789012345678901234567890123456789012345678901234567890123456789'; done"],&env,Duration::from_secs(2)).is_err());
        let result = bounded_output(
            Path::new("/bin/sh"),
            &["-c", "printf ok"],
            &env,
            Duration::from_secs(1),
        )
        .expect("probe");
        assert!(result.status.success());
        assert_eq!(result.stdout, b"ok");
    }
}
