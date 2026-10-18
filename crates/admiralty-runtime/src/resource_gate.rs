//! Fail-closed resource admission for prepared session launches.
//!
//! Authenticated permits are opaque and have no serde input. The central
//! integration may construct one only after the configured trusted issuer has
//! verified its signature, revocation state, current reservation and authority
//! generation. Agent JSON, placement offers, CPU visibility and build-job
//! counts cannot mint permits.
//!
//! The only implemented enforcement backend is a user systemd service in a
//! delegated cgroup v2 hierarchy. It applies CPU, memory, task and wall-time
//! bounds before exec and reads back the effective CPU set and cgroup limits.
//! taskset/prlimit alone cannot safely enforce a process-tree task limit because
//! RLIMIT_NPROC is per real UID, so they are not used as a substitute.
//!
//! This is resource control, not a full sandbox. It does not isolate filesystem
//! paths, credentials, network, namespaces, devices or descendants that escape
//! the managed cgroup. Callers need a separately qualified workspace and
//! credential/network boundary.

use crate::session::{LaunchSpec, SessionError, SessionHost};
use admiralty_core::{AttemptId, ProjectId, TaskId, content_digest, nodes::NodeId};
use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs, io,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use thiserror::Error;

const CGROUP_ROOT: &str = "/sys/fs/cgroup";
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const OUTPUT_LIMIT: usize = 16 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdmissionIdentity {
    pub project_id: ProjectId,
    pub task_id: TaskId,
    pub attempt_id: AttemptId,
    pub node_id: NodeId,
    pub lease_id: String,
}

/// Authenticated claims returned only by a configured permit issuer.
/// Deliberately not deserializable: untrusted JSON cannot create authority.
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
}

/// Opaque issuer-authenticated permit. There is no public constructor and no
/// Deserialize implementation. The parent integration must validate the
/// configured issuer signature and reservation before using this crate-private
/// constructor.
#[derive(Clone, Debug)]
pub struct VerifiedResourcePermit {
    claims: ResourcePermitClaims,
}

impl VerifiedResourcePermit {
    #[allow(dead_code)] // Called by centralized issuer integration when it is wired.
    pub(crate) fn from_authenticated_issuer(claims: ResourcePermitClaims) -> Self {
        Self { claims }
    }
}

#[derive(Debug, Error)]
pub enum ResourceGateError {
    #[error("resource permit is unavailable or not current")]
    PermitUnavailable,
    #[error("permit identity or exact launch digest does not match")]
    IdentityMismatch,
    #[error("permit or lease is expired")]
    Expired,
    #[error("capacity is invalid or reserved concurrency is exhausted")]
    CapacityUnavailable,
    #[error("CPU set is outside registered, inherited or effective scope")]
    CpuScope,
    #[error("enforceable systemd cgroup backend unavailable: {0}")]
    UnsupportedBackend(String),
    #[error("observed resource enforcement does not match permit: {0}")]
    EnforcementMismatch(&'static str),
    #[error("session host failed: {0}")]
    Session(#[from] SessionError),
    #[error("resource gate I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("resource gate lock was poisoned")]
    Poisoned,
}

#[derive(Default)]
struct GateState {
    active_by_permit: BTreeMap<String, u32>,
}

/// Per-node gate. Clones share the in-process concurrency counter.
#[derive(Clone)]
pub struct ResourceGate {
    node_id: NodeId,
    state: Arc<Mutex<GateState>>,
}

impl ResourceGate {
    pub fn new(node_id: NodeId) -> Self {
        Self {
            node_id,
            state: Arc::new(Mutex::new(GateState::default())),
        }
    }

    /// Validate identity, current lease, capacity and delegated backend before
    /// returning a launch wrapper. No child is started by this method.
    pub fn prepare(
        &self,
        permit: VerifiedResourcePermit,
        identity: AdmissionIdentity,
        spec: LaunchSpec,
    ) -> Result<AdmittedLaunch, ResourceGateError> {
        let now = unix_time_ms()?;
        let claims = permit.claims;
        validate_claims(&claims, &identity, &spec, &self.node_id, now)?;
        let backend = SystemdCgroupBackend::discover()?;
        backend.validate_parent_scope(&claims.cpu_set)?;

        let mut state = self.state.lock().map_err(|_| ResourceGateError::Poisoned)?;
        let active = state
            .active_by_permit
            .entry(claims.permit_id.clone())
            .or_default();
        if *active >= claims.max_concurrency {
            return Err(ResourceGateError::CapacityUnavailable);
        }
        *active += 1;
        drop(state);
        let slot = CapacitySlot {
            state: Arc::clone(&self.state),
            permit_id: claims.permit_id.clone(),
            retain_on_drop: false,
        };

        let unit = systemd_unit_name(&claims.permit_id)?;
        let enforced_spec = backend.wrap(&spec, &claims, &unit, now)?;
        let original_spec_digest =
            content_digest(&spec).map_err(|_| ResourceGateError::IdentityMismatch)?;
        Ok(AdmittedLaunch {
            claims,
            backend,
            unit,
            original_spec_digest,
            enforced_spec,
            slot: Some(slot),
        })
    }
}

/// Opaque wrapper produced only after permit and backend admission.
pub struct AdmittedLaunch {
    claims: ResourcePermitClaims,
    backend: SystemdCgroupBackend,
    unit: String,
    original_spec_digest: String,
    enforced_spec: LaunchSpec,
    slot: Option<CapacitySlot>,
}

impl AdmittedLaunch {
    /// Launches through a direct systemd-run argv vector, then verifies the
    /// actual cgroup controls and process membership. Failed or ambiguous
    /// launches retain the concurrency reservation for reconciliation.
    pub fn launch(mut self, host: &SessionHost) -> Result<RunningAdmission, ResourceGateError> {
        let slot = self.slot.as_mut().ok_or(ResourceGateError::CapacityUnavailable)?;
        slot.retain_on_drop = true;
        host.launch(self.enforced_spec.clone())?;
        let observation = match self.backend.observe(&self.unit, &self.claims) {
            Ok(observation) => observation,
            Err(error) => {
                let _ = host.terminate(&self.enforced_spec.run_id);
                return Err(error);
            }
        };
        if observation.original_spec_digest != self.original_spec_digest {
            let _ = host.terminate(&self.enforced_spec.run_id);
            return Err(ResourceGateError::IdentityMismatch);
        }
        Ok(RunningAdmission {
            claims: self.claims,
            backend: self.backend,
            unit: self.unit,
            run_id: self.enforced_spec.run_id,
            observation,
            slot: self.slot.take(),
        })
    }
}

pub struct EnforcementObservation {
    pub unit: String,
    pub process_ids: Vec<u32>,
    pub cpu_set: BTreeSet<u32>,
    pub memory_max_bytes: u64,
    pub tasks_max: u32,
    pub tasks_current: u32,
    pub original_spec_digest: String,
}

pub struct RunningAdmission {
    claims: ResourcePermitClaims,
    backend: SystemdCgroupBackend,
    unit: String,
    run_id: String,
    observation: EnforcementObservation,
    slot: Option<CapacitySlot>,
}

impl RunningAdmission {
    pub fn observation(&self) -> &EnforcementObservation {
        &self.observation
    }

    /// Release reserved concurrency only after the SessionHost observes exit
    /// and systemd reports the service inactive. Dropping an unresolved handle
    /// retains its slot; callers must reconcile before admitting a retry.
    pub fn reconcile_complete(mut self, host: &SessionHost) -> Result<ExitStatus, ResourceGateError> {
        let status = host.status(&self.run_id)?;
        if status.exit.is_none() || !self.backend.unit_is_inactive(&self.unit)? {
            return Err(ResourceGateError::CapacityUnavailable);
        }
        let exit = host.wait(&self.run_id)?;
        if let Some(slot) = self.slot.as_mut() {
            slot.release()?;
        }
        self.slot.take();
        let _ = &self.claims;
        Ok(exit)
    }
}

struct CapacitySlot {
    state: Arc<Mutex<GateState>>,
    permit_id: String,
    retain_on_drop: bool,
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
        self.retain_on_drop = true;
        Ok(())
    }
}

impl Drop for CapacitySlot {
    fn drop(&mut self) {
        if self.retain_on_drop {
            return;
        }
        if let Ok(mut state) = self.state.lock()
            && let Some(active) = state.active_by_permit.get_mut(&self.permit_id)
        {
            *active = active.saturating_sub(1);
            if *active == 0 {
                state.active_by_permit.remove(&self.permit_id);
            }
        }
    }
}

fn validate_claims(
    claims: &ResourcePermitClaims,
    identity: &AdmissionIdentity,
    spec: &LaunchSpec,
    node_id: &NodeId,
    now_ms: u64,
) -> Result<(), ResourceGateError> {
    if claims.identity != *identity
        || claims.identity.node_id != *node_id
        || claims.launch_spec_digest
            != content_digest(spec).map_err(|_| ResourceGateError::IdentityMismatch)?
    {
        return Err(ResourceGateError::IdentityMismatch);
    }
    if claims.issuer_ref.trim().is_empty()
        || claims.permit_id.trim().is_empty()
        || claims.reservation_ref.trim().is_empty()
        || claims.host_incarnation != read_host_incarnation()?
    {
        return Err(ResourceGateError::IdentityMismatch);
    }
    if now_ms >= claims.expires_at_unix_ms || now_ms >= claims.lease_expires_at_unix_ms {
        return Err(ResourceGateError::Expired);
    }
    if claims.cpu_set.is_empty()
        || !claims.cpu_set.is_subset(&claims.registered_cpu_scope)
        || claims.memory_max_bytes == 0
        || claims.max_tasks == 0
        || claims.max_concurrency == 0
    {
        return Err(ResourceGateError::CapacityUnavailable);
    }
    if !read_current_cpu_set()?.is_subset(&claims.cpu_set) {
        return Err(ResourceGateError::CpuScope);
    }
    Ok(())
}

#[derive(Clone)]
struct SystemdCgroupBackend {
    systemd_run: PathBuf,
    systemctl: PathBuf,
    cgroup_parent: PathBuf,
}

impl SystemdCgroupBackend {
    fn discover() -> Result<Self, ResourceGateError> {
        #[cfg(not(target_os = "linux"))]
        {
            return Err(ResourceGateError::UnsupportedBackend(
                "Linux cgroup v2 is required".into(),
            ));
        }
        #[cfg(target_os = "linux")]
        {
            let systemd_run = trusted_executable("systemd-run")?;
            let systemctl = trusted_executable("systemctl")?;
            let manager = bounded_output(
                &systemctl,
                &["--user", "show-environment"],
                PROBE_TIMEOUT,
            )
            .map_err(|error| {
                ResourceGateError::UnsupportedBackend(format!(
                    "user systemd manager unavailable: {error}"
                ))
            })?;
            if !manager.status.success() || manager.stdout.len() > OUTPUT_LIMIT {
                return Err(ResourceGateError::UnsupportedBackend(
                    "user systemd manager did not answer bounded probe".into(),
                ));
            }
            let cgroup_parent = current_cgroup_path()?;
            let available = fs::read_to_string(cgroup_parent.join("cgroup.controllers"))?;
            let available: BTreeSet<_> = available.split_ascii_whitespace().collect();
            if !["cpuset", "memory", "pids"]
                .iter()
                .all(|controller| available.contains(controller))
            {
                return Err(ResourceGateError::UnsupportedBackend(
                    "current cgroup lacks cpuset, memory or pids controller".into(),
                ));
            }
            for file in ["cpuset.cpus.effective", "memory.max", "pids.max"] {
                if !cgroup_parent.join(file).is_file() {
                    return Err(ResourceGateError::UnsupportedBackend(format!(
                        "required delegated control is missing: {file}"
                    )));
                }
            }
            Ok(Self {
                systemd_run,
                systemctl,
                cgroup_parent,
            })
        }
    }

    fn validate_parent_scope(&self, cpus: &BTreeSet<u32>) -> Result<(), ResourceGateError> {
        let parent_cpus = parse_cpu_set(&fs::read_to_string(
            self.cgroup_parent.join("cpuset.cpus.effective"),
        )?)?;
        if !cpus.is_subset(&parent_cpus) {
            return Err(ResourceGateError::CpuScope);
        }
        Ok(())
    }

    fn wrap(
        &self,
        original: &LaunchSpec,
        claims: &ResourcePermitClaims,
        unit: &str,
        now_ms: u64,
    ) -> Result<LaunchSpec, ResourceGateError> {
        let expires_at = claims.expires_at_unix_ms.min(claims.lease_expires_at_unix_ms);
        let runtime_ms = expires_at.saturating_sub(now_ms);
        if runtime_ms == 0 {
            return Err(ResourceGateError::Expired);
        }
        let cpus = claims
            .cpu_set
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let args = vec![
            "--user".into(),
            "--quiet".into(),
            "--pipe".into(),
            "--wait".into(),
            format!("--unit={unit}"),
            format!("--property=AllowedCPUs={cpus}"),
            format!("--property=MemoryMax={}B", claims.memory_max_bytes),
            format!("--property=TasksMax={}", claims.max_tasks),
            format!(
                "--property=RuntimeMaxSec={}.{:03}s",
                runtime_ms / 1000,
                runtime_ms % 1000
            ),
            "--service-type=exec".into(),
            "--".into(),
            original.program.to_string_lossy().into_owned(),
        ]
        .into_iter()
        .chain(original.args.iter().cloned())
        .collect();
        Ok(LaunchSpec {
            run_id: original.run_id.clone(),
            program: self.systemd_run.clone(),
            args,
            cwd: original.cwd.clone(),
            spool_dir: original.spool_dir.clone(),
        })
    }

    fn observe(
        &self,
        unit: &str,
        claims: &ResourcePermitClaims,
    ) -> Result<EnforcementObservation, ResourceGateError> {
        let output = bounded_output(
            &self.systemctl,
            &["--user", "show", "--property=ControlGroup", "--value", unit],
            PROBE_TIMEOUT,
        )?;
        if !output.status.success() || output.stdout.len() > OUTPUT_LIMIT {
            return Err(ResourceGateError::EnforcementMismatch(
                "systemd did not report admitted cgroup",
            ));
        }
        let relative = String::from_utf8(output.stdout)
            .map_err(|_| ResourceGateError::EnforcementMismatch("invalid cgroup path"))?
            .trim()
            .to_owned();
        if relative.is_empty() || relative.contains("..") {
            return Err(ResourceGateError::EnforcementMismatch(
                "empty or traversing cgroup path",
            ));
        }
        let cgroup = Path::new(CGROUP_ROOT).join(relative.trim_start_matches('/'));
        let canonical = fs::canonicalize(&cgroup)?;
        if !canonical.starts_with(CGROUP_ROOT)
            || canonical == Path::new(CGROUP_ROOT)
            || !canonical.starts_with(&self.cgroup_parent)
        {
            return Err(ResourceGateError::EnforcementMismatch(
                "reported cgroup escaped the validated delegated subtree",
            ));
        }
        let actual_cpus = parse_cpu_set(&fs::read_to_string(
            canonical.join("cpuset.cpus.effective"),
        )?)?;
        if actual_cpus != claims.cpu_set {
            return Err(ResourceGateError::EnforcementMismatch(
                "effective CPU set differs from permit",
            ));
        }
        let memory = parse_limit(&fs::read_to_string(canonical.join("memory.max"))?)?;
        if memory == 0 || memory > claims.memory_max_bytes {
            return Err(ResourceGateError::EnforcementMismatch(
                "memory.max absent or exceeds permit",
            ));
        }
        let tasks = parse_limit(&fs::read_to_string(canonical.join("pids.max"))?)?;
        let current_tasks = parse_limit(&fs::read_to_string(canonical.join("pids.current"))?)?;
        if tasks == 0 || tasks > u64::from(claims.max_tasks) || current_tasks > tasks {
            return Err(ResourceGateError::EnforcementMismatch(
                "pids controller does not enforce task bound",
            ));
        }
        let process_ids = fs::read_to_string(canonical.join("cgroup.procs"))?
            .lines()
            .filter_map(|line| line.parse::<u32>().ok())
            .collect::<Vec<_>>();
        if process_ids.is_empty() {
            return Err(ResourceGateError::EnforcementMismatch(
                "no process observed in admitted cgroup",
            ));
        }
        Ok(EnforcementObservation {
            unit: unit.into(),
            process_ids,
            cpu_set: actual_cpus,
            memory_max_bytes: memory,
            tasks_max: u32::try_from(tasks).map_err(|_| {
                ResourceGateError::EnforcementMismatch("task limit exceeds supported range")
            })?,
            tasks_current: u32::try_from(current_tasks).map_err(|_| {
                ResourceGateError::EnforcementMismatch("observed task count exceeds supported range")
            })?,
            original_spec_digest: claims.launch_spec_digest.clone(),
        })
    }

    fn unit_is_inactive(&self, unit: &str) -> Result<bool, ResourceGateError> {
        let output = bounded_output(
            &self.systemctl,
            &["--user", "show", "--property=ActiveState", "--value", unit],
            PROBE_TIMEOUT,
        )?;
        if !output.status.success() {
            return Ok(true);
        }
        Ok(matches!(
            String::from_utf8_lossy(&output.stdout).trim(),
            "inactive" | "failed"
        ))
    }
}

fn validate_identity_text(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
}

fn systemd_unit_name(permit_id: &str) -> Result<String, ResourceGateError> {
    if !validate_identity_text(permit_id) {
        return Err(ResourceGateError::IdentityMismatch);
    }
    Ok(format!("adm-resource-{permit_id}.service"))
}

fn unix_time_ms() -> Result<u64, ResourceGateError> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| ResourceGateError::Expired)?;
    u64::try_from(elapsed.as_millis()).map_err(|_| ResourceGateError::Expired)
}

fn read_host_incarnation() -> Result<String, ResourceGateError> {
    Ok(fs::read_to_string("/proc/sys/kernel/random/boot_id")?
        .trim()
        .to_owned())
}

fn current_cgroup_path() -> Result<PathBuf, ResourceGateError> {
    let contents = fs::read_to_string("/proc/self/cgroup")?;
    let relative = contents
        .lines()
        .find_map(|line| line.strip_prefix("0::"))
        .ok_or_else(|| {
            ResourceGateError::UnsupportedBackend("unified cgroup v2 unavailable".into())
        })?;
    let canonical = fs::canonicalize(Path::new(CGROUP_ROOT).join(relative.trim_start_matches('/')))?;
    if !canonical.starts_with(CGROUP_ROOT) {
        return Err(ResourceGateError::UnsupportedBackend(
            "process cgroup escapes unified hierarchy".into(),
        ));
    }
    Ok(canonical)
}

fn parse_cpu_set(value: &str) -> Result<BTreeSet<u32>, ResourceGateError> {
    let mut result = BTreeSet::new();
    for field in value.trim().split(',') {
        if field.is_empty() {
            return Err(ResourceGateError::CpuScope);
        }
        let (start, end) = match field.split_once('-') {
            Some((first, last)) => (
                first.parse::<u32>().map_err(|_| ResourceGateError::CpuScope)?,
                last.parse::<u32>().map_err(|_| ResourceGateError::CpuScope)?,
            ),
            None => {
                let cpu = field.parse::<u32>().map_err(|_| ResourceGateError::CpuScope)?;
                (cpu, cpu)
            }
        };
        if end < start || end.saturating_sub(start) > 4096 {
            return Err(ResourceGateError::CpuScope);
        }
        for cpu in start..=end {
            if !result.insert(cpu) {
                return Err(ResourceGateError::CpuScope);
            }
        }
    }
    if result.is_empty() {
        return Err(ResourceGateError::CpuScope);
    }
    Ok(result)
}

fn read_current_cpu_set() -> Result<BTreeSet<u32>, ResourceGateError> {
    let status = fs::read_to_string("/proc/self/status")?;
    let allowed = status
        .lines()
        .find_map(|line| line.strip_prefix("Cpus_allowed_list:"))
        .ok_or(ResourceGateError::CpuScope)?;
    parse_cpu_set(allowed)
}

fn parse_limit(value: &str) -> Result<u64, ResourceGateError> {
    let trimmed = value.trim();
    if trimmed == "max" {
        return Err(ResourceGateError::EnforcementMismatch(
            "unbounded cgroup control",
        ));
    }
    trimmed
        .parse::<u64>()
        .map_err(|_| ResourceGateError::EnforcementMismatch("invalid cgroup control"))
}

fn trusted_executable(name: &str) -> Result<PathBuf, ResourceGateError> {
    for directory in env::split_paths(&env::var_os("PATH").unwrap_or_default()) {
        let candidate = directory.join(name);
        if fs::metadata(&candidate).is_err() {
            continue;
        }
        let canonical = fs::canonicalize(&candidate)?;
        let metadata = fs::metadata(&canonical)?;
        if metadata.is_file()
            && metadata.uid() == 0
            && metadata.mode() & 0o022 == 0
            && metadata.mode() & 0o111 != 0
        {
            return Ok(canonical);
        }
    }
    Err(ResourceGateError::UnsupportedBackend(format!(
        "trusted {name} executable not found"
    )))
}

struct BoundedOutput {
    status: ExitStatus,
    stdout: Vec<u8>,
}

fn bounded_output(
    program: &Path,
    args: &[&str],
    timeout: Duration,
) -> Result<BoundedOutput, ResourceGateError> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            let output = child.wait_with_output()?;
            if output.stdout.len() > OUTPUT_LIMIT {
                return Err(ResourceGateError::UnsupportedBackend(
                    "systemd probe output exceeded bound".into(),
                ));
            }
            return Ok(BoundedOutput {
                status,
                stdout: output.stdout,
            });
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(ResourceGateError::UnsupportedBackend(
                "systemd probe timed out".into(),
            ));
        }
        thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> Result<AdmissionIdentity, Box<dyn std::error::Error>> {
        Ok(AdmissionIdentity {
            project_id: ProjectId::new("project-a")?,
            task_id: TaskId::new("task-a")?,
            attempt_id: AttemptId::new("attempt-a")?,
            node_id: NodeId::new("node-a")?,
            lease_id: "lease-a".into(),
        })
    }

    fn spec() -> LaunchSpec {
        LaunchSpec {
            run_id: "resource-run-a".into(),
            program: PathBuf::from("/usr/bin/true"),
            args: vec!["--literal-arg".into()],
            cwd: PathBuf::from("/"),
            spool_dir: PathBuf::from("/resource-gate-test"),
        }
    }

    fn claims(
        identity: AdmissionIdentity,
        spec: &LaunchSpec,
    ) -> Result<ResourcePermitClaims, Box<dyn std::error::Error>> {
        let now = unix_time_ms()?;
        Ok(ResourcePermitClaims {
            issuer_ref: "configured-issuer".into(),
            permit_id: "permit-a".into(),
            reservation_ref: "reservation-a".into(),
            identity,
            host_incarnation: read_host_incarnation()?,
            expires_at_unix_ms: now.saturating_add(60_000),
            lease_expires_at_unix_ms: now.saturating_add(60_000),
            registered_cpu_scope: read_current_cpu_set()?,
            cpu_set: read_current_cpu_set()?,
            memory_max_bytes: 512 * 1024 * 1024,
            max_tasks: 16,
            max_concurrency: 1,
            launch_spec_digest: content_digest(spec)?,
        })
    }

    #[test]
    fn identity_lease_and_exact_launch_digest_are_mandatory() -> Result<(), Box<dyn std::error::Error>> {
        let request_identity = identity()?;
        let launch = spec();
        let permit = claims(request_identity.clone(), &launch)?;
        validate_claims(
            &permit,
            &request_identity,
            &launch,
            &request_identity.node_id,
            unix_time_ms()?,
        )?;

        let mut changed_launch = launch.clone();
        changed_launch.args.push("different".into());
        assert!(matches!(
            validate_claims(
                &permit,
                &request_identity,
                &changed_launch,
                &request_identity.node_id,
                unix_time_ms()?
            ),
            Err(ResourceGateError::IdentityMismatch)
        ));
        let mut changed_identity = request_identity.clone();
        changed_identity.lease_id.push_str("-stale");
        assert!(matches!(
            validate_claims(
                &permit,
                &changed_identity,
                &launch,
                &request_identity.node_id,
                unix_time_ms()?
            ),
            Err(ResourceGateError::IdentityMismatch)
        ));
        Ok(())
    }

    #[test]
    fn registered_cpu_scope_and_expiration_fail_closed() -> Result<(), Box<dyn std::error::Error>> {
        let request_identity = identity()?;
        let launch = spec();
        let mut permit = claims(request_identity.clone(), &launch)?;
        permit.registered_cpu_scope = BTreeSet::from([0, 1]);
        assert!(matches!(
            validate_claims(
                &permit,
                &request_identity,
                &launch,
                &request_identity.node_id,
                unix_time_ms()?
            ),
            Err(ResourceGateError::CapacityUnavailable)
        ));

        permit.registered_cpu_scope = permit.cpu_set.clone();
        permit.expires_at_unix_ms = 1;
        assert!(matches!(
            validate_claims(
                &permit,
                &request_identity,
                &launch,
                &request_identity.node_id,
                unix_time_ms()?
            ),
            Err(ResourceGateError::Expired)
        ));
        Ok(())
    }

    #[test]
    fn systemd_argv_preserves_arguments_without_a_shell() -> Result<(), Box<dyn std::error::Error>> {
        let backend = SystemdCgroupBackend {
            systemd_run: PathBuf::from("/usr/bin/systemd-run"),
            systemctl: PathBuf::from("/usr/bin/systemctl"),
            cgroup_parent: PathBuf::from(CGROUP_ROOT),
        };
        let original = LaunchSpec {
            run_id: "run".into(),
            program: PathBuf::from("/owned/program"),
            args: vec!["arg with spaces".into(), "$(literal-not-shell)".into()],
            cwd: PathBuf::from("/owned"),
            spool_dir: PathBuf::from("/owned/spool"),
        };
        let mut permit = claims(identity()?, &original)?;
        permit.cpu_set = BTreeSet::from([0]);
        permit.memory_max_bytes = 1024;
        permit.max_tasks = 4;
        let wrapped = backend.wrap(&original, &permit, "adm-resource-permit-a.service", 1)?;
        assert_eq!(wrapped.program, PathBuf::from("/usr/bin/systemd-run"));
        assert_eq!(wrapped.args.last(), Some(&"$(literal-not-shell)".to_owned()));
        assert!(wrapped.args.contains(&"--".to_owned()));
        assert!(!wrapped.args.iter().any(|argument| argument == "-c"));
        Ok(())
    }

    #[test]
    fn cpu_and_limit_parsers_reject_unbounded_or_malformed_observations() {
        assert_eq!(parse_cpu_set("1-3,7"), Ok(BTreeSet::from([1, 2, 3, 7])));
        assert!(parse_cpu_set("1,,3").is_err());
        assert!(parse_cpu_set("3-1").is_err());
        assert_eq!(parse_limit("4096"), Ok(4096));
        assert!(parse_limit("max").is_err());
    }
}
