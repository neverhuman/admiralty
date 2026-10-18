//! One-use, operator-authorized Linux resource admission probe.
//! This is not a production scheduler issuer or execution qualification.
use admiralty_core::{AttemptId, ProjectId, TaskId, content_digest, nodes::NodeId};
use admiralty_runtime::{
    resource_gate::{
        AdmissionIdentity, ClosedAdmission, QualifiedEnvironment, ResourceGate,
        ResourcePermitAuthority, ResourcePermitClaims, SystemdBackendConfig,
        VerifiedResourcePermit,
    },
    session::{LaunchSpec, SessionHost},
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fs::{self, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

const AUTH_PATH: &str = "/home/ubuntu/admiralty-v2-build/private-config/native-probe-20261018T210858Z/operator-authorization.json";
const AUTH_DIR: &str = "/home/ubuntu/admiralty-v2-build/private-config/native-probe-20261018T210858Z";
const EXPECTED_POLICY_SHA256: &str = "185b36d77bb6c5b6b5e4c720fab56fb5ed98d10646e972e58bb651eb5f536ede";
const MAX_AUTH_BYTES: u64 = 64 * 1024;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Authorization {
    schema_version: u32,
    record_kind: String,
    production_execution_qualified: bool,
    issued_at_utc: String,
    expires_at_utc: String,
    issuer: String,
    authority_ref: String,
    project_id: String,
    root_id: String,
    task_id: String,
    attempt_id: String,
    node_id: String,
    lease_id: String,
    reservation_ref: String,
    permit_id: String,
    host_incarnation: String,
    expires_at_unix_ms: u64,
    lease_expires_at_unix_ms: u64,
    registered_cpu_scope: Vec<u32>,
    cpu_set: Vec<u32>,
    memory_max_bytes: u64,
    max_tasks: u32,
    max_concurrency: u32,
    launch_spec: LaunchSpec,
    launch_spec_digest: String,
    worker_environment: BTreeMap<String, String>,
    worker_environment_digest: String,
    backend: BackendPolicy,
    backend_config_digest: String,
    scope: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BackendPolicy {
    runtime_directory: PathBuf,
    delegated_slice: String,
}

#[derive(Clone)]
struct CurrentOperatorAuthorization {
    path: PathBuf,
    claims: ResourcePermitClaims,
    policy_bytes: Vec<u8>,
}

impl ResourcePermitAuthority for CurrentOperatorAuthorization {
    fn is_current(&self, claims: &ResourcePermitClaims) -> bool {
        if claims != &self.claims {
            return false;
        }
        let Ok((policy, bytes)) = read_authorization(&self.path) else {
            return false;
        };
        bytes == self.policy_bytes
            && current_time_ms().is_ok_and(|now| {
                now < policy.expires_at_unix_ms && now < policy.lease_expires_at_unix_ms
            })
            && read_text(Path::new("/proc/sys/kernel/random/boot_id"))
                .is_ok_and(|boot| boot.trim() == policy.host_incarnation)
            && current_cpus().is_ok_and(|cpus| {
                policy
                    .registered_cpu_scope
                    .iter()
                    .all(|cpu| cpus.contains(cpu))
            })
    }
}

fn main() -> Result<()> {
    let mode = std::env::args().nth(1);
    if !matches!(mode.as_deref(), Some("--preflight" | "--run-authorized-once")) {
        return Err("pass --run-authorized-once for the issued single-run probe".into());
    }
    let (authorization, bytes) = read_authorization(Path::new(AUTH_PATH))?;
    validate_policy(&authorization, &bytes)?;
    if Path::new(AUTH_DIR).join("one-run-claimed").exists()
        || Path::new(AUTH_DIR).join("admission-recovery.json").exists()
        || Path::new(AUTH_DIR).join("closed-admission.json").exists()
    {
        return Err("one-run authorization already consumed or reconciled".into());
    }
    let spec = authorization.launch_spec.clone();
    let environment = QualifiedEnvironment::new(authorization.worker_environment.clone())?;
    let backend = SystemdBackendConfig {
        runtime_directory: authorization.backend.runtime_directory.clone(),
        delegated_slice: authorization.backend.delegated_slice.clone(),
    };
    if content_digest(&spec)? != authorization.launch_spec_digest
        || environment.digest()? != authorization.worker_environment_digest
        || backend.digest()? != authorization.backend_config_digest
    {
        return Err("issued launch, environment, or backend digest mismatch".into());
    }
    let claims = claims_from(&authorization)?;
    validate_host_and_cpu(&authorization)?;
    validate_prepared_paths(&authorization)?;

    let authority = Arc::new(CurrentOperatorAuthorization {
        path: PathBuf::from(AUTH_PATH),
        claims: claims.clone(),
        policy_bytes: bytes.clone(),
    });
    let permit = VerifiedResourcePermit::verify_current(claims.clone(), authority)?;
    if mode.as_deref() == Some("--preflight") {
        println!(
            "PASS: protected one-run authorization; policy_sha256={} spec={} environment={} backend={}",
            sha256_hex(&bytes),
            authorization.launch_spec_digest,
            authorization.worker_environment_digest,
            authorization.backend_config_digest
        );
        return Ok(());
    }
    let gate = ResourceGate::configured(
        NodeId::new(authorization.node_id.clone())?,
        backend,
        environment,
    );
    let identity = claims.identity.clone();
    let admitted = gate.prepare(permit, identity, spec.clone())?;
    let recovery = admitted.recovery_identity();
    write_once(Path::new(AUTH_DIR).join("admission-recovery.json"), &serde_json::to_vec(&recovery)?)?;
    write_once(
        Path::new(AUTH_DIR).join("one-run-claimed"),
        format!("{}\n{}\n", authorization.authority_ref, sha256_hex(&bytes)).as_bytes(),
    )?;

    let spool = &authorization.launch_spec.spool_dir;
    validate_private_directory(spool)?;
    if fs::read_dir(spool)?.next().is_some() {
        return Err("issued spool directory is not empty; refusing the one-run launch".into());
    }
    fs::remove_dir(spool)?;

    let host = SessionHost::new();
    let (mut running, launch_error) = match admitted.launch(&host) {
        Ok(running) => (running, None),
        Err(failure) => {
            let message = failure.error.to_string();
            match failure.recovery {
                Some(recovery) => (*recovery, Some(message)),
                None => return Err(message.into()),
            }
        }
    };
    host.close_input(&authorization.launch_spec.run_id)?;
    host.wait(&authorization.launch_spec.run_id)?;
    let proof = running.prove_closed(&host).map_err(|error| match launch_error {
        Some(launch_error) => format!("{error}; launch observation error: {launch_error}"),
        None => error.to_string(),
    })?;
    write_once(
        Path::new(AUTH_DIR).join("closed-admission.json"),
        &serde_json::to_vec(&proof)?,
    )?;
    print_proof(&proof);
    if !proof.success() || proof.exit_code() != Some(0) {
        return Err("owned probe closed, but child did not exit successfully".into());
    }
    Ok(())
}

fn validate_policy(policy: &Authorization, bytes: &[u8]) -> Result<()> {
    if policy.schema_version != 1
        || policy.record_kind != "trusted_operator_qualification_authorization"
        || policy.production_execution_qualified
        || policy.issued_at_utc != "2026-10-18T21:08:58.937209+00:00"
        || policy.expires_at_utc != "2026-10-18T21:23:58.937209+00:00"
        || policy.issuer != "Codex Admiralty platform owner under Jepson hard-switch authorization"
        || policy.root_id != "native-cutover-qualification"
        || policy.scope != "One actual 4-second owned sleep resource/closure probe only; no model inference, production task, unrelated service or privilege grant. Resource bounds are within registered P10 allocation. Task/attempt/lease/reservation/permit IDs are ISSUED here, not claimed externally observed. No other launches. Expired/modified policy or uncertain outcome denies retry."
        || policy.memory_max_bytes != 134_217_728
        || policy.max_tasks != 16
        || policy.max_concurrency != 1
        || policy.registered_cpu_scope != vec![0, 1, 2, 3, 4, 5]
        || policy.cpu_set != vec![0, 1]
        || policy.launch_spec.program != Path::new("/usr/bin/sleep")
        || policy.launch_spec.args != ["4"]
    {
        return Err("operator authorization scope differs from this probe".into());
    }
    if bytes.is_empty() || bytes.len() as u64 > MAX_AUTH_BYTES {
        return Err("operator authorization exceeds size bound".into());
    }
    if sha256_hex(bytes) != EXPECTED_POLICY_SHA256 {
        return Err("operator authorization bytes differ from the issued policy".into());
    }
    Ok(())
}

fn claims_from(policy: &Authorization) -> Result<ResourcePermitClaims> {
    Ok(ResourcePermitClaims {
        issuer_ref: policy.authority_ref.clone(),
        permit_id: policy.permit_id.clone(),
        reservation_ref: policy.reservation_ref.clone(),
        identity: AdmissionIdentity {
            project_id: ProjectId::new(policy.project_id.clone())?,
            task_id: TaskId::new(policy.task_id.clone())?,
            attempt_id: AttemptId::new(policy.attempt_id.clone())?,
            node_id: NodeId::new(policy.node_id.clone())?,
            lease_id: policy.lease_id.clone(),
        },
        host_incarnation: policy.host_incarnation.clone(),
        expires_at_unix_ms: policy.expires_at_unix_ms,
        lease_expires_at_unix_ms: policy.lease_expires_at_unix_ms,
        registered_cpu_scope: policy.registered_cpu_scope.iter().copied().collect(),
        cpu_set: policy.cpu_set.iter().copied().collect(),
        memory_max_bytes: policy.memory_max_bytes,
        max_tasks: policy.max_tasks,
        max_concurrency: policy.max_concurrency,
        launch_spec_digest: policy.launch_spec_digest.clone(),
        worker_environment_digest: policy.worker_environment_digest.clone(),
        backend_config_digest: policy.backend_config_digest.clone(),
    })
}

fn validate_host_and_cpu(policy: &Authorization) -> Result<()> {
    if read_text(Path::new("/proc/sys/kernel/random/boot_id"))?.trim() != policy.host_incarnation {
        return Err("current boot identity differs from the issued authorization".into());
    }
    if current_time_ms()? >= policy.expires_at_unix_ms
        || current_time_ms()? >= policy.lease_expires_at_unix_ms
    {
        return Err("operator authorization or lease has expired".into());
    }
    let current = current_cpus()?;
    let registered: BTreeSet<_> = policy.registered_cpu_scope.iter().copied().collect();
    let allowed: BTreeSet<_> = policy.cpu_set.iter().copied().collect();
    if !registered.is_subset(&current) || !allowed.is_subset(&registered) {
        return Err("issued CPU scope is not currently eligible".into());
    }
    Ok(())
}

fn validate_prepared_paths(policy: &Authorization) -> Result<()> {
    if fs::canonicalize(&policy.launch_spec.program)? != policy.launch_spec.program
        || fs::metadata(&policy.launch_spec.program)?.len() == 0
        || fs::canonicalize(&policy.launch_spec.cwd)? != policy.launch_spec.cwd
    {
        return Err("issued executable or workspace path is not canonical".into());
    }
    validate_private_directory(&policy.launch_spec.cwd)?;
    validate_private_directory(&policy.launch_spec.spool_dir)?;
    if fs::read_dir(&policy.launch_spec.spool_dir)?.next().is_some() {
        return Err("issued spool directory must be empty".into());
    }
    Ok(())
}

fn validate_private_directory(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o777 != 0o700
    {
        return Err(format!("unsafe private directory: {}", path.display()).into());
    }
    Ok(())
}

fn read_authorization(path: &Path) -> Result<(Authorization, Vec<u8>)> {
    let dir = Path::new(AUTH_DIR);
    validate_private_directory(dir)?;
    let before = fs::symlink_metadata(path)?;
    if !before.is_file()
        || before.file_type().is_symlink()
        || before.uid() != rustix::process::geteuid().as_raw()
        || before.mode() & 0o777 != 0o600
        || before.nlink() != 1
        || before.len() == 0
        || before.len() > MAX_AUTH_BYTES
    {
        return Err("operator authorization file is not private, regular, and singly linked".into());
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags((rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32)
        .open(path)?;
    let opened = file.metadata()?;
    if opened.dev() != before.dev() || opened.ino() != before.ino() {
        return Err("operator authorization changed while opening".into());
    }
    let mut bytes = Vec::with_capacity(before.len() as usize);
    file.take(MAX_AUTH_BYTES + 1).read_to_end(&mut bytes)?;
    let after = fs::symlink_metadata(path)?;
    let opened_after = fs::metadata(path)?;
    if bytes.len() as u64 != before.len()
        || after.dev() != before.dev()
        || after.ino() != before.ino()
        || after.mtime() != before.mtime()
        || after.mtime_nsec() != before.mtime_nsec()
        || opened_after.nlink() != 1
    {
        return Err("operator authorization changed while reading".into());
    }
    let policy = serde_json::from_slice(&bytes)?;
    Ok((policy, bytes))
}

fn current_time_ms() -> Result<u64> {
    Ok(u64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis())?)
}

fn current_cpus() -> Result<BTreeSet<u32>> {
    let status = read_text(Path::new("/proc/self/status"))?;
    let list = status
        .lines()
        .find_map(|line| line.strip_prefix("Cpus_allowed_list:"))
        .ok_or("Cpus_allowed_list missing")?;
    parse_cpu_list(list)
}

fn parse_cpu_list(value: &str) -> Result<BTreeSet<u32>> {
    let mut result = BTreeSet::new();
    for part in value.trim().split(',') {
        let (start, end) = part
            .split_once('-')
            .map_or((part, part), |(start, end)| (start, end));
        let start: u32 = start.parse()?;
        let end: u32 = end.parse()?;
        if end < start || end - start > 4096 {
            return Err("invalid CPU list".into());
        }
        result.extend(start..=end);
    }
    Ok(result)
}

fn read_text(path: &Path) -> Result<String> {
    Ok(fs::read_to_string(path)?)
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn write_once(path: PathBuf, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn print_proof(proof: &ClosedAdmission) {
    let observation = proof.observation();
    println!(
        "closed={} success={} exit_code={:?} cpus={:?} memory_max_bytes={} tasks_max={} cgroup={} closure_digest={}",
        proof.identity().unit,
        proof.success(),
        proof.exit_code(),
        observation.cpu_set,
        observation.memory_max_bytes,
        observation.tasks_max,
        observation.cgroup_path.display(),
        proof.closure_digest()
    );
}
