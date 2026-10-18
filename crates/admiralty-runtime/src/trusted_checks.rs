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
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

pub const MAX_TRUSTED_CHECKS: usize = 16;
pub const MAX_CHECK_OUTPUT_BYTES: usize = 1024 * 1024;
pub const MAX_CHECK_TIMEOUT_SECS: u32 = 300;
const MAX_STATUS_OUTPUT_BYTES: usize = 256 * 1024;
const MAX_TAIL_PAGE_BYTES: usize = 256 * 1024;
const SOURCE_PROBE_TIMEOUT_SECS: u32 = 10;
const POLL_INTERVAL: Duration = Duration::from_millis(10);

#[derive(Debug, thiserror::Error)]
pub enum TrustedCheckError {
    #[error("trusted-check request is invalid: {0}")]
    Invalid(&'static str),
    #[error("candidate checkout is not an owned private isolated repository")]
    CandidateCustody,
    #[error("candidate source differs from its pinned identity or is dirty")]
    CandidateDrift,
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
    receipt_digest: String,
    execution_digest: String,
}

impl ExecutedChecks {
    pub fn receipt(&self) -> &VerificationReceipt {
        &self.receipt
    }

    /// Canonical digest of the validated check receipt for lifecycle evidence.
    /// It does not establish custody or supervised process closure.
    pub fn receipt_digest(&self) -> &str {
        &self.receipt_digest
    }

    /// Binds this run's checked source and receipt; it is not a closure digest.
    pub fn execution_digest(&self) -> &str {
        &self.execution_digest
    }
}

#[derive(Debug)]
struct ProcessOutput {
    exit_code: i32,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

#[derive(Debug, PartialEq, Eq)]
struct SourceIdentity {
    commit: String,
    tree: String,
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
    let git_program = resolve_program("git", &candidate)?;
    let mut sequence = 0_u32;
    let before = source_identity(
        host,
        &git_program,
        &candidate,
        &spool,
        run_prefix,
        &mut sequence,
    )?;
    if before.commit != inputs.source_commit_sha || before.tree != inputs.source_tree_sha {
        return Err(TrustedCheckError::CandidateDrift);
    }

    let mut results = Vec::with_capacity(contract.checks.len());
    for check in &contract.checks {
        let output = execute_one(
            host,
            check,
            0,
            &candidate,
            &spool,
            run_prefix,
            &mut sequence,
        )?;
        results.push(check_result(check, output));
    }

    let mut negative_results = Vec::with_capacity(policy.required_negative_checks.len());
    for negative in &policy.required_negative_checks {
        let output = execute_one(
            host,
            &negative.check,
            negative.expected_exit_code,
            &candidate,
            &spool,
            run_prefix,
            &mut sequence,
        )?;
        negative_results.push(check_result(&negative.check, output));
    }

    let after = source_identity(
        host,
        &git_program,
        &candidate,
        &spool,
        run_prefix,
        &mut sequence,
    )?;
    if after != before {
        return Err(TrustedCheckError::CandidateDrift);
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
    let execution_digest = content_digest(&("trusted-check-execution-v1", &before, &receipt))
        .map_err(|error| TrustedCheckError::Evidence(error.to_string()))?;
    Ok(ExecutedChecks {
        receipt,
        receipt_digest: validated.receipt_digest().to_owned(),
        execution_digest,
    })
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
    if std::env::vars_os().any(|(key, _)| {
        key.to_string_lossy()
            .to_ascii_uppercase()
            .starts_with("GIT_")
    }) {
        return Err(TrustedCheckError::Invalid(
            "Git environment overrides are not permitted",
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

fn source_identity(
    host: &SessionHost,
    git_program: &Path,
    candidate: &Path,
    spool: &Path,
    run_prefix: &str,
    sequence: &mut u32,
) -> Result<SourceIdentity, TrustedCheckError> {
    let status = run_owned(
        host,
        git_program,
        &[
            "-c".into(),
            "core.fsmonitor=false".into(),
            "status".into(),
            "--porcelain=v2".into(),
            "--branch".into(),
            "--untracked-files=all".into(),
        ],
        candidate,
        spool,
        run_prefix,
        sequence,
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
        host,
        git_program,
        &["rev-parse".into(), "--verify".into(), "HEAD^{tree}".into()],
        candidate,
        spool,
        run_prefix,
        sequence,
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
        host,
        git_program,
        &[
            "-c".into(),
            "core.fsmonitor=false".into(),
            "status".into(),
            "--porcelain=v2".into(),
            "--branch".into(),
            "--untracked-files=all".into(),
        ],
        candidate,
        spool,
        run_prefix,
        sequence,
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
    host: &SessionHost,
    check: &CheckSpec,
    expected_exit: i32,
    candidate: &Path,
    spool: &Path,
    run_prefix: &str,
    sequence: &mut u32,
) -> Result<ProcessOutput, TrustedCheckError> {
    let cwd = candidate.join(&check.cwd);
    let cwd = cwd
        .canonicalize()
        .map_err(|_| TrustedCheckError::Invalid("check cwd is unavailable"))?;
    if !cwd.starts_with(candidate) || !cwd.is_dir() {
        return Err(TrustedCheckError::Invalid(
            "check cwd escapes the candidate checkout",
        ));
    }
    let program = resolve_program(&check.program, &cwd)?;
    let timeout = check.timeout_secs.min(MAX_CHECK_TIMEOUT_SECS);
    let output = run_owned(
        host,
        &program,
        &check.args,
        &cwd,
        spool,
        run_prefix,
        sequence,
        timeout,
        MAX_CHECK_OUTPUT_BYTES,
    )?;
    if output.exit_code != expected_exit {
        return Err(TrustedCheckError::ExitMismatch);
    }
    Ok(output)
}

fn run_owned(
    host: &SessionHost,
    program: &Path,
    args: &[String],
    cwd: &Path,
    spool_root: &Path,
    run_prefix: &str,
    sequence: &mut u32,
    timeout_secs: u32,
    output_limit: usize,
) -> Result<ProcessOutput, TrustedCheckError> {
    *sequence = sequence
        .checked_add(1)
        .ok_or(TrustedCheckError::Invalid("process sequence exhausted"))?;
    let run_id = format!("{run_prefix}-{sequence}");
    if run_id.len() > 128 {
        return Err(TrustedCheckError::Invalid(
            "run ID exceeds SessionHost bound",
        ));
    }
    let spool_dir = spool_root.join(&run_id);
    let spec = LaunchSpec {
        run_id: run_id.clone(),
        program: program.to_path_buf(),
        args: args.to_vec(),
        cwd: cwd.to_path_buf(),
        spool_dir,
    };
    host.launch(spec).map_err(session_error)?;
    if let Err(error) = host.close_input(&run_id) {
        let _ = host.terminate(&run_id);
        return Err(session_error(error));
    }
    let deadline = Instant::now() + Duration::from_secs(u64::from(timeout_secs));
    loop {
        let status = match host.status(&run_id) {
            Ok(status) => status,
            Err(SessionError::Busy) => {
                thread::sleep(POLL_INTERVAL);
                continue;
            }
            Err(error) => {
                let _ = host.terminate(&run_id);
                return Err(session_error(error));
            }
        };
        if status.input_outcome_unknown || status.capture_failed {
            let _ = host.terminate(&run_id);
            return Err(TrustedCheckError::CaptureIncomplete);
        }
        if status.output_offset > output_limit as u64 {
            let _ = host.terminate(&run_id);
            return Err(TrustedCheckError::OutputLimit);
        }
        if status.exit.is_some() {
            break;
        }
        if Instant::now() >= deadline {
            host.terminate(&run_id).map_err(session_error)?;
            return Err(TrustedCheckError::TimedOut);
        }
        thread::sleep(POLL_INTERVAL);
    }
    let exit = match host.wait(&run_id) {
        Ok(exit) => exit,
        Err(error) => {
            let _ = host.terminate(&run_id);
            return Err(session_error(error));
        }
    };
    let status = match host.status(&run_id) {
        Ok(status) => status,
        Err(error) => {
            let _ = host.terminate(&run_id);
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
        read_complete_output(host, &run_id, status.final_output_offset.unwrap_or(0))?;
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

fn resolve_program(program: &str, candidate_dir: &Path) -> Result<PathBuf, TrustedCheckError> {
    let requested = Path::new(program);
    let resolved = if requested.is_absolute() {
        requested.to_path_buf()
    } else if program.contains('/') || program.contains('\\') {
        candidate_dir.join(requested)
    } else {
        let path = std::env::var_os("PATH")
            .ok_or(TrustedCheckError::Invalid("execution PATH is unavailable"))?;
        std::env::split_paths(&path)
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

    fn git(path: &Path, args: &[&str]) -> io::Result<Vec<u8>> {
        let output = Command::new("git")
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
        let checkout = temp.path().join("candidate");
        let spool = temp.path().join("spool");
        fs::create_dir(&checkout)?;
        fs::create_dir(&spool)?;
        fs::set_permissions(&checkout, fs::Permissions::from_mode(0o700))?;
        fs::set_permissions(&spool, fs::Permissions::from_mode(0o700))?;
        let output = Command::new("git")
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
            let output = Command::new("git")
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
        assert_eq!(
            evidence.receipt_digest(),
            content_digest(evidence.receipt())
                .expect("receipt digest")
                .as_str()
        );
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
