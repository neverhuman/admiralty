//! Real, bounded execution of trusted contract checks against an exact clean
//! candidate checkout. Results are evidence only and never grant acceptance.

use crate::session::{CaptureState, LaunchSpec, OutputStream, SessionError, SessionHost};
use admiralty_core::{
    CheckSpec, TaskContract, content_digest,
    verification::{
        CheckResult, ReceiptProducer, VerificationInputs, VerificationPolicy, VerificationReceipt,
        validate_receipt,
    },
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    fs,
    io::Read,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

pub const MAX_TRUSTED_CHECKS: usize = 16;
pub const MAX_CHECK_OUTPUT_BYTES: usize = 1024 * 1024;
pub const MAX_CHECK_TIMEOUT_SECS: u32 = 300;
const MAX_STATUS_OUTPUT_BYTES: usize = 256 * 1024;
const MAX_TAIL_PAGE_BYTES: usize = 256 * 1024;
const MAX_TRUSTED_PATH_BYTES: usize = 16 * 1024;
const SOURCE_PROBE_TIMEOUT_SECS: u32 = 10;
const MAX_EXECUTABLE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_ENVIRONMENT_EXECUTABLE_BYTES: u64 = 512 * 1024 * 1024;
const ENVIRONMENT_PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const RUNNER_LOCALE: &str = "C";
const POLL_INTERVAL: Duration = Duration::from_millis(10);

#[derive(Debug, thiserror::Error)]
pub enum TrustedCheckError {
    #[error("trusted-check request is invalid: {0}")]
    Invalid(&'static str),
    #[error("candidate checkout is not an owned private isolated repository")]
    CandidateCustody,
    #[error("candidate source differs from its pinned identity or is dirty")]
    CandidateDrift,
    #[error("trusted check executable or controlled environment differs from its pinned identity")]
    EnvironmentDrift,
    #[error("trusted check timed out")]
    TimedOut,
    #[error("trusted check exceeded its output bound")]
    OutputLimit,
    #[error("trusted check capture was incomplete or ambiguous")]
    CaptureIncomplete,
    #[error("trusted check exit did not match its trusted expectation")]
    ExitMismatch,
    #[error("trusted check could not be executed safely: {0}")]
    Process(String),
    #[error("verification evidence is internally inconsistent: {0}")]
    Evidence(String),
}

/// Opaque serialization-only proof that all named checks actually ran and the
/// exact source remained clean before and after them. This is not acceptance.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct ExecutedChecks {
    receipt: VerificationReceipt,
    execution_digest: String,
    environment: ExecutionEnvironment,
    environment_digest: String,
    cache_key_digest: String,
}

impl ExecutedChecks {
    pub fn receipt(&self) -> &VerificationReceipt {
        &self.receipt
    }

    pub fn execution_digest(&self) -> &str {
        &self.execution_digest
    }

    pub fn environment_digest(&self) -> &str {
        &self.environment_digest
    }

    /// Exact inputs and observed execution environment, without a run/receipt ID.
    /// A matching key never authenticates an issuer or grants acceptance.
    pub fn cache_key_digest(&self) -> &str {
        &self.cache_key_digest
    }

    pub fn candidate_checkout(&self) -> &Path {
        &self.environment.candidate_checkout
    }

    /// Recheck executable bytes and controlled variables against this proof.
    /// Performs bounded filesystem I/O; never call inside a Store authority
    /// callback or while holding the evidence catalog's state lock.
    pub fn revalidate_environment(
        &self,
        contract: &TaskContract,
        policy: &VerificationPolicy,
    ) -> Result<(), TrustedCheckError> {
        validate_request(
            &self.receipt.inputs,
            contract,
            policy,
            &self.receipt.receipt_id,
        )?;
        let candidate = validate_candidate(self.candidate_checkout())?;
        let current = capture_environment(&candidate, contract, policy)?;
        if current != self.environment {
            return Err(TrustedCheckError::EnvironmentDrift);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
struct ExecutableIdentity {
    path: PathBuf,
    sha256: String,
    bytes: u64,
    mode: u32,
}

/// Observed argv executables and the variables the runner actually installs.
/// Shared libraries, interpreter children and kernel state are outside this
/// identity. Private HOME/TMPDIR names vary per run; their creation policy is fixed.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
struct ExecutionEnvironment {
    os: &'static str,
    architecture: &'static str,
    candidate_checkout: PathBuf,
    search_path: String,
    home_policy: &'static str,
    temporary_policy: &'static str,
    lang: &'static str,
    lc_all: &'static str,
    executables: Vec<ExecutableIdentity>,
}

#[derive(Debug)]
struct ProcessOutput {
    exit_code: i32,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct SourceIdentity {
    commit: String,
    tree: String,
}

struct RunContext<'a> {
    host: &'a SessionHost,
    candidate: &'a Path,
    spool: &'a Path,
    search_path: &'a OsStr,
    run_prefix: &'a str,
    sequence: &'a mut u32,
}

/// Execute exact argv checks from a trusted contract and protected policy.
/// `host` must be the caller's live SessionHost; this function never adopts a
/// spool or claims ownership of a process started elsewhere.
pub fn execute_checks(
    host: &SessionHost,
    inputs: &VerificationInputs,
    contract: &TaskContract,
    policy: &VerificationPolicy,
    candidate_checkout: &Path,
    spool_root: &Path,
    run_prefix: &str,
) -> Result<ExecutedChecks, TrustedCheckError> {
    validate_request(inputs, contract, policy, run_prefix)?;
    let candidate = validate_candidate(candidate_checkout)?;
    let spool = validate_private_directory(spool_root)?;
    if candidate.starts_with(&spool) || spool.starts_with(&candidate) {
        return Err(TrustedCheckError::CandidateCustody);
    }
    let environment = capture_environment(&candidate, contract, policy)?;
    let search_path = OsStr::new(&environment.search_path);
    let git_program = environment.executables[0].path.clone();
    let environment_digest = content_digest(&("trusted-check-environment-v1", &environment))
        .map_err(|error| TrustedCheckError::Evidence(error.to_string()))?;
    let cache_key_digest = content_digest(&("trusted-check-cache-v1", inputs, &environment_digest))
        .map_err(|error| TrustedCheckError::Evidence(error.to_string()))?;
    let mut sequence = 0_u32;
    let mut run = RunContext {
        host,
        candidate: &candidate,
        spool: &spool,
        search_path,
        run_prefix,
        sequence: &mut sequence,
    };
    let before = source_identity(&mut run, &git_program)?;
    if before.commit != inputs.source_commit_sha || before.tree != inputs.source_tree_sha {
        return Err(TrustedCheckError::CandidateDrift);
    }

    let mut results = Vec::with_capacity(contract.checks.len());
    for check in &contract.checks {
        let output = execute_one(&mut run, check, 0)?;
        results.push(check_result(check, output));
    }

    let mut negative_results = Vec::with_capacity(policy.required_negative_checks.len());
    for negative in &policy.required_negative_checks {
        let output = execute_one(&mut run, &negative.check, negative.expected_exit_code)?;
        negative_results.push(check_result(&negative.check, output));
    }

    let after = source_identity(&mut run, &git_program)?;
    if after != before {
        return Err(TrustedCheckError::CandidateDrift);
    }
    if capture_environment(&candidate, contract, policy)? != environment {
        return Err(TrustedCheckError::EnvironmentDrift);
    }

    let receipt = VerificationReceipt {
        schema_version: admiralty_core::verification::RECEIPT_SCHEMA_VERSION,
        receipt_id: run_prefix.to_owned(),
        producer: ReceiptProducer::IndependentVerifier,
        verifier: policy.verifier.clone(),
        inputs: inputs.clone(),
        checks: results,
        negative_checks: negative_results,
    };
    let validated = validate_receipt(contract, inputs, policy, &receipt)
        .map_err(|error| TrustedCheckError::Evidence(error.to_string()))?;
    if !validated.all_checks_passed() {
        return Err(TrustedCheckError::ExitMismatch);
    }
    let execution_digest = content_digest(&(
        "trusted-check-execution-v2",
        &before,
        &receipt,
        &environment_digest,
        &cache_key_digest,
    ))
    .map_err(|error| TrustedCheckError::Evidence(error.to_string()))?;
    Ok(ExecutedChecks {
        receipt,
        execution_digest,
        environment,
        environment_digest,
        cache_key_digest,
    })
}

fn capture_environment(
    candidate: &Path,
    contract: &TaskContract,
    policy: &VerificationPolicy,
) -> Result<ExecutionEnvironment, TrustedCheckError> {
    let search_path = trusted_search_path()?;
    let git_program = resolve_trusted_git(&search_path)?;
    let deadline = Instant::now() + ENVIRONMENT_PROBE_TIMEOUT;
    let mut remaining = MAX_ENVIRONMENT_EXECUTABLE_BYTES;
    let mut hashed = BTreeMap::new();
    let mut paths = vec![git_program];
    for check in contract.checks.iter().chain(
        policy
            .required_negative_checks
            .iter()
            .map(|negative| &negative.check),
    ) {
        let cwd = check_cwd(candidate, check)?;
        paths.push(resolve_program(&check.program, &cwd, &search_path)?);
    }
    let mut executables = Vec::with_capacity(paths.len());
    for path in paths {
        let identity = match hashed.entry(path.clone()) {
            std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(hash_executable(&path, &mut remaining, deadline)?)
            }
        };
        executables.push(identity.clone());
    }
    Ok(ExecutionEnvironment {
        os: std::env::consts::OS,
        architecture: std::env::consts::ARCH,
        candidate_checkout: candidate.to_path_buf(),
        search_path: search_path
            .to_str()
            .ok_or(TrustedCheckError::Invalid(
                "trusted PATH is not valid UTF-8",
            ))?
            .to_owned(),
        home_policy: "fresh-owned-0700-empty-home-per-process-v1",
        temporary_policy: "fresh-owned-0700-empty-tmp-per-process-v1",
        lang: RUNNER_LOCALE,
        lc_all: RUNNER_LOCALE,
        executables,
    })
}

fn hash_executable(
    path: &Path,
    remaining: &mut u64,
    deadline: Instant,
) -> Result<ExecutableIdentity, TrustedCheckError> {
    #[cfg(unix)]
    let mut file = fs::File::from(
        rustix::fs::open(
            path,
            rustix::fs::OFlags::RDONLY
                | rustix::fs::OFlags::NOFOLLOW
                | rustix::fs::OFlags::NONBLOCK,
            rustix::fs::Mode::empty(),
        )
        .map_err(|_| TrustedCheckError::EnvironmentDrift)?,
    );
    #[cfg(not(unix))]
    let mut file = fs::File::open(path).map_err(|_| TrustedCheckError::EnvironmentDrift)?;
    let before = file
        .metadata()
        .map_err(|_| TrustedCheckError::EnvironmentDrift)?;
    if !before.is_file() || before.len() > MAX_EXECUTABLE_BYTES || before.len() > *remaining {
        return Err(TrustedCheckError::Invalid(
            "executable exceeds hashing bounds",
        ));
    }
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 32 * 1024];
    let mut bytes = 0_u64;
    loop {
        if Instant::now() >= deadline {
            return Err(TrustedCheckError::TimedOut);
        }
        let count = file
            .read(&mut buffer)
            .map_err(|_| TrustedCheckError::EnvironmentDrift)?;
        if count == 0 {
            break;
        }
        bytes += count as u64;
        if bytes > MAX_EXECUTABLE_BYTES || bytes > *remaining || bytes > before.len() {
            return Err(TrustedCheckError::EnvironmentDrift);
        }
        hasher.update(&buffer[..count]);
    }
    let after = file
        .metadata()
        .map_err(|_| TrustedCheckError::EnvironmentDrift)?;
    let named = fs::symlink_metadata(path).map_err(|_| TrustedCheckError::EnvironmentDrift)?;
    if bytes != before.len()
        || !same_executable_metadata(&before, &after)
        || !same_executable_metadata(&after, &named)
    {
        return Err(TrustedCheckError::EnvironmentDrift);
    }
    *remaining -= bytes;
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        after.permissions().mode()
    };
    #[cfg(not(unix))]
    let mode = u32::from(after.permissions().readonly());
    Ok(ExecutableIdentity {
        path: path.to_path_buf(),
        sha256: format!("sha256:{:x}", hasher.finalize()),
        bytes,
        mode,
    })
}

fn same_executable_metadata(a: &fs::Metadata, b: &fs::Metadata) -> bool {
    if !a.is_file() || !b.is_file() || a.len() != b.len() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        a.dev() == b.dev()
            && a.ino() == b.ino()
            && a.mode() == b.mode()
            && a.mtime() == b.mtime()
            && a.mtime_nsec() == b.mtime_nsec()
            && a.ctime() == b.ctime()
            && a.ctime_nsec() == b.ctime_nsec()
    }
    #[cfg(not(unix))]
    {
        a.modified().ok() == b.modified().ok()
            && a.permissions().readonly() == b.permissions().readonly()
    }
}

fn validate_request(
    inputs: &VerificationInputs,
    contract: &TaskContract,
    policy: &VerificationPolicy,
    run_prefix: &str,
) -> Result<(), TrustedCheckError> {
    if contract.validate().is_err()
        || contract.project_id != inputs.project_id
        || contract.root_id != inputs.root_id
        || contract.task_id != inputs.task_id
        || contract.revision != inputs.contract_revision
        || inputs.protected_oracle_digest != policy.protected_oracle_digest
        || run_prefix.is_empty()
        || run_prefix.len() > 80
        || !run_prefix
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        || contract.checks.len() + policy.required_negative_checks.len() > MAX_TRUSTED_CHECKS
    {
        return Err(TrustedCheckError::Invalid(
            "contract, input identity, policy binding, run ID, or check count is invalid",
        ));
    }
    if content_digest(contract).map_err(|_| TrustedCheckError::Invalid("contract digest failed"))?
        != inputs.contract_digest
        || content_digest(policy).map_err(|_| TrustedCheckError::Invalid("policy digest failed"))?
            != inputs.verification_policy_digest
    {
        return Err(TrustedCheckError::Invalid(
            "contract or verification policy digest mismatch",
        ));
    }
    if policy.required_negative_checks.is_empty()
        || contract.checks.is_empty()
        || contract
            .checks
            .iter()
            .any(|check| check.timeout_secs == 0 || check.timeout_secs > MAX_CHECK_TIMEOUT_SECS)
        || policy.required_negative_checks.iter().any(|check| {
            check.check.timeout_secs == 0
                || check.check.timeout_secs > MAX_CHECK_TIMEOUT_SECS
                || !(0..=255).contains(&check.expected_exit_code)
        })
    {
        return Err(TrustedCheckError::Invalid(
            "checks must be present and within runner bounds",
        ));
    }
    Ok(())
}

fn validate_candidate(path: &Path) -> Result<PathBuf, TrustedCheckError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| TrustedCheckError::CandidateCustody)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(TrustedCheckError::CandidateCustody);
    }
    let canonical = path
        .canonicalize()
        .map_err(|_| TrustedCheckError::CandidateCustody)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let metadata = fs::metadata(&canonical).map_err(|_| TrustedCheckError::CandidateCustody)?;
        if metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.permissions().mode() & 0o077 != 0
        {
            return Err(TrustedCheckError::CandidateCustody);
        }
    }
    let git_dir = canonical.join(".git");
    let git_metadata =
        fs::symlink_metadata(&git_dir).map_err(|_| TrustedCheckError::CandidateCustody)?;
    if git_metadata.file_type().is_symlink() || !git_metadata.is_dir() {
        return Err(TrustedCheckError::CandidateCustody);
    }
    Ok(canonical)
}

fn validate_private_directory(path: &Path) -> Result<PathBuf, TrustedCheckError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| TrustedCheckError::CandidateCustody)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(TrustedCheckError::CandidateCustody);
    }
    let canonical = path
        .canonicalize()
        .map_err(|_| TrustedCheckError::CandidateCustody)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let metadata = fs::metadata(&canonical).map_err(|_| TrustedCheckError::CandidateCustody)?;
        if metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.permissions().mode() & 0o077 != 0
        {
            return Err(TrustedCheckError::CandidateCustody);
        }
    }
    Ok(canonical)
}

fn trusted_search_path() -> Result<OsString, TrustedCheckError> {
    let inherited = std::env::var_os("PATH")
        .ok_or(TrustedCheckError::Invalid("trusted PATH is unavailable"))?;
    if inherited.is_empty() || inherited.len() > MAX_TRUSTED_PATH_BYTES {
        return Err(TrustedCheckError::Invalid(
            "trusted PATH is invalid or oversized",
        ));
    }
    let mut trusted_directories = Vec::new();
    for directory in std::env::split_paths(&inherited) {
        if directory.as_os_str().is_empty() || !directory.is_absolute() {
            continue;
        }
        let Ok(canonical) = directory.canonicalize() else {
            continue;
        };
        let Ok(metadata) = fs::metadata(&canonical) else {
            continue;
        };
        if !metadata.is_dir() {
            continue;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            let uid = metadata.uid();
            if (uid != 0 && uid != rustix::process::geteuid().as_raw())
                || metadata.permissions().mode() & 0o022 != 0
            {
                continue;
            }
        }
        #[cfg(not(unix))]
        {
            return Err(TrustedCheckError::Invalid(
                "trusted check environment is unsupported on this platform",
            ));
        }
        if !trusted_directories.contains(&canonical) {
            trusted_directories.push(canonical);
        }
        if trusted_directories.len() > 64 {
            return Err(TrustedCheckError::Invalid(
                "trusted PATH has too many entries",
            ));
        }
    }
    if trusted_directories.is_empty() {
        return Err(TrustedCheckError::Invalid("trusted PATH is empty"));
    }
    std::env::join_paths(trusted_directories)
        .map_err(|_| TrustedCheckError::Invalid("trusted PATH could not be encoded"))
}

fn resolve_trusted_git(search_path: &OsStr) -> Result<PathBuf, TrustedCheckError> {
    for directory in std::env::split_paths(search_path) {
        let Ok(program) = directory.join("git").canonicalize() else {
            continue;
        };
        let Ok(metadata) = fs::metadata(&program) else {
            continue;
        };
        if !executable_file(&program) {
            continue;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            if metadata.uid() != 0 || metadata.permissions().mode() & 0o022 != 0 {
                continue;
            }
        }
        return Ok(program);
    }
    Err(TrustedCheckError::Invalid(
        "trusted system Git executable is unavailable",
    ))
}

fn source_git_args(args: &[&str]) -> Vec<String> {
    let mut result = [
        "--no-pager",
        "-c",
        "core.fsmonitor=false",
        "-c",
        "core.untrackedCache=false",
        "-c",
        "core.excludesFile=/dev/null",
        "-c",
        "core.hooksPath=/dev/null",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<Vec<_>>();
    result.extend(args.iter().map(|arg| (*arg).to_owned()));
    result
}

fn prepare_child_environment(
    spool_root: &Path,
    run_id: &str,
    search_path: &OsStr,
) -> Result<Vec<(OsString, OsString)>, TrustedCheckError> {
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    let environment_root = spool_root.join(format!(".trusted-env-{run_id}"));
    fs::create_dir(&environment_root).map_err(|_| TrustedCheckError::CandidateCustody)?;
    #[cfg(unix)]
    fs::set_permissions(&environment_root, fs::Permissions::from_mode(0o700))
        .map_err(|_| TrustedCheckError::CandidateCustody)?;
    let environment_root = validate_private_directory(&environment_root)?;

    let home = environment_root.join("home");
    let temporary = environment_root.join("tmp");
    for directory in [&home, &temporary] {
        fs::create_dir(directory).map_err(|_| TrustedCheckError::CandidateCustody)?;
        #[cfg(unix)]
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
            .map_err(|_| TrustedCheckError::CandidateCustody)?;
        validate_private_directory(directory)?;
    }
    let home = home
        .canonicalize()
        .map_err(|_| TrustedCheckError::CandidateCustody)?;
    let temporary = temporary
        .canonicalize()
        .map_err(|_| TrustedCheckError::CandidateCustody)?;

    Ok(vec![
        (OsString::from("PATH"), search_path.to_os_string()),
        (OsString::from("HOME"), home.into_os_string()),
        (OsString::from("TMPDIR"), temporary.into_os_string()),
        (OsString::from("LANG"), OsString::from(RUNNER_LOCALE)),
        (OsString::from("LC_ALL"), OsString::from(RUNNER_LOCALE)),
    ])
}

fn source_identity(
    run: &mut RunContext<'_>,
    git_program: &Path,
) -> Result<SourceIdentity, TrustedCheckError> {
    let candidate = run.candidate.to_path_buf();
    let status = run_owned(
        run,
        git_program,
        &source_git_args(&[
            "status",
            "--porcelain=v2",
            "--branch",
            "--untracked-files=all",
        ]),
        &candidate,
        SOURCE_PROBE_TIMEOUT_SECS,
        MAX_STATUS_OUTPUT_BYTES,
    )?;
    if status.exit_code != 0
        || status.stdout.len() > MAX_STATUS_OUTPUT_BYTES
        || status
            .stdout
            .split(|byte| *byte == b'\n')
            .any(|line| !line.is_empty() && !line.starts_with(b"# "))
    {
        return Err(TrustedCheckError::CandidateDrift);
    }
    let tree = run_owned(
        run,
        git_program,
        &source_git_args(&["rev-parse", "--verify", "HEAD^{tree}"]),
        &candidate,
        SOURCE_PROBE_TIMEOUT_SECS,
        256,
    )?;
    if tree.exit_code != 0 || tree.stdout.len() > 128 {
        return Err(TrustedCheckError::CandidateDrift);
    }
    let tree_id = std::str::from_utf8(&tree.stdout)
        .map_err(|_| TrustedCheckError::CandidateDrift)?
        .trim()
        .to_owned();
    let status_text =
        std::str::from_utf8(&status.stdout).map_err(|_| TrustedCheckError::CandidateDrift)?;
    let commit = status_text
        .lines()
        .find_map(|line| line.strip_prefix("# branch.oid "))
        .filter(|value| valid_git_object_id(value))
        .ok_or(TrustedCheckError::CandidateDrift)?
        .to_owned();
    if !valid_git_object_id(&tree_id) {
        return Err(TrustedCheckError::CandidateDrift);
    }
    let status_after_tree = run_owned(
        run,
        git_program,
        &source_git_args(&[
            "status",
            "--porcelain=v2",
            "--branch",
            "--untracked-files=all",
        ]),
        &candidate,
        SOURCE_PROBE_TIMEOUT_SECS,
        MAX_STATUS_OUTPUT_BYTES,
    )?;
    if status_after_tree.exit_code != 0
        || status_after_tree.stdout != status.stdout
        || status_after_tree
            .stdout
            .split(|byte| *byte == b'\n')
            .any(|line| !line.is_empty() && !line.starts_with(b"# "))
    {
        return Err(TrustedCheckError::CandidateDrift);
    }
    Ok(SourceIdentity {
        commit,
        tree: tree_id,
    })
}

fn valid_git_object_id(value: &str) -> bool {
    (value.len() == 40 || value.len() == 64)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn execute_one(
    run: &mut RunContext<'_>,
    check: &CheckSpec,
    expected_exit: i32,
) -> Result<ProcessOutput, TrustedCheckError> {
    let cwd = check_cwd(run.candidate, check)?;
    let program = resolve_program(&check.program, &cwd, run.search_path)?;
    let timeout = check.timeout_secs.min(MAX_CHECK_TIMEOUT_SECS);
    let output = run_owned(
        run,
        &program,
        &check.args,
        &cwd,
        timeout,
        MAX_CHECK_OUTPUT_BYTES,
    )?;
    if output.exit_code != expected_exit {
        return Err(TrustedCheckError::ExitMismatch);
    }
    Ok(output)
}

fn check_cwd(candidate: &Path, check: &CheckSpec) -> Result<PathBuf, TrustedCheckError> {
    let cwd = candidate.join(&check.cwd);
    let cwd = cwd
        .canonicalize()
        .map_err(|_| TrustedCheckError::Invalid("check cwd is unavailable"))?;
    if !cwd.starts_with(candidate) || !cwd.is_dir() {
        return Err(TrustedCheckError::Invalid(
            "check cwd escapes the candidate checkout",
        ));
    }
    Ok(cwd)
}

fn run_owned(
    run: &mut RunContext<'_>,
    program: &Path,
    args: &[String],
    cwd: &Path,
    timeout_secs: u32,
    output_limit: usize,
) -> Result<ProcessOutput, TrustedCheckError> {
    *run.sequence = run
        .sequence
        .checked_add(1)
        .ok_or(TrustedCheckError::Invalid("process sequence exhausted"))?;
    let run_id = format!("{}-{}", run.run_prefix, *run.sequence);
    if run_id.len() > 128 {
        return Err(TrustedCheckError::Invalid(
            "run ID exceeds SessionHost bound",
        ));
    }
    let spool_dir = run.spool.join(&run_id);
    let spec = LaunchSpec {
        run_id: run_id.clone(),
        program: program.to_path_buf(),
        args: args.to_vec(),
        cwd: cwd.to_path_buf(),
        spool_dir,
    };
    let environment = prepare_child_environment(run.spool, &run_id, run.search_path)?;
    run.host
        .launch_with_environment(spec, &environment)
        .map_err(session_error)?;
    if let Err(error) = run.host.close_input(&run_id) {
        let _ = run.host.terminate(&run_id);
        return Err(session_error(error));
    }
    let deadline = Instant::now() + Duration::from_secs(u64::from(timeout_secs));
    loop {
        if Instant::now() >= deadline {
            run.host.terminate(&run_id).map_err(session_error)?;
            return Err(TrustedCheckError::TimedOut);
        }
        let status = match run.host.status(&run_id) {
            Ok(status) => status,
            Err(SessionError::Busy) => {
                thread::sleep(POLL_INTERVAL);
                continue;
            }
            Err(error) => {
                let _ = run.host.terminate(&run_id);
                return Err(session_error(error));
            }
        };
        if status.input_outcome_unknown || status.capture_failed {
            let _ = run.host.terminate(&run_id);
            return Err(TrustedCheckError::CaptureIncomplete);
        }
        if status.output_offset > output_limit as u64 {
            let _ = run.host.terminate(&run_id);
            return Err(TrustedCheckError::OutputLimit);
        }
        if status.exit.is_some() {
            break;
        }
        thread::sleep(POLL_INTERVAL);
    }
    let exit = match run.host.wait(&run_id) {
        Ok(exit) => exit,
        Err(error) => {
            let _ = run.host.terminate(&run_id);
            return Err(session_error(error));
        }
    };
    let status = match run.host.status(&run_id) {
        Ok(status) => status,
        Err(error) => {
            let _ = run.host.terminate(&run_id);
            return Err(session_error(error));
        }
    };
    if status.capture_failed
        || status.capture_state != CaptureState::Complete
        || status.exit.is_none()
        || status.input_outcome_unknown
        || status.final_output_offset != Some(status.output_offset)
        || status.output_offset > output_limit as u64
    {
        return Err(TrustedCheckError::CaptureIncomplete);
    }
    let exit_code = exit.code().ok_or(TrustedCheckError::CaptureIncomplete)?;
    let (stdout, stderr) =
        read_complete_output(run.host, &run_id, status.final_output_offset.unwrap_or(0))?;
    Ok(ProcessOutput {
        exit_code,
        stdout,
        stderr,
    })
}

fn read_complete_output(
    host: &SessionHost,
    run_id: &str,
    final_offset: u64,
) -> Result<(Vec<u8>, Vec<u8>), TrustedCheckError> {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut offset = 0_u64;
    while offset < final_offset {
        let page = host
            .tail(run_id, offset, MAX_TAIL_PAGE_BYTES)
            .map_err(session_error)?;
        if page.next_offset <= offset || page.chunks.is_empty() {
            return Err(TrustedCheckError::CaptureIncomplete);
        }
        let mut page_offset = offset;
        for chunk in page.chunks {
            if chunk.offset != page_offset || chunk.bytes.is_empty() {
                return Err(TrustedCheckError::CaptureIncomplete);
            }
            page_offset = page_offset
                .checked_add(chunk.bytes.len() as u64)
                .ok_or(TrustedCheckError::CaptureIncomplete)?;
            match chunk.stream {
                OutputStream::Stdout => stdout.extend_from_slice(&chunk.bytes),
                OutputStream::Stderr => stderr.extend_from_slice(&chunk.bytes),
            }
        }
        if page_offset != page.next_offset {
            return Err(TrustedCheckError::CaptureIncomplete);
        }
        offset = page.next_offset;
        if stdout.len().saturating_add(stderr.len()) > MAX_CHECK_OUTPUT_BYTES {
            return Err(TrustedCheckError::OutputLimit);
        }
    }
    if offset != final_offset {
        return Err(TrustedCheckError::CaptureIncomplete);
    }
    Ok((stdout, stderr))
}

fn check_result(check: &CheckSpec, output: ProcessOutput) -> CheckResult {
    CheckResult {
        check: check.clone(),
        exit_code: Some(output.exit_code),
        stdout_digest: sha256(&output.stdout),
        stderr_digest: sha256(&output.stderr),
    }
}

fn sha256(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn resolve_program(
    program: &str,
    candidate_dir: &Path,
    search_path: &OsStr,
) -> Result<PathBuf, TrustedCheckError> {
    let requested = Path::new(program);
    let resolved = if requested.is_absolute() {
        requested.to_path_buf()
    } else if program.contains('/') || program.contains('\\') {
        candidate_dir.join(requested)
    } else {
        std::env::split_paths(search_path)
            .map(|directory| directory.join(OsStr::new(program)))
            .find(|path| executable_file(path))
            .ok_or(TrustedCheckError::Invalid(
                "trusted check program is unavailable",
            ))?
    };
    let canonical = resolved
        .canonicalize()
        .map_err(|_| TrustedCheckError::Invalid("trusted check program is unavailable"))?;
    if !executable_file(&canonical) {
        return Err(TrustedCheckError::Invalid(
            "trusted check program is not an executable regular file",
        ));
    }
    if !requested.is_absolute()
        && (program.contains('/') || program.contains('\\'))
        && !canonical.starts_with(candidate_dir)
    {
        return Err(TrustedCheckError::Invalid(
            "relative check program escapes candidate checkout",
        ));
    }
    Ok(canonical)
}

fn executable_file(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn session_error(error: SessionError) -> TrustedCheckError {
    match error {
        SessionError::Io(error) => TrustedCheckError::Process(error.to_string()),
        _ => TrustedCheckError::Process("owned session failed or became ambiguous".into()),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use admiralty_core::{
        AcceptanceCriterion, AttemptId, PrincipalId, ProjectId, RootId, TaskId,
        verification::RequiredNegativeCheck,
    };
    use std::{io, os::unix::fs::PermissionsExt, process::Command};

    struct GitFixture {
        _temp: tempfile::TempDir,
        checkout: PathBuf,
        spool: PathBuf,
        inputs: VerificationInputs,
        contract: TaskContract,
        policy: VerificationPolicy,
    }

    fn fixture_command(program: &str, home: &Path) -> Command {
        let mut command = Command::new(program);
        command.env_clear();
        if let Some(path) = std::env::var_os("PATH") {
            command.env("PATH", path);
        }
        command.env("HOME", home).env("GIT_CONFIG_NOSYSTEM", "1");
        command
    }

    fn git(path: &Path, args: &[&str]) -> io::Result<Vec<u8>> {
        let home = path.parent().unwrap_or(path);
        let output = fixture_command("git", home)
            .arg("-C")
            .arg(path)
            .args(args)
            .output()?;
        if !output.status.success() {
            return Err(io::Error::other("git fixture command failed"));
        }
        Ok(output.stdout)
    }

    fn fixture(positive_program: &str, positive_args: Vec<String>) -> io::Result<GitFixture> {
        let temp = tempfile::tempdir()?;
        fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700))?;
        let checkout = temp.path().join("candidate");
        let spool = temp.path().join("spool");
        fs::create_dir(&checkout)?;
        fs::create_dir(&spool)?;
        fs::set_permissions(&checkout, fs::Permissions::from_mode(0o700))?;
        fs::set_permissions(&spool, fs::Permissions::from_mode(0o700))?;
        let output = fixture_command("git", temp.path())
            .arg("-C")
            .arg(&checkout)
            .args(["init", "--quiet"])
            .output()?;
        if !output.status.success() {
            return Err(io::Error::other("git init failed"));
        }
        fs::write(checkout.join("README.md"), b"trusted fixture source\n")?;
        for args in [
            vec!["config", "user.name", "Trusted Check Fixture"],
            vec!["config", "user.email", "fixture@example.invalid"],
            vec!["add", "README.md"],
            vec!["commit", "--quiet", "-m", "fixture"],
        ] {
            let output = fixture_command("git", temp.path())
                .arg("-C")
                .arg(&checkout)
                .args(args)
                .output()?;
            if !output.status.success() {
                return Err(io::Error::other("git fixture commit failed"));
            }
        }
        let commit =
            String::from_utf8(git(&checkout, &["rev-parse", "HEAD"])?).map_err(io::Error::other)?;
        let tree = String::from_utf8(git(&checkout, &["rev-parse", "HEAD^{tree}"])?)
            .map_err(io::Error::other)?;
        let positive = CheckSpec {
            id: "candidate_head".into(),
            program: positive_program.into(),
            args: positive_args,
            cwd: ".".into(),
            timeout_secs: 3,
        };
        let negative = CheckSpec {
            id: "missing_object".into(),
            program: "git".into(),
            args: vec![
                "-C".into(),
                checkout.to_string_lossy().into_owned(),
                "cat-file".into(),
                "-e".into(),
                "ffffffffffffffffffffffffffffffffffffffff^{commit}".into(),
            ],
            cwd: ".".into(),
            timeout_secs: 3,
        };
        let contract = TaskContract {
            schema_version: 1,
            project_id: ProjectId::new("trusted-project").map_err(io::Error::other)?,
            root_id: RootId::new("trusted-root").map_err(io::Error::other)?,
            task_id: TaskId::new("trusted-task").map_err(io::Error::other)?,
            revision: 1,
            title: "Trusted check fixture".into(),
            objective: "Execute exact trusted argv".into(),
            non_goals: Vec::new(),
            owned_paths: vec!["README.md".into()],
            forbidden_paths: Vec::new(),
            read_first: Vec::new(),
            dependencies: Vec::new(),
            criteria: vec![AcceptanceCriterion {
                id: "real_checks".into(),
                text: "Both checks execute with expected exits".into(),
                check_ids: vec![positive.id.clone()],
            }],
            checks: vec![positive],
            authority_ref: "authority:trusted-check-test".into(),
            max_attempts: 1,
            simulated: false,
        };
        let policy = VerificationPolicy {
            schema_version: 1,
            verifier: PrincipalId::new("trusted-verifier").map_err(io::Error::other)?,
            protected_oracle_digest: format!("sha256:{}", "d".repeat(64)),
            required_negative_checks: vec![RequiredNegativeCheck {
                check: negative,
                expected_exit_code: 128,
                fixture_digest: format!("sha256:{}", "e".repeat(64)),
            }],
        };
        let inputs = VerificationInputs {
            project_id: contract.project_id.clone(),
            root_id: contract.root_id.clone(),
            task_id: contract.task_id.clone(),
            attempt_id: AttemptId::new("trusted-attempt").map_err(io::Error::other)?,
            candidate_ref: "candidate:trusted-fixture".into(),
            contract_revision: contract.revision,
            contract_digest: content_digest(&contract).map_err(io::Error::other)?,
            verification_policy_digest: content_digest(&policy).map_err(io::Error::other)?,
            source_commit_sha: commit.trim().into(),
            source_tree_sha: tree.trim().into(),
            source_artifact_digest: format!("sha256:{}", "c".repeat(64)),
            protected_oracle_digest: policy.protected_oracle_digest.clone(),
        };
        Ok(GitFixture {
            _temp: temp,
            checkout,
            spool,
            inputs,
            contract,
            policy,
        })
    }

    #[test]
    fn executes_real_positive_and_negative_argv_and_returns_opaque_evidence() {
        let fixture = fixture(
            "git",
            vec![
                "-C".into(),
                "candidate".into(),
                "rev-parse".into(),
                "--verify".into(),
                "HEAD".into(),
            ],
        )
        .expect("owned git fixture");
        let mut contract = fixture.contract.clone();
        contract.checks[0].args[1] = fixture.checkout.to_string_lossy().into_owned();
        let mut inputs = fixture.inputs.clone();
        inputs.contract_digest = content_digest(&contract).expect("contract digest");
        inputs.verification_policy_digest = content_digest(&fixture.policy).expect("policy digest");
        let host = SessionHost::new();
        let evidence = execute_checks(
            &host,
            &inputs,
            &contract,
            &fixture.policy,
            &fixture.checkout,
            &fixture.spool,
            "positive-negative",
        )
        .expect("real checks pass with exact expected exits");
        assert_eq!(evidence.receipt().inputs, inputs);
        assert_eq!(evidence.receipt().checks.len(), 1);
        assert_eq!(evidence.receipt().negative_checks[0].exit_code, Some(128));
        assert!(!evidence.execution_digest().is_empty());
    }

    #[test]
    fn rejects_source_mismatch_before_launch() {
        let fixture = fixture("git", vec!["status".into()]).expect("owned git fixture");
        let mut inputs = fixture.inputs.clone();
        inputs.source_commit_sha = "a".repeat(40);
        let host = SessionHost::new();
        let result = execute_checks(
            &host,
            &inputs,
            &fixture.contract,
            &fixture.policy,
            &fixture.checkout,
            &fixture.spool,
            "mismatch",
        );
        assert!(matches!(result, Err(TrustedCheckError::CandidateDrift)));
    }

    #[test]
    fn rejects_check_exit_mismatch() {
        let fixture = fixture("git", vec!["-C".into(), ".".into(), "bad-command".into()])
            .expect("owned git fixture");
        let host = SessionHost::new();
        let result = execute_checks(
            &host,
            &fixture.inputs,
            &fixture.contract,
            &fixture.policy,
            &fixture.checkout,
            &fixture.spool,
            "exit-mismatch",
        );
        assert!(matches!(result, Err(TrustedCheckError::ExitMismatch)));
    }

    #[test]
    fn terminates_owned_timed_out_check_and_reports_no_evidence() {
        let fixture = fixture("sleep", vec!["5".into()]).expect("owned git fixture");
        let mut contract = fixture.contract.clone();
        contract.checks[0].timeout_secs = 1;
        let mut inputs = fixture.inputs.clone();
        inputs.contract_digest = content_digest(&contract).expect("contract digest");
        let host = SessionHost::new();
        let result = execute_checks(
            &host,
            &inputs,
            &contract,
            &fixture.policy,
            &fixture.checkout,
            &fixture.spool,
            "timed-out",
        );
        assert!(matches!(result, Err(TrustedCheckError::TimedOut)));
    }

    #[test]
    fn source_drift_after_a_real_check_invalidates_the_run() {
        let fixture =
            fixture("touch", vec!["generated-untracked".into()]).expect("owned git fixture");
        let mut contract = fixture.contract.clone();
        contract.checks[0].timeout_secs = 3;
        let mut inputs = fixture.inputs.clone();
        inputs.contract_digest = content_digest(&contract).expect("contract digest");
        let host = SessionHost::new();
        let result = execute_checks(
            &host,
            &inputs,
            &contract,
            &fixture.policy,
            &fixture.checkout,
            &fixture.spool,
            "drift",
        );
        assert!(matches!(result, Err(TrustedCheckError::CandidateDrift)));
    }
}
