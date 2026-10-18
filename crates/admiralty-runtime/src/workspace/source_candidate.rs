//! Post-execution changed-source candidate custody and immutable checkout materialization.
//!
//! # Integration Contract
//!
//! This module bridges pinned workspace preparation (`PreparedWorkspace`),
//! resource-gated service closure (`ClosedAdmission`), conflict analysis
//! (`admiralty_core::conflicts::CandidateChange`), independent trusted-check
//! execution (`crate::trusted_checks::execute_checks`), and native lifecycle
//! candidate decisions (`admiralty_core::CandidateDecisionEvidence`):
//!
//! 1. **`NativeExecutor` handoff (`PendingRun` / `ClosedExecution`):**
//!    - `PendingRun` and `ClosedExecution` retain the run's opaque
//!      [`PreparedWorkspace`] handle alongside [`ClosedAdmission`].
//!    - Once the owned service exits cleanly and `RunningAdmission::prove_closed`
//!      mints a [`ClosedAdmission`], trusted composition calls
//!      [`prove_closed_workspace`] to bind the [`PreparedWorkspace`] and
//!      [`ClosedAdmission`] into an opaque, non-deserializable
//!      [`ClosedWorkspaceExecution`].
//!    - Raw filesystem paths, agent JSON claims, or captured session transcript
//!      bytes/digests are never accepted as workspace custody or candidate source.
//!
//! 2. **`controller` lifecycle flow (`TaskStarted` -> `CandidateSubmitted` -> `DecideCandidate`):**
//!    - After recording `DomainEvent::TaskStarted`, trusted composition calls
//!      [`materialize_source_candidate`] with the [`ClosedWorkspaceExecution`] and
//!      a private mode-`0700` `candidate_parent` directory.
//!    - [`materialize_source_candidate`] re-validates workspace custody and `.git`
//!      metadata, freezes dirty working-tree edits or verifies pre-committed
//!      provider commits on top of `base_commit_sha`, enforces `owned_paths` and
//!      `forbidden_paths`, validates the resulting [`CandidateChange`] via
//!      [`admiralty_core::conflicts::parse_candidates`], and materializes an
//!      immutable, clean, detached mode-`0700` checkout at `source_commit_sha`.
//!    - Trusted composition emits [`SourceCandidate::candidate_submitted_event`]
//!      (`DomainEvent::CandidateSubmitted`), runs
//!      [`crate::trusted_checks::execute_checks`] directly against
//!      [`SourceCandidate::checkout_path`] using
//!      [`SourceCandidate::verification_inputs`], and binds the resulting
//!      [`ExecutedChecks`] into [`CandidateDecisionEvidence`] via
//!      [`SourceCandidate::decision_evidence`] for `LifecycleAction::DecideCandidate`.
//!
//! 3. **Qualification Gate:**
//!    - `native_execution_qualified` remains `false` until Root wires central
//!      startup and controller composition end-to-end.

use crate::{
    resource_gate::{ClosedAdmission, EnforcementObservation, RecoveryIdentity},
    trusted_checks::ExecutedChecks,
    workspace::{
        preparation::PreparedWorkspace,
        registry::{Custody, GitMetadata, ValidatedWorkspace, WorkspaceSpec},
    },
};
use admiralty_core::{
    AttemptId, CandidateDecisionEvidence, DomainEvent, PrincipalId, ProjectId, RootId, TaskId,
    conflicts::{CandidateChange, CandidateConstraints, FileChange, HunkRange, parse_candidates},
    content_digest,
    contracts::safe_relative_path,
    verification::{ReceiptProducer, VerificationInputs},
};
use rustix::fs::{AtFlags, Mode, OFlags, fstat, mkdirat, open, openat, statat};
use std::{
    collections::BTreeSet,
    fs,
    io::{self, Read},
    os::{
        fd::{AsFd, OwnedFd},
        unix::{
            fs::{FileTypeExt, MetadataExt, PermissionsExt},
            process::CommandExt,
        },
    },
    path::{Component, Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};
use thiserror::Error;

const TRUSTED_GIT: [&str; 2] = ["/usr/bin/git", "/bin/git"];
const GIT_TIMEOUT: Duration = Duration::from_secs(5);
const GIT_TRANSFER_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_GIT_OUTPUT: usize = 1024 * 1024;
const MAX_GIT_STDERR: usize = 16 * 1024;
const MAX_TREE_LISTING: usize = 4 * 1024 * 1024;
const MAX_WORKING_TREE_ENTRIES: usize = 10_000;
const MAX_WORKING_TREE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_DESTINATION_NAME_BYTES: usize = 128;
const DETERMINISTIC_AUTHOR_NAME: &str = "Admiralty Source Candidate";
const DETERMINISTIC_AUTHOR_EMAIL: &str = "source-candidate@admiralty.invalid";
const DETERMINISTIC_COMMIT_DATE: &str = "1970-01-01T00:00:00Z";

mod sealed {
    pub trait Sealed {}
}

/// Sealed proof trait implemented only for [`ClosedAdmission`] (and in-module unit test fixtures).
/// External callers cannot implement this trait or forge closure proofs from JSON.
pub trait ClosedAdmissionProof: sealed::Sealed {
    fn identity(&self) -> &RecoveryIdentity;
    fn observation(&self) -> &EnforcementObservation;
    fn exit_code(&self) -> Option<i32>;
    fn success(&self) -> bool;
    fn closure_digest(&self) -> &str;
    fn into_closed_admission(self) -> Option<ClosedAdmission>;
}

impl sealed::Sealed for ClosedAdmission {}

impl ClosedAdmissionProof for ClosedAdmission {
    fn identity(&self) -> &RecoveryIdentity {
        ClosedAdmission::identity(self)
    }

    fn observation(&self) -> &EnforcementObservation {
        ClosedAdmission::observation(self)
    }

    fn exit_code(&self) -> Option<i32> {
        ClosedAdmission::exit_code(self)
    }

    fn success(&self) -> bool {
        ClosedAdmission::success(self)
    }

    fn closure_digest(&self) -> &str {
        ClosedAdmission::closure_digest(self)
    }

    fn into_closed_admission(self) -> Option<ClosedAdmission> {
        Some(self)
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SourceCandidateError {
    #[error("closed service execution did not succeed with exit code 0")]
    ExecutionNotSucceeded,
    #[error("closed service identity or workspace binding does not match prepared workspace")]
    ClosureIdentityMismatch,
    #[error("prepared workspace custody or filesystem metadata is unsafe")]
    UnsafeWorkspaceMetadata,
    #[error("candidate parent or destination checkout custody is invalid")]
    InvalidCandidateDestination,
    #[error("trusted system Git executable is unavailable")]
    GitUnavailable,
    #[error("git command timed out")]
    GitTimeout,
    #[error("git command exceeded bounded output")]
    GitOutputLimit,
    #[error("git command failed: {0}")]
    GitCommand(String),
    #[error("provider execution produced no source changes relative to pinned base commit")]
    NoSourceChanges,
    #[error("post-execution HEAD does not descend from pinned base commit")]
    InvalidBaseAncestry,
    #[error("changed path violates contract write scope: {path}")]
    ScopeViolation { path: String },
    #[error("candidate diff failed conflict model validation: {0}")]
    ConflictValidation(String),
    #[error("trusted check execution receipt does not match source candidate identity")]
    VerificationBindingMismatch,
    #[error("canonical content digest could not be computed")]
    Digest,
}

/// Canonical digest of a validated workspace specification.
pub fn workspace_spec_digest(
    workspace: &ValidatedWorkspace,
) -> Result<String, SourceCandidateError> {
    content_digest(&(
        "validated-workspace-v1",
        workspace.node_id.as_str(),
        &workspace.root_id,
        &workspace.project_id,
        &workspace.workspace,
        &workspace.owned_paths,
        &workspace.forbidden_paths,
    ))
    .map_err(|_| SourceCandidateError::Digest)
}

/// Opaque, non-deserializable handoff binding a [`PreparedWorkspace`] to its
/// matching [`ClosedAdmission`].
///
/// ```compile_fail
/// use admiralty_runtime::workspace::source_candidate::ClosedWorkspaceExecution;
/// fn require_deserialize<T: serde::de::DeserializeOwned>() {}
/// require_deserialize::<ClosedWorkspaceExecution>();
/// ```
#[derive(Debug)]
pub struct ClosedWorkspaceExecution {
    prepared: PreparedWorkspace,
    identity: RecoveryIdentity,
    observation: EnforcementObservation,
    exit_code: Option<i32>,
    success: bool,
    closure_digest: String,
    workspace_digest: String,
    closed_admission: Option<ClosedAdmission>,
}

impl ClosedWorkspaceExecution {
    pub fn prepared(&self) -> &PreparedWorkspace {
        &self.prepared
    }

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

    pub fn workspace_digest(&self) -> &str {
        &self.workspace_digest
    }

    pub fn closed_admission(&self) -> Option<&ClosedAdmission> {
        self.closed_admission.as_ref()
    }
}

/// Verify that a closed service admission succeeded and matches the exact
/// task, attempt, project, root, node, and workspace custody of `prepared`.
pub fn prove_closed_workspace<C: ClosedAdmissionProof>(
    prepared: PreparedWorkspace,
    closure: C,
) -> Result<ClosedWorkspaceExecution, SourceCandidateError> {
    if !closure.success() || closure.exit_code() != Some(0) {
        return Err(SourceCandidateError::ExecutionNotSucceeded);
    }
    let closure_digest = closure.closure_digest().to_owned();
    if !valid_sha256_digest(&closure_digest) {
        return Err(SourceCandidateError::ClosureIdentityMismatch);
    }
    let ws = prepared.workspace();
    let revalidated = WorkspaceSpec {
        node_id: ws.node_id.clone(),
        root_id: ws.root_id.clone(),
        project_id: ws.project_id.clone(),
        workspace: ws.workspace.clone(),
        owned_paths: ws.owned_paths.clone(),
        forbidden_paths: ws.forbidden_paths.clone(),
    }
    .validate()
    .map_err(|_| SourceCandidateError::UnsafeWorkspaceMetadata)?;
    if &revalidated != ws
        || revalidated.custody != Custody::RegisteredOnly
        || revalidated.git_metadata != GitMetadata::Directory
    {
        return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
    }
    let workspace_digest = workspace_spec_digest(ws)?;
    let identity = closure.identity().clone();
    let observation = closure.observation().clone();
    if identity.identity.project_id != *prepared.project_id()
        || identity.identity.task_id != *prepared.task_id()
        || identity.identity.attempt_id != *prepared.attempt_id()
        || identity.identity.node_id.as_str() != ws.node_id.as_str()
        || observation.actual_cwd != ws.workspace
        || identity.unit != observation.unit
        || identity.invocation_id.as_deref() != Some(observation.invocation_id.as_str())
        || identity.main_pid != Some(observation.main_pid)
        || identity.process_start.as_deref() != Some(observation.process_start.as_str())
    {
        return Err(SourceCandidateError::ClosureIdentityMismatch);
    }
    let exit_code = closure.exit_code();
    let success = closure.success();
    let closed_admission = closure.into_closed_admission();
    Ok(ClosedWorkspaceExecution {
        prepared,
        identity,
        observation,
        exit_code,
        success,
        closure_digest,
        workspace_digest,
        closed_admission,
    })
}

/// Opaque, non-deserializable changed-source candidate holding an immutable,
/// clean, detached mode-`0700` Git checkout at `source_commit_sha`.
///
/// ```compile_fail
/// use admiralty_runtime::workspace::source_candidate::SourceCandidate;
/// fn require_deserialize<T: serde::de::DeserializeOwned>() {}
/// require_deserialize::<SourceCandidate>();
/// ```
#[derive(Debug)]
pub struct SourceCandidate {
    execution: ClosedWorkspaceExecution,
    checkout_workspace: ValidatedWorkspace,
    candidate_ref: String,
    base_commit_sha: String,
    base_tree_sha: String,
    source_commit_sha: String,
    source_tree_sha: String,
    source_artifact_digest: String,
    custody_digest: String,
    files: Vec<FileChange>,
    candidate_change: CandidateChange,
    _candidate_parent_fd: OwnedFd,
    _candidate_checkout_fd: OwnedFd,
}

impl SourceCandidate {
    pub fn execution(&self) -> &ClosedWorkspaceExecution {
        &self.execution
    }

    pub fn project_id(&self) -> &ProjectId {
        self.execution.prepared.project_id()
    }

    pub fn root_id(&self) -> &RootId {
        self.execution.prepared.root_id()
    }

    pub fn task_id(&self) -> &TaskId {
        self.execution.prepared.task_id()
    }

    pub fn attempt_id(&self) -> &AttemptId {
        self.execution.prepared.attempt_id()
    }

    pub fn candidate_ref(&self) -> &str {
        &self.candidate_ref
    }

    pub fn base_commit_sha(&self) -> &str {
        &self.base_commit_sha
    }

    pub fn base_tree_sha(&self) -> &str {
        &self.base_tree_sha
    }

    pub fn source_commit_sha(&self) -> &str {
        &self.source_commit_sha
    }

    pub fn source_tree_sha(&self) -> &str {
        &self.source_tree_sha
    }

    pub fn source_artifact_digest(&self) -> &str {
        &self.source_artifact_digest
    }

    pub fn custody_digest(&self) -> &str {
        &self.custody_digest
    }

    pub fn closure_digest(&self) -> &str {
        self.execution.closure_digest()
    }

    pub fn workspace_digest(&self) -> &str {
        self.execution.workspace_digest()
    }

    pub fn checkout_path(&self) -> &Path {
        &self.checkout_workspace.workspace
    }

    pub fn checkout_workspace(&self) -> &ValidatedWorkspace {
        &self.checkout_workspace
    }

    pub fn files(&self) -> &[FileChange] {
        &self.files
    }

    pub fn candidate_change(&self) -> &CandidateChange {
        &self.candidate_change
    }

    /// Attach declared contract/interface constraints to this candidate's diff
    /// and validate through [`admiralty_core::conflicts::parse_candidates`].
    pub fn candidate_change_with_constraints(
        &self,
        constraints: CandidateConstraints,
    ) -> Result<CandidateChange, SourceCandidateError> {
        let mut change = self.candidate_change.clone();
        change.constraints = Some(constraints);
        validate_candidate_change(&change)?;
        Ok(change)
    }

    /// Construct [`VerificationInputs`] bound to this candidate's immutable
    /// source commit, tree, artifact digest, and candidate reference.
    pub fn verification_inputs(
        &self,
        contract_revision: u32,
        contract_digest: &str,
        verification_policy_digest: &str,
        protected_oracle_digest: &str,
    ) -> VerificationInputs {
        VerificationInputs {
            project_id: self.project_id().clone(),
            root_id: self.root_id().clone(),
            task_id: self.task_id().clone(),
            attempt_id: self.attempt_id().clone(),
            candidate_ref: self.candidate_ref.clone(),
            contract_revision,
            contract_digest: contract_digest.to_owned(),
            verification_policy_digest: verification_policy_digest.to_owned(),
            source_commit_sha: self.source_commit_sha.clone(),
            source_tree_sha: self.source_tree_sha.clone(),
            source_artifact_digest: self.source_artifact_digest.clone(),
            protected_oracle_digest: protected_oracle_digest.to_owned(),
        }
    }

    /// Bind an [`ExecutedChecks`] proof to this candidate's custody and closure
    /// digests for `LifecycleAction::DecideCandidate`.
    pub fn decision_evidence(
        &self,
        executed_checks: &ExecutedChecks,
        verifier: PrincipalId,
    ) -> Result<CandidateDecisionEvidence, SourceCandidateError> {
        let receipt = executed_checks.receipt();
        let inputs = &receipt.inputs;
        let receipt_digest = content_digest(receipt).map_err(|_| SourceCandidateError::Digest)?;
        if receipt.producer != ReceiptProducer::IndependentVerifier
            || receipt.verifier != verifier
            || inputs.project_id != *self.project_id()
            || inputs.root_id != *self.root_id()
            || inputs.task_id != *self.task_id()
            || inputs.attempt_id != *self.attempt_id()
            || inputs.candidate_ref != self.candidate_ref
            || inputs.source_commit_sha != self.source_commit_sha
            || inputs.source_tree_sha != self.source_tree_sha
            || inputs.source_artifact_digest != self.source_artifact_digest
            || !valid_sha256_digest(&inputs.contract_digest)
            || !valid_sha256_digest(&inputs.verification_policy_digest)
            || !valid_sha256_digest(executed_checks.execution_digest())
            || !valid_sha256_digest(&receipt_digest)
        {
            return Err(SourceCandidateError::VerificationBindingMismatch);
        }
        Ok(CandidateDecisionEvidence {
            contract_digest: inputs.contract_digest.clone(),
            verification_policy_digest: inputs.verification_policy_digest.clone(),
            receipt_digest,
            custody_digest: self.custody_digest.clone(),
            closure_digest: self.execution.closure_digest().to_owned(),
            source_commit_sha: self.source_commit_sha.clone(),
            source_tree_sha: self.source_tree_sha.clone(),
            verifier,
        })
    }

    /// Construct the `DomainEvent::CandidateSubmitted` event for this candidate.
    pub fn candidate_submitted_event(&self) -> DomainEvent {
        DomainEvent::CandidateSubmitted {
            task_id: self.task_id().clone(),
            attempt_id: self.attempt_id().clone(),
            candidate_ref: self.candidate_ref.clone(),
        }
    }
}

/// Inspect a closed execution workspace, freeze or validate changed Git source
/// relative to `prepared.head_oid()`, enforce path scope, and materialize an
/// immutable, clean, detached mode-`0700` checkout beneath `candidate_parent`.
pub fn materialize_source_candidate(
    execution: ClosedWorkspaceExecution,
    candidate_parent: &Path,
    destination_name: &str,
) -> Result<SourceCandidate, SourceCandidateError> {
    let git = trusted_git()?;
    let workspace_path = execution.prepared.workspace().workspace.clone();
    let workspace_parent = workspace_path
        .parent()
        .ok_or(SourceCandidateError::UnsafeWorkspaceMetadata)?;
    let workspace_parent_fd = open_directory_chain(workspace_parent)
        .map_err(|_| SourceCandidateError::UnsafeWorkspaceMetadata)?;
    ensure_private_fd(&workspace_parent_fd)
        .map_err(|_| SourceCandidateError::UnsafeWorkspaceMetadata)?;
    let workspace_fd = open_directory_chain(&workspace_path)
        .map_err(|_| SourceCandidateError::UnsafeWorkspaceMetadata)?;
    ensure_private_fd(&workspace_fd).map_err(|_| SourceCandidateError::UnsafeWorkspaceMetadata)?;
    ensure_path_points_to_fd(&workspace_path, &workspace_fd)
        .map_err(|_| SourceCandidateError::UnsafeWorkspaceMetadata)?;

    let workspace_cwd = PinnedGitDirectory::new(&workspace_path, &workspace_fd)?;
    validate_workspace_git_metadata(&git, &workspace_cwd, &workspace_fd)?;
    validate_working_tree_entries(&workspace_path)?;

    let base_commit_sha = execution.prepared.head_oid().to_owned();
    let base_tree_sha = execution.prepared.tree_oid().to_owned();
    if !valid_sha1_oid(&base_commit_sha) || !valid_sha1_oid(&base_tree_sha) {
        return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
    }
    let base_kind = git_text(
        &git,
        Some(&workspace_cwd),
        &["cat-file", "-t", &base_commit_sha],
    )?;
    if base_kind.trim() != "commit" {
        return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
    }

    let status_before = git_status_porcelain(&git, &workspace_cwd)?;
    let current_head = git_text(
        &git,
        Some(&workspace_cwd),
        &["rev-parse", "--verify", "HEAD"],
    )?
    .trim()
    .to_owned();
    if !valid_sha1_oid(&current_head) {
        return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
    }
    if current_head != base_commit_sha {
        verify_commit_ancestry(&git, &workspace_cwd, &base_commit_sha, &current_head)?;
    }

    if !status_before.trim().is_empty() {
        freeze_dirty_working_tree(&git, &workspace_cwd)?;
    } else if current_head == base_commit_sha {
        return Err(SourceCandidateError::NoSourceChanges);
    }

    validate_workspace_git_metadata(&git, &workspace_cwd, &workspace_fd)?;
    let status_after = git_status_porcelain(&git, &workspace_cwd)?;
    if !status_after.trim().is_empty() {
        return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
    }

    let source_commit_sha = git_text(
        &git,
        Some(&workspace_cwd),
        &["rev-parse", "--verify", "HEAD"],
    )?
    .trim()
    .to_owned();
    let source_tree_sha = git_text(
        &git,
        Some(&workspace_cwd),
        &["rev-parse", "--verify", "HEAD^{tree}"],
    )?
    .trim()
    .to_owned();
    if !valid_sha1_oid(&source_commit_sha)
        || !valid_sha1_oid(&source_tree_sha)
        || source_commit_sha == source_tree_sha
    {
        return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
    }
    if source_commit_sha == base_commit_sha || source_tree_sha == base_tree_sha {
        return Err(SourceCandidateError::NoSourceChanges);
    }
    verify_commit_ancestry(&git, &workspace_cwd, &base_commit_sha, &source_commit_sha)?;
    validate_commit_tree_entries(&git, &workspace_cwd, &source_tree_sha)?;

    let files = extract_and_validate_changed_files(
        &git,
        &workspace_cwd,
        execution.prepared.workspace(),
        &base_commit_sha,
        &source_commit_sha,
    )?;
    if files.is_empty() {
        return Err(SourceCandidateError::NoSourceChanges);
    }

    let (checkout_workspace, candidate_parent_fd, candidate_checkout_fd) =
        materialize_immutable_checkout(
            &git,
            &workspace_cwd,
            execution.prepared.workspace(),
            candidate_parent,
            destination_name,
            &source_commit_sha,
            &source_tree_sha,
        )?;

    let source_artifact_digest = content_digest(&(
        "source-candidate-artifact-v1",
        &base_commit_sha,
        &source_commit_sha,
        &source_tree_sha,
        &files,
    ))
    .map_err(|_| SourceCandidateError::Digest)?;

    let custody_digest = content_digest(&(
        "source-candidate-custody-v1",
        execution.prepared.project_id(),
        execution.prepared.root_id(),
        execution.prepared.task_id(),
        execution.prepared.attempt_id(),
        execution.workspace_digest(),
        execution.closure_digest(),
        &base_commit_sha,
        &source_commit_sha,
        &source_tree_sha,
        &source_artifact_digest,
    ))
    .map_err(|_| SourceCandidateError::Digest)?;

    let raw_candidate_digest = content_digest(&(
        "source-candidate-ref-v1",
        execution.prepared.task_id(),
        execution.prepared.attempt_id(),
        &source_commit_sha,
        &source_tree_sha,
        &custody_digest,
    ))
    .map_err(|_| SourceCandidateError::Digest)?;
    let candidate_hex = raw_candidate_digest
        .strip_prefix("sha256:")
        .ok_or(SourceCandidateError::Digest)?;
    let candidate_ref = format!("candidate-{candidate_hex}");

    let ancestry_digest = content_digest(&(
        "source-candidate-ancestry-v1",
        &base_commit_sha,
        &source_commit_sha,
    ))
    .map_err(|_| SourceCandidateError::Digest)?;

    let candidate_change = CandidateChange {
        project_id: execution.prepared.project_id().clone(),
        root_id: execution.prepared.root_id().clone(),
        task_id: execution.prepared.task_id().clone(),
        candidate_id: candidate_ref.clone(),
        base_sha: base_commit_sha.clone(),
        head_sha: source_commit_sha.clone(),
        tree_sha: source_tree_sha.clone(),
        ancestry_digest: Some(ancestry_digest),
        files: files.clone(),
        constraints: None,
    };
    validate_candidate_change(&candidate_change)?;

    Ok(SourceCandidate {
        execution,
        checkout_workspace,
        candidate_ref,
        base_commit_sha,
        base_tree_sha,
        source_commit_sha,
        source_tree_sha,
        source_artifact_digest,
        custody_digest,
        files,
        candidate_change,
        _candidate_parent_fd: candidate_parent_fd,
        _candidate_checkout_fd: candidate_checkout_fd,
    })
}

fn validate_candidate_change(change: &CandidateChange) -> Result<(), SourceCandidateError> {
    let encoded = serde_json::to_vec(&vec![change.clone()])
        .map_err(|error| SourceCandidateError::ConflictValidation(error.to_string()))?;
    parse_candidates(&encoded)
        .map(|_| ())
        .map_err(|error| SourceCandidateError::ConflictValidation(error.to_string()))
}

fn freeze_dirty_working_tree(
    git: &Path,
    workspace_cwd: &PinnedGitDirectory<'_>,
) -> Result<(), SourceCandidateError> {
    run_git(
        git,
        Some(workspace_cwd),
        &[
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.attributesFile=/dev/null",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.autocrlf=false",
            "add",
            "--all",
            "--",
            ".",
        ],
    )?;
    let staged = run_git_with_options(
        git,
        Some(workspace_cwd),
        &[
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.attributesFile=/dev/null",
            "-c",
            "core.fsmonitor=false",
            "ls-files",
            "--stage",
            "-z",
        ],
        None,
        GIT_TIMEOUT,
        MAX_TREE_LISTING,
    )?
    .stdout;
    for record in staged
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
    {
        let text = std::str::from_utf8(record)
            .map_err(|_| SourceCandidateError::UnsafeWorkspaceMetadata)?;
        let mode = text
            .split_ascii_whitespace()
            .next()
            .ok_or(SourceCandidateError::UnsafeWorkspaceMetadata)?;
        if !matches!(mode, "100644" | "100755") {
            return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
        }
    }
    run_git(
        git,
        Some(workspace_cwd),
        &[
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.attributesFile=/dev/null",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "commit.gpgSign=false",
            "-c",
            &format!("user.name={DETERMINISTIC_AUTHOR_NAME}"),
            "-c",
            &format!("user.email={DETERMINISTIC_AUTHOR_EMAIL}"),
            "commit",
            "--quiet",
            "--no-verify",
            "--no-gpg-sign",
            "-m",
            "admiralty source candidate",
        ],
    )?;
    make_git_dir_private(&workspace_cwd.access_path().join(".git"))?;
    Ok(())
}

fn verify_commit_ancestry(
    git: &Path,
    workspace_cwd: &PinnedGitDirectory<'_>,
    base_commit_sha: &str,
    head_commit_sha: &str,
) -> Result<(), SourceCandidateError> {
    run_git(
        git,
        Some(workspace_cwd),
        &[
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.attributesFile=/dev/null",
            "-c",
            "core.fsmonitor=false",
            "merge-base",
            "--is-ancestor",
            base_commit_sha,
            head_commit_sha,
        ],
    )
    .map(|_| ())
    .map_err(|_| SourceCandidateError::InvalidBaseAncestry)
}

fn validate_commit_tree_entries(
    git: &Path,
    workspace_cwd: &PinnedGitDirectory<'_>,
    tree_oid: &str,
) -> Result<(), SourceCandidateError> {
    let listing = run_git_with_options(
        git,
        Some(workspace_cwd),
        &["ls-tree", "-r", "-t", "-z", "--full-tree", tree_oid],
        None,
        GIT_TIMEOUT,
        MAX_TREE_LISTING,
    )?
    .stdout;
    let mut entries = 0usize;
    for record in listing
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
    {
        entries = entries
            .checked_add(1)
            .filter(|count| *count <= MAX_WORKING_TREE_ENTRIES)
            .ok_or(SourceCandidateError::UnsafeWorkspaceMetadata)?;
        let tab = record
            .iter()
            .position(|byte| *byte == b'\t')
            .ok_or(SourceCandidateError::UnsafeWorkspaceMetadata)?;
        let header = std::str::from_utf8(&record[..tab])
            .map_err(|_| SourceCandidateError::UnsafeWorkspaceMetadata)?;
        let path_text = std::str::from_utf8(&record[tab + 1..])
            .map_err(|_| SourceCandidateError::UnsafeWorkspaceMetadata)?;
        if !safe_relative_path(path_text) {
            return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
        }
        let mut fields = header.split_ascii_whitespace();
        let mode = fields
            .next()
            .ok_or(SourceCandidateError::UnsafeWorkspaceMetadata)?;
        let kind = fields
            .next()
            .ok_or(SourceCandidateError::UnsafeWorkspaceMetadata)?;
        let oid = fields
            .next()
            .ok_or(SourceCandidateError::UnsafeWorkspaceMetadata)?;
        if fields.next().is_some()
            || !valid_sha1_oid(oid)
            || !matches!(
                (mode, kind),
                ("100644" | "100755", "blob") | ("040000", "tree")
            )
        {
            return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
        }
    }
    Ok(())
}

fn extract_and_validate_changed_files(
    git: &Path,
    workspace_cwd: &PinnedGitDirectory<'_>,
    workspace: &ValidatedWorkspace,
    base_commit_sha: &str,
    source_commit_sha: &str,
) -> Result<Vec<FileChange>, SourceCandidateError> {
    let diff_names = run_git_with_options(
        git,
        Some(workspace_cwd),
        &[
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.attributesFile=/dev/null",
            "-c",
            "core.fsmonitor=false",
            "diff",
            "--name-only",
            "--no-renames",
            "-z",
            base_commit_sha,
            source_commit_sha,
        ],
        None,
        GIT_TIMEOUT,
        MAX_GIT_OUTPUT,
    )?
    .stdout;

    let mut unique_paths = BTreeSet::new();
    for raw_path in diff_names
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
    {
        let path_str = std::str::from_utf8(raw_path)
            .map_err(|_| SourceCandidateError::UnsafeWorkspaceMetadata)?;
        if !safe_relative_path(path_str) {
            return Err(SourceCandidateError::ScopeViolation {
                path: path_str.to_owned(),
            });
        }
        check_path_scope(workspace, path_str)?;
        unique_paths.insert(path_str.to_owned());
    }

    let mut files = Vec::with_capacity(unique_paths.len());
    for path in unique_paths {
        let hunks = extract_file_hunks(
            git,
            workspace_cwd,
            base_commit_sha,
            source_commit_sha,
            &path,
        )?;
        files.push(FileChange { path, hunks });
    }
    Ok(files)
}

fn check_path_scope(
    workspace: &ValidatedWorkspace,
    changed_path: &str,
) -> Result<(), SourceCandidateError> {
    let candidate = Path::new(changed_path);
    if workspace
        .forbidden_paths
        .iter()
        .any(|forbidden| paths_related_case_insensitive(forbidden, candidate))
    {
        return Err(SourceCandidateError::ScopeViolation {
            path: changed_path.to_owned(),
        });
    }
    if !workspace.owned_paths.is_empty()
        && !workspace
            .owned_paths
            .iter()
            .any(|owned| path_contains(owned, candidate))
    {
        return Err(SourceCandidateError::ScopeViolation {
            path: changed_path.to_owned(),
        });
    }
    Ok(())
}

fn path_contains(scope: &Path, path: &Path) -> bool {
    scope == Path::new(".") || path == scope || path.starts_with(scope)
}

fn paths_related_case_insensitive(left: &Path, right: &Path) -> bool {
    let left_lower = left.as_os_str().to_string_lossy().to_ascii_lowercase();
    let right_lower = right.as_os_str().to_string_lossy().to_ascii_lowercase();
    let left_path = Path::new(&left_lower);
    let right_path = Path::new(&right_lower);
    path_contains(left_path, right_path) || path_contains(right_path, left_path)
}

fn extract_file_hunks(
    git: &Path,
    workspace_cwd: &PinnedGitDirectory<'_>,
    base_commit_sha: &str,
    source_commit_sha: &str,
    path: &str,
) -> Result<Option<Vec<HunkRange>>, SourceCandidateError> {
    let diff = git_text(
        git,
        Some(workspace_cwd),
        &[
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.attributesFile=/dev/null",
            "-c",
            "core.fsmonitor=false",
            "diff",
            "--unified=0",
            "--no-color",
            "--no-ext-diff",
            "--no-renames",
            base_commit_sha,
            source_commit_sha,
            "--",
            path,
        ],
    )?;
    if diff.lines().any(|line| line.starts_with("Binary files ")) {
        return Ok(None);
    }
    let mut ranges = Vec::<HunkRange>::new();
    for line in diff.lines() {
        if !line.starts_with("@@ ") {
            continue;
        }
        let Some(range) = parse_unified_zero_hunk_header(line) else {
            return Ok(None);
        };
        if let Some(previous) = ranges.last_mut()
            && range.start <= previous.end
        {
            previous.end = previous.end.max(range.end);
        } else {
            ranges.push(range);
        }
    }
    if ranges.is_empty() {
        Ok(None)
    } else {
        Ok(Some(ranges))
    }
}

fn parse_unified_zero_hunk_header(line: &str) -> Option<HunkRange> {
    let body = line.strip_prefix("@@ -")?;
    let end_header = body.find(" @@")?;
    let header = &body[..end_header];
    let (old_part, new_part) = header.split_once(" +")?;
    let (old_start, old_count) = parse_hunk_span(old_part)?;
    let (new_start, new_count) = parse_hunk_span(new_part)?;
    let anchor = if new_count > 0 {
        (new_start, new_count)
    } else if old_count > 0 {
        (old_start, old_count)
    } else {
        let start = new_start.max(old_start).max(1);
        (start, 1)
    };
    let start = anchor.0.max(1);
    let end = start.checked_add(anchor.1.saturating_sub(1))?;
    Some(HunkRange { start, end })
}

fn parse_hunk_span(span: &str) -> Option<(u32, u32)> {
    if let Some((start, count)) = span.split_once(',') {
        Some((start.parse().ok()?, count.parse().ok()?))
    } else {
        Some((span.parse().ok()?, 1))
    }
}

fn materialize_immutable_checkout(
    git: &Path,
    source_cwd: &PinnedGitDirectory<'_>,
    source_workspace: &ValidatedWorkspace,
    candidate_parent: &Path,
    destination_name: &str,
    source_commit_sha: &str,
    source_tree_sha: &str,
) -> Result<(ValidatedWorkspace, OwnedFd, OwnedFd), SourceCandidateError> {
    validate_destination_name(destination_name)?;
    if !candidate_parent.is_absolute() {
        return Err(SourceCandidateError::InvalidCandidateDestination);
    }
    let parent_fd = open_directory_chain(candidate_parent)
        .map_err(|_| SourceCandidateError::InvalidCandidateDestination)?;
    ensure_private_fd(&parent_fd).map_err(|_| SourceCandidateError::InvalidCandidateDestination)?;
    let canonical_parent = fs::canonicalize(candidate_parent)
        .map_err(|_| SourceCandidateError::InvalidCandidateDestination)?;
    if canonical_parent != candidate_parent {
        return Err(SourceCandidateError::InvalidCandidateDestination);
    }
    ensure_path_points_to_fd(&canonical_parent, &parent_fd)
        .map_err(|_| SourceCandidateError::InvalidCandidateDestination)?;

    mkdirat(
        &parent_fd,
        destination_name,
        Mode::from_bits_truncate(0o700),
    )
    .map_err(|_| SourceCandidateError::InvalidCandidateDestination)?;
    let destination_fd = match openat(
        &parent_fd,
        destination_name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(fd) => fd,
        Err(_) => return Err(SourceCandidateError::InvalidCandidateDestination),
    };
    let destination_path = canonical_parent.join(destination_name);
    let setup = (|| -> Result<ValidatedWorkspace, SourceCandidateError> {
        rustix::fs::fchmod(&destination_fd, Mode::from_bits_truncate(0o700))
            .map_err(|_| SourceCandidateError::InvalidCandidateDestination)?;
        ensure_private_fd(&destination_fd)
            .map_err(|_| SourceCandidateError::InvalidCandidateDestination)?;
        ensure_same_directory(&parent_fd, destination_name, &destination_fd)
            .map_err(|_| SourceCandidateError::InvalidCandidateDestination)?;
        let destination_cwd = PinnedGitDirectory::new(&destination_path, &destination_fd)?;
        run_git(
            git,
            Some(&destination_cwd),
            &[
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "core.attributesFile=/dev/null",
                "-c",
                "core.fsmonitor=false",
                "init",
                "--quiet",
                "--template=",
            ],
        )?;
        make_git_dir_private(&destination_cwd.access_path().join(".git"))?;
        let source_path_text = source_cwd
            .access_path()
            .to_str()
            .ok_or(SourceCandidateError::UnsafeWorkspaceMetadata)?
            .to_owned();
        run_git_with_options(
            git,
            Some(&destination_cwd),
            &[
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "core.attributesFile=/dev/null",
                "-c",
                "core.fsmonitor=false",
                "-c",
                "protocol.file.allow=always",
                "fetch",
                "--quiet",
                "--depth=1",
                "--no-tags",
                "--no-recurse-submodules",
                &source_path_text,
                source_commit_sha,
            ],
            None,
            GIT_TRANSFER_TIMEOUT,
            MAX_GIT_OUTPUT,
        )?;
        run_git(
            git,
            Some(&destination_cwd),
            &[
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "core.attributesFile=/dev/null",
                "-c",
                "core.fsmonitor=false",
                "checkout",
                "--quiet",
                "--detach",
                "--no-recurse-submodules",
                source_commit_sha,
            ],
        )?;
        make_git_dir_private(&destination_cwd.access_path().join(".git"))?;
        fs::set_permissions(&destination_path, fs::Permissions::from_mode(0o700))
            .map_err(|_| SourceCandidateError::InvalidCandidateDestination)?;
        ensure_private_fd(&destination_fd)
            .map_err(|_| SourceCandidateError::InvalidCandidateDestination)?;
        validate_workspace_git_metadata(git, &destination_cwd, &destination_fd)?;

        let head = git_text(
            git,
            Some(&destination_cwd),
            &["rev-parse", "--verify", "HEAD"],
        )?
        .trim()
        .to_owned();
        let tree = git_text(
            git,
            Some(&destination_cwd),
            &["rev-parse", "--verify", "HEAD^{tree}"],
        )?
        .trim()
        .to_owned();
        if head != source_commit_sha || tree != source_tree_sha {
            return Err(SourceCandidateError::InvalidCandidateDestination);
        }
        let branch_status = git_text(
            git,
            Some(&destination_cwd),
            &[
                "--no-pager",
                "-c",
                "core.fsmonitor=false",
                "-c",
                "core.untrackedCache=false",
                "-c",
                "core.excludesFile=/dev/null",
                "-c",
                "core.hooksPath=/dev/null",
                "status",
                "--porcelain=v2",
                "--branch",
                "--untracked-files=all",
            ],
        )?;
        if branch_status
            .lines()
            .any(|line| !line.is_empty() && !line.starts_with("# "))
            || !branch_status
                .lines()
                .any(|line| line.strip_prefix("# branch.oid ") == Some(source_commit_sha))
        {
            return Err(SourceCandidateError::InvalidCandidateDestination);
        }

        let validated = WorkspaceSpec {
            node_id: source_workspace.node_id.clone(),
            root_id: source_workspace.root_id.clone(),
            project_id: source_workspace.project_id.clone(),
            workspace: destination_path.clone(),
            owned_paths: source_workspace.owned_paths.clone(),
            forbidden_paths: source_workspace.forbidden_paths.clone(),
        }
        .validate()
        .map_err(|_| SourceCandidateError::InvalidCandidateDestination)?;
        if validated.custody != Custody::RegisteredOnly
            || validated.git_metadata != GitMetadata::Directory
        {
            return Err(SourceCandidateError::InvalidCandidateDestination);
        }
        ensure_path_points_to_fd(&canonical_parent, &parent_fd)
            .map_err(|_| SourceCandidateError::InvalidCandidateDestination)?;
        ensure_same_directory(&parent_fd, destination_name, &destination_fd)
            .map_err(|_| SourceCandidateError::InvalidCandidateDestination)?;
        Ok(validated)
    })();

    match setup {
        Ok(validated) => Ok((validated, parent_fd, destination_fd)),
        Err(error) => {
            if ensure_same_directory(&parent_fd, destination_name, &destination_fd).is_ok() {
                let _ = fs::remove_dir_all(&destination_path);
            }
            Err(error)
        }
    }
}

fn validate_destination_name(name: &str) -> Result<(), SourceCandidateError> {
    if name.is_empty()
        || name.len() > MAX_DESTINATION_NAME_BYTES
        || name == "."
        || name == ".."
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
    {
        return Err(SourceCandidateError::InvalidCandidateDestination);
    }
    Ok(())
}

fn validate_working_tree_entries(workspace_root: &Path) -> Result<(), SourceCandidateError> {
    let uid = rustix::process::geteuid().as_raw();
    let mut stack = vec![workspace_root.to_path_buf()];
    let mut entries = 0usize;
    let mut total_bytes = 0u64;
    while let Some(directory) = stack.pop() {
        let dir_meta = fs::symlink_metadata(&directory)
            .map_err(|_| SourceCandidateError::UnsafeWorkspaceMetadata)?;
        if dir_meta.file_type().is_symlink() || !dir_meta.is_dir() || dir_meta.uid() != uid {
            return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
        }
        for entry in
            fs::read_dir(&directory).map_err(|_| SourceCandidateError::UnsafeWorkspaceMetadata)?
        {
            let entry = entry.map_err(|_| SourceCandidateError::UnsafeWorkspaceMetadata)?;
            let name = entry.file_name();
            let name_str = name
                .to_str()
                .ok_or(SourceCandidateError::UnsafeWorkspaceMetadata)?;
            let path = entry.path();
            if directory == workspace_root && name_str == ".git" {
                continue;
            }
            if name_str == ".git" || name_str == ".gitmodules" {
                return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
            }
            entries = entries
                .checked_add(1)
                .filter(|count| *count <= MAX_WORKING_TREE_ENTRIES)
                .ok_or(SourceCandidateError::UnsafeWorkspaceMetadata)?;
            let meta = fs::symlink_metadata(&path)
                .map_err(|_| SourceCandidateError::UnsafeWorkspaceMetadata)?;
            let file_type = meta.file_type();
            if file_type.is_symlink()
                || file_type.is_fifo()
                || file_type.is_socket()
                || file_type.is_block_device()
                || file_type.is_char_device()
                || meta.uid() != uid
            {
                return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
            }
            if meta.is_dir() {
                if meta.mode() & 0o002 != 0 {
                    return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
                }
                stack.push(path);
            } else if meta.is_file() {
                if meta.nlink() != 1 || meta.mode() & 0o002 != 0 {
                    return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
                }
                total_bytes = total_bytes
                    .checked_add(meta.len())
                    .filter(|bytes| *bytes <= MAX_WORKING_TREE_BYTES)
                    .ok_or(SourceCandidateError::UnsafeWorkspaceMetadata)?;
            } else {
                return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
            }
        }
    }
    Ok(())
}

fn validate_workspace_git_metadata(
    git: &Path,
    workspace_cwd: &PinnedGitDirectory<'_>,
    workspace_fd: &OwnedFd,
) -> Result<(), SourceCandidateError> {
    let git_dir = workspace_cwd.access_path().join(".git");
    let uid = rustix::process::geteuid().as_raw();
    let meta = fs::symlink_metadata(&git_dir)
        .map_err(|_| SourceCandidateError::UnsafeWorkspaceMetadata)?;
    if meta.file_type().is_symlink()
        || !meta.is_dir()
        || meta.uid() != uid
        || meta.mode() & 0o022 != 0
    {
        return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
    }
    read_safe_config(&git_dir.join("config"))?;
    for forbidden in [
        Path::new("objects/info/alternates"),
        Path::new("objects/info/http-alternates"),
    ] {
        if path_entry_exists(&git_dir.join(forbidden))? {
            return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
        }
    }
    let hooks_dir = git_dir.join("hooks");
    if path_entry_exists(&hooks_dir)? {
        let hooks_meta = fs::symlink_metadata(&hooks_dir)
            .map_err(|_| SourceCandidateError::UnsafeWorkspaceMetadata)?;
        if hooks_meta.file_type().is_symlink() || !hooks_meta.is_dir() || hooks_meta.uid() != uid {
            return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
        }
        for hook in
            fs::read_dir(&hooks_dir).map_err(|_| SourceCandidateError::UnsafeWorkspaceMetadata)?
        {
            let hook = hook.map_err(|_| SourceCandidateError::UnsafeWorkspaceMetadata)?;
            let hook_meta = fs::symlink_metadata(hook.path())
                .map_err(|_| SourceCandidateError::UnsafeWorkspaceMetadata)?;
            let hook_name = hook.file_name();
            let hook_name_str = hook_name.to_string_lossy();
            if hook_meta.file_type().is_symlink()
                || hook_meta.uid() != uid
                || (!hook_name_str.ends_with(".sample") && hook_meta.mode() & 0o111 != 0)
            {
                return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
            }
        }
    }
    let format = git_text(
        git,
        Some(workspace_cwd),
        &["rev-parse", "--show-object-format"],
    )?;
    if format.trim() != "sha1" {
        return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
    }
    let top = git_text(git, Some(workspace_cwd), &["rev-parse", "--show-toplevel"])?;
    if !same_directory_as_fd(Path::new(top.trim()), workspace_fd) {
        return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
    }
    Ok(())
}

fn read_safe_config(path: &Path) -> Result<Vec<u8>, SourceCandidateError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|_| SourceCandidateError::UnsafeWorkspaceMetadata)?;
    let uid = rustix::process::geteuid().as_raw();
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.uid() != uid
        || metadata.mode() & 0o022 != 0
    {
        return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
    }
    let bytes = fs::read(path).map_err(|_| SourceCandidateError::UnsafeWorkspaceMetadata)?;
    let lower = String::from_utf8_lossy(&bytes).to_ascii_lowercase();
    if lower.contains("[include")
        || lower.contains("[filter")
        || lower.contains("fsmonitor")
        || lower.contains("hookspath")
        || lower.contains("promisor")
        || lower.contains("partialclone")
        || lower.contains("protocol.")
        || lower.contains("insteadof")
        || lower.contains("credential.helper")
        || lower.contains("sshcommand")
        || lower.contains("ext::")
    {
        return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
    }
    Ok(bytes)
}

fn make_git_dir_private(path: &Path) -> Result<(), SourceCandidateError> {
    let uid = rustix::process::geteuid().as_raw();
    let metadata =
        fs::symlink_metadata(path).map_err(|_| SourceCandidateError::UnsafeWorkspaceMetadata)?;
    if metadata.file_type().is_symlink() || metadata.uid() != uid {
        return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
    }
    if metadata.is_dir() {
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|_| SourceCandidateError::UnsafeWorkspaceMetadata)?;
        for entry in
            fs::read_dir(path).map_err(|_| SourceCandidateError::UnsafeWorkspaceMetadata)?
        {
            let entry = entry.map_err(|_| SourceCandidateError::UnsafeWorkspaceMetadata)?;
            make_git_dir_private(&entry.path())?;
        }
        Ok(())
    } else if metadata.is_file() {
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .map_err(|_| SourceCandidateError::UnsafeWorkspaceMetadata)
    } else {
        Err(SourceCandidateError::UnsafeWorkspaceMetadata)
    }
}

fn git_status_porcelain(
    git: &Path,
    workspace_cwd: &PinnedGitDirectory<'_>,
) -> Result<String, SourceCandidateError> {
    git_text(
        git,
        Some(workspace_cwd),
        &[
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.attributesFile=/dev/null",
            "-c",
            "core.fsmonitor=false",
            "status",
            "--porcelain=v2",
            "--untracked-files=all",
            "--ignored=matching",
            "--ignore-submodules=none",
        ],
    )
}

fn path_entry_exists(path: &Path) -> Result<bool, SourceCandidateError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(SourceCandidateError::UnsafeWorkspaceMetadata),
    }
}

struct PinnedGitDirectory<'a> {
    named: &'a Path,
    directory: &'a OwnedFd,
}

impl<'a> PinnedGitDirectory<'a> {
    fn new(named: &'a Path, directory: &'a OwnedFd) -> Result<Self, SourceCandidateError> {
        let pinned = Self { named, directory };
        pinned.check()?;
        Ok(pinned)
    }

    fn access_path(&self) -> &Path {
        self.named
    }

    fn check(&self) -> Result<(), SourceCandidateError> {
        ensure_path_points_to_fd(self.named, self.directory)
            .map_err(|_| SourceCandidateError::UnsafeWorkspaceMetadata)
    }
}

fn open_directory_chain(path: &Path) -> Result<OwnedFd, SourceCandidateError> {
    if !path.is_absolute() {
        return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
    }
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let mut current = open("/", flags, Mode::empty())
        .map_err(|_| SourceCandidateError::UnsafeWorkspaceMetadata)?;
    for component in path.components() {
        ensure_safe_ancestor_fd(&current)?;
        match component {
            Component::RootDir => {}
            Component::Normal(name) => {
                current = openat(&current, name, flags, Mode::empty())
                    .map_err(|_| SourceCandidateError::UnsafeWorkspaceMetadata)?;
            }
            _ => return Err(SourceCandidateError::UnsafeWorkspaceMetadata),
        }
    }
    ensure_safe_ancestor_fd(&current)?;
    Ok(current)
}

fn ensure_safe_ancestor_fd(fd: &OwnedFd) -> Result<(), SourceCandidateError> {
    let stat = fstat(fd).map_err(|_| SourceCandidateError::UnsafeWorkspaceMetadata)?;
    let uid = rustix::process::geteuid().as_raw();
    let mode = stat.st_mode & 0o7777;
    if (stat.st_uid != 0 && stat.st_uid != uid) || (mode & 0o022 != 0 && mode & 0o1000 == 0) {
        return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
    }
    Ok(())
}

fn ensure_private_fd(fd: &OwnedFd) -> Result<(), SourceCandidateError> {
    let stat = fstat(fd).map_err(|_| SourceCandidateError::UnsafeWorkspaceMetadata)?;
    let uid = rustix::process::geteuid().as_raw();
    if stat.st_uid != uid || stat.st_mode & 0o077 != 0 {
        return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
    }
    Ok(())
}

fn ensure_path_points_to_fd(path: &Path, fd: &OwnedFd) -> Result<(), SourceCandidateError> {
    let opened = open_directory_chain(path)?;
    let left = fstat(&opened).map_err(|_| SourceCandidateError::UnsafeWorkspaceMetadata)?;
    let right = fstat(fd).map_err(|_| SourceCandidateError::UnsafeWorkspaceMetadata)?;
    if left.st_dev != right.st_dev || left.st_ino != right.st_ino {
        return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
    }
    Ok(())
}

fn ensure_same_directory(
    parent_fd: &OwnedFd,
    name: &str,
    child_fd: &OwnedFd,
) -> Result<(), SourceCandidateError> {
    let child_stat =
        fstat(child_fd).map_err(|_| SourceCandidateError::InvalidCandidateDestination)?;
    let named_stat = statat(parent_fd, name, AtFlags::SYMLINK_NOFOLLOW)
        .map_err(|_| SourceCandidateError::InvalidCandidateDestination)?;
    if child_stat.st_dev != named_stat.st_dev || child_stat.st_ino != named_stat.st_ino {
        return Err(SourceCandidateError::InvalidCandidateDestination);
    }
    Ok(())
}

fn same_directory_as_fd(path: &Path, directory: &OwnedFd) -> bool {
    let Ok(path_metadata) = fs::metadata(path) else {
        return false;
    };
    let Ok(stat) = fstat(directory) else {
        return false;
    };
    path_metadata.dev() == stat.st_dev && path_metadata.ino() == stat.st_ino
}

fn trusted_git() -> Result<PathBuf, SourceCandidateError> {
    let uid = rustix::process::geteuid().as_raw();
    for candidate in TRUSTED_GIT {
        let path = Path::new(candidate);
        let Ok(meta) = fs::symlink_metadata(path) else {
            continue;
        };
        if meta.file_type().is_symlink()
            || !meta.is_file()
            || (meta.uid() != 0 && meta.uid() != uid)
            || meta.mode() & 0o022 != 0
            || meta.mode() & 0o111 == 0
        {
            continue;
        }
        let Ok(canonical) = fs::canonicalize(path) else {
            continue;
        };
        if canonical == path {
            return Ok(canonical);
        }
    }
    Err(SourceCandidateError::GitUnavailable)
}

fn base_git_command(git: &Path, cwd: Option<&Path>) -> Command {
    let mut command = Command::new(git);
    command.env_clear();
    command
        .env("HOME", "/nonexistent")
        .env("PATH", "/usr/bin:/bin")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .env("GIT_PROTOCOL_FROM_USER", "0")
        .env("GIT_ALLOW_PROTOCOL", "file")
        .env("GIT_AUTHOR_NAME", DETERMINISTIC_AUTHOR_NAME)
        .env("GIT_AUTHOR_EMAIL", DETERMINISTIC_AUTHOR_EMAIL)
        .env("GIT_AUTHOR_DATE", DETERMINISTIC_COMMIT_DATE)
        .env("GIT_COMMITTER_NAME", DETERMINISTIC_AUTHOR_NAME)
        .env("GIT_COMMITTER_EMAIL", DETERMINISTIC_AUTHOR_EMAIL)
        .env("GIT_COMMITTER_DATE", DETERMINISTIC_COMMIT_DATE);
    if let Some(directory) = cwd {
        command.current_dir(directory);
    }
    command
}

fn git_text(
    git: &Path,
    cwd: Option<&PinnedGitDirectory<'_>>,
    args: &[&str],
) -> Result<String, SourceCandidateError> {
    let output = run_git(git, cwd, args)?;
    String::from_utf8(output.stdout)
        .map_err(|_| SourceCandidateError::GitCommand("non-UTF-8 git output".into()))
}

fn run_git(
    git: &Path,
    cwd: Option<&PinnedGitDirectory<'_>>,
    args: &[&str],
) -> Result<Output, SourceCandidateError> {
    run_git_with_options(git, cwd, args, None, GIT_TIMEOUT, MAX_GIT_OUTPUT)
}

fn run_git_with_options(
    git: &Path,
    cwd: Option<&PinnedGitDirectory<'_>>,
    args: &[&str],
    input: Option<&[u8]>,
    timeout: Duration,
    output_limit: usize,
) -> Result<Output, SourceCandidateError> {
    if let Some(directory) = cwd {
        directory.check()?;
    }
    let command_cwd = cwd.map(PinnedGitDirectory::access_path);
    let mut command = base_git_command(git, command_cwd);
    command
        .args(args)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let mut child = command
        .spawn()
        .map_err(|error| SourceCandidateError::GitCommand(error.to_string()))?;
    let mut stdout = match child.stdout.take() {
        Some(pipe) => Some(pipe),
        None => {
            return abort_git_child(
                &mut child,
                SourceCandidateError::GitCommand("missing stdout pipe".into()),
            );
        }
    };
    let mut stderr = match child.stderr.take() {
        Some(pipe) => Some(pipe),
        None => {
            return abort_git_child(
                &mut child,
                SourceCandidateError::GitCommand("missing stderr pipe".into()),
            );
        }
    };
    if let Some(pipe) = stdout.as_ref()
        && set_nonblocking(pipe).is_err()
    {
        return abort_git_child(
            &mut child,
            SourceCandidateError::GitCommand("failed to configure nonblocking stdout".into()),
        );
    }
    if let Some(pipe) = stderr.as_ref()
        && set_nonblocking(pipe).is_err()
    {
        return abort_git_child(
            &mut child,
            SourceCandidateError::GitCommand("failed to configure nonblocking stderr".into()),
        );
    }
    let mut stdout_bytes = Vec::with_capacity(output_limit.min(8192));
    let mut stderr_bytes = Vec::with_capacity(MAX_GIT_STDERR.min(8192));
    let deadline = Instant::now() + timeout;
    let mut status = None;
    loop {
        let stdout_overflow = match drain_pipe(&mut stdout, &mut stdout_bytes, output_limit) {
            Ok(overflow) => overflow,
            Err(error) => {
                return abort_git_child(
                    &mut child,
                    SourceCandidateError::GitCommand(error.to_string()),
                );
            }
        };
        let stderr_overflow = match drain_pipe(&mut stderr, &mut stderr_bytes, MAX_GIT_STDERR) {
            Ok(overflow) => overflow,
            Err(error) => {
                return abort_git_child(
                    &mut child,
                    SourceCandidateError::GitCommand(error.to_string()),
                );
            }
        };
        if status.is_none() {
            status = match child.try_wait() {
                Ok(status) => status,
                Err(error) => {
                    return abort_git_child(
                        &mut child,
                        SourceCandidateError::GitCommand(error.to_string()),
                    );
                }
            };
        }
        if stdout_overflow || stderr_overflow {
            if status.is_none() {
                terminate_git_group(&mut child);
                let _ = reap_child_bounded(&mut child);
            }
            return Err(SourceCandidateError::GitOutputLimit);
        }
        if status.is_some() && stdout.is_none() && stderr.is_none() {
            break;
        }
        if Instant::now() >= deadline {
            if status.is_none() {
                terminate_git_group(&mut child);
                let _ = reap_child_bounded(&mut child);
            }
            return Err(SourceCandidateError::GitTimeout);
        }
        thread::sleep(Duration::from_millis(10));
    }
    let exit_status =
        status.ok_or_else(|| SourceCandidateError::GitCommand("missing exit status".into()))?;
    if let Some(directory) = cwd {
        directory.check()?;
    }
    if exit_status.success() {
        Ok(Output {
            status: exit_status,
            stdout: stdout_bytes,
            stderr: stderr_bytes,
        })
    } else {
        let detail = String::from_utf8_lossy(&stderr_bytes);
        let operation = args.join(" ");
        Err(SourceCandidateError::GitCommand(format!(
            "{}: {}",
            operation.chars().take(128).collect::<String>(),
            detail.chars().take(320).collect::<String>()
        )))
    }
}

fn set_nonblocking(pipe: &impl AsFd) -> io::Result<()> {
    let flags = rustix::fs::fcntl_getfl(pipe).map_err(io::Error::from)?;
    rustix::fs::fcntl_setfl(pipe, flags | rustix::fs::OFlags::NONBLOCK).map_err(io::Error::from)
}

fn drain_pipe<R: Read>(
    pipe: &mut Option<R>,
    retained: &mut Vec<u8>,
    limit: usize,
) -> io::Result<bool> {
    let Some(reader) = pipe.as_mut() else {
        return Ok(false);
    };
    let mut buffer = [0u8; 8192];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => {
                pipe.take();
                return Ok(false);
            }
            Ok(count) => {
                let available = limit.saturating_sub(retained.len());
                let keep = count.min(available);
                retained.extend_from_slice(&buffer[..keep]);
                if keep < count {
                    return Ok(true);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(false),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
}

fn abort_git_child<T>(
    child: &mut Child,
    error: SourceCandidateError,
) -> Result<T, SourceCandidateError> {
    terminate_git_group(child);
    let _ = reap_child_bounded(child);
    Err(error)
}

fn terminate_git_group(child: &mut Child) {
    use rustix::process::{Pid, Signal, kill_process_group};
    if let Ok(raw_pid) = i32::try_from(child.id())
        && let Some(pid) = Pid::from_raw(raw_pid)
        && raw_pid > 1
    {
        let _ = kill_process_group(pid, Signal::KILL);
    }
    let _ = child.kill();
}

fn reap_child_bounded(child: &mut Child) -> Result<(), SourceCandidateError> {
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        if child
            .try_wait()
            .map_err(|error| SourceCandidateError::GitCommand(error.to_string()))?
            .is_some()
        {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(SourceCandidateError::GitTimeout);
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn valid_sha1_oid(value: &str) -> bool {
    value.len() == 40
        && !value.bytes().all(|byte| byte == b'0')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_sha256_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::{
        resource_gate::AdmissionIdentity,
        session::SessionHost,
        trusted_checks::{TrustedCheckError, execute_checks},
        workspace::{
            preparation::{WorkspacePreparation, prepare_workspace},
            registry::NodeRef,
        },
    };
    use admiralty_core::{
        AcceptanceCriterion, CheckSpec, LifecycleAction, LifecycleCommand, Projection,
        SchedulingState, TaskContract,
        nodes::NodeId,
        reduce,
        verification::{RequiredNegativeCheck, VerificationPolicy},
    };
    use std::sync::atomic::{AtomicU64, Ordering};

    static FIXTURE_SEQ: AtomicU64 = AtomicU64::new(1);

    #[derive(Clone, Debug)]
    struct FixtureClosedAdmission {
        identity: RecoveryIdentity,
        observation: EnforcementObservation,
        exit_code: Option<i32>,
        success: bool,
        closure_digest: String,
    }

    impl sealed::Sealed for FixtureClosedAdmission {}

    impl ClosedAdmissionProof for FixtureClosedAdmission {
        fn identity(&self) -> &RecoveryIdentity {
            &self.identity
        }

        fn observation(&self) -> &EnforcementObservation {
            &self.observation
        }

        fn exit_code(&self) -> Option<i32> {
            self.exit_code
        }

        fn success(&self) -> bool {
            self.success
        }

        fn closure_digest(&self) -> &str {
            &self.closure_digest
        }

        fn into_closed_admission(self) -> Option<ClosedAdmission> {
            None
        }
    }

    struct WorkspaceFixture {
        root: PathBuf,
        workspace_path: PathBuf,
        candidate_parent: PathBuf,
        spool_root: PathBuf,
        prepared: Option<PreparedWorkspace>,
        closure: FixtureClosedAdmission,
        contract: TaskContract,
    }

    impl WorkspaceFixture {
        fn take_prepared(&mut self) -> PreparedWorkspace {
            self.prepared.take().expect("prepared workspace handle")
        }
    }

    impl Drop for WorkspaceFixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn setup_fixture(owned_paths: Vec<&str>, forbidden_paths: Vec<&str>) -> WorkspaceFixture {
        let base_tmp = fs::canonicalize(std::env::temp_dir()).expect("canonical temp dir");
        let seq = FIXTURE_SEQ.fetch_add(1, Ordering::Relaxed);
        let root = base_tmp.join(format!("adm-source-candidate-{}-{seq}", std::process::id()));
        fs::create_dir(&root).expect("create root");
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).expect("chmod 0700 root");

        let source_repo = root.join("origin-repo");
        let managed_parent = root.join("managed-workspaces");
        let candidate_parent = root.join("candidate-checkouts");
        let spool_root = root.join("trusted-check-spool");
        for dir in [
            &source_repo,
            &managed_parent,
            &candidate_parent,
            &spool_root,
        ] {
            fs::create_dir(dir).expect("create subdir");
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).expect("chmod 0700 subdir");
        }

        fs::create_dir_all(source_repo.join("src/secret")).expect("create src/secret");
        fs::create_dir_all(source_repo.join("docs")).expect("create docs");
        fs::write(
            source_repo.join("src/lib.rs"),
            b"pub fn answer() -> u32 {\n    41\n}\n",
        )
        .expect("seed src/lib.rs");
        fs::write(source_repo.join("src/secret/key.txt"), b"top-secret\n")
            .expect("seed src/secret/key.txt");
        fs::write(source_repo.join("docs/readme.md"), b"# Readme\n").expect("seed docs/readme.md");

        run_fixture_git(&source_repo, &["init", "--quiet", "--template="]);
        run_fixture_git(&source_repo, &["config", "user.name", "Fixture Author"]);
        run_fixture_git(
            &source_repo,
            &["config", "user.email", "fixture@admiralty.invalid"],
        );
        run_fixture_git(&source_repo, &["add", "--all"]);
        run_fixture_git(&source_repo, &["commit", "--quiet", "-m", "initial"]);
        let pinned_commit = run_fixture_git(&source_repo, &["rev-parse", "HEAD"]);
        make_git_dir_private(&source_repo.join(".git")).expect("private origin .git");

        let project_id = ProjectId::new("adm-project").expect("project_id");
        let root_id = RootId::new("adm-root").expect("root_id");
        let task_id = TaskId::new("TASK-SOURCE-1").expect("task_id");
        let attempt_id = AttemptId::new("attempt-1").expect("attempt_id");

        let prepared = prepare_workspace(WorkspacePreparation {
            source: source_repo,
            managed_parent,
            destination_name: "attempt-1-work".into(),
            pinned_commit,
            node_id: NodeRef::new("node-alpha").expect("node_id"),
            root_id: root_id.clone(),
            project_id: project_id.clone(),
            task_id: task_id.clone(),
            attempt_id: attempt_id.clone(),
            owned_paths: owned_paths.iter().map(PathBuf::from).collect(),
            forbidden_paths: forbidden_paths.iter().map(PathBuf::from).collect(),
        })
        .expect("prepare_workspace");
        let workspace_path = prepared.workspace().workspace.clone();

        let observation = EnforcementObservation {
            unit: "adm-unit-1.service".into(),
            invocation_id: "inv-1234".into(),
            main_pid: 4242,
            process_start: "987654".into(),
            process_ids: vec![4242],
            cpu_set: BTreeSet::from([0]),
            memory_max_bytes: 64 * 1024 * 1024,
            tasks_max: 16,
            actual_executable: PathBuf::from("/usr/bin/true"),
            actual_cwd: workspace_path.clone(),
            actual_argv_digest: format!("sha256:{}", "1".repeat(64)),
            actual_environment_digest: format!("sha256:{}", "2".repeat(64)),
            cgroup_path: PathBuf::from("/sys/fs/cgroup/admiralty.slice/adm-unit-1.service"),
        };
        let identity = RecoveryIdentity {
            identity: AdmissionIdentity {
                project_id: project_id.clone(),
                task_id: task_id.clone(),
                attempt_id: attempt_id.clone(),
                node_id: NodeId::new("node-alpha").expect("node"),
                lease_id: "lease-1".into(),
            },
            launch_spec_digest: format!("sha256:{}", "3".repeat(64)),
            worker_environment_digest: format!("sha256:{}", "4".repeat(64)),
            unit: observation.unit.clone(),
            run_id: "run-1".into(),
            permit_id: "permit-1".into(),
            reservation_ref: "reservation-1".into(),
            host_incarnation: "boot-1".into(),
            backend_config_digest: format!("sha256:{}", "5".repeat(64)),
            invocation_id: Some(observation.invocation_id.clone()),
            main_pid: Some(observation.main_pid),
            process_start: Some(observation.process_start.clone()),
        };
        let closure_digest = content_digest(&(
            "closed-admission-v1",
            &identity,
            &observation,
            Some(0i32),
            true,
        ))
        .expect("closure digest");
        let closure = FixtureClosedAdmission {
            identity,
            observation,
            exit_code: Some(0),
            success: true,
            closure_digest,
        };

        let contract = TaskContract {
            schema_version: 1,
            project_id,
            root_id,
            task_id,
            revision: 1,
            title: "Materialize verified source candidate".into(),
            objective: "Bind changed Git source to trusted checks and lifecycle decision".into(),
            non_goals: vec![],
            owned_paths: if owned_paths.is_empty() {
                vec!["src".into()]
            } else {
                owned_paths.into_iter().map(str::to_owned).collect()
            },
            forbidden_paths: forbidden_paths.into_iter().map(str::to_owned).collect(),
            read_first: vec![],
            dependencies: vec![],
            criteria: vec![AcceptanceCriterion {
                id: "c-source".into(),
                text: "Updated source file exists in candidate checkout".into(),
                check_ids: vec!["verify-updated-source".into()],
            }],
            checks: vec![CheckSpec {
                id: "verify-updated-source".into(),
                program: "grep".into(),
                args: vec!["-q".into(), "42".into(), "src/lib.rs".into()],
                cwd: ".".into(),
                timeout_secs: 5,
            }],
            authority_ref: "grant:source-candidate-test".into(),
            max_attempts: 2,
            simulated: false,
        };

        WorkspaceFixture {
            root,
            workspace_path,
            candidate_parent,
            spool_root,
            prepared: Some(prepared),
            closure,
            contract,
        }
    }

    fn test_policy(verifier: PrincipalId) -> VerificationPolicy {
        VerificationPolicy {
            schema_version: 1,
            verifier,
            protected_oracle_digest: format!("sha256:{}", "a".repeat(64)),
            required_negative_checks: vec![RequiredNegativeCheck {
                check: CheckSpec {
                    id: "neg-reject-41".into(),
                    program: "grep".into(),
                    args: vec!["-q".into(), "41".into(), "src/lib.rs".into()],
                    cwd: ".".into(),
                    timeout_secs: 5,
                },
                expected_exit_code: 1,
                fixture_digest: format!("sha256:{}", "b".repeat(64)),
            }],
        }
    }

    fn run_fixture_git(repo: &Path, args: &[&str]) -> String {
        let output = Command::new("/usr/bin/git")
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", "/nonexistent")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_AUTHOR_NAME", "Fixture Provider")
            .env("GIT_AUTHOR_EMAIL", "provider@admiralty.invalid")
            .env("GIT_COMMITTER_NAME", "Fixture Provider")
            .env("GIT_COMMITTER_EMAIL", "provider@admiralty.invalid")
            .args([
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "core.attributesFile=/dev/null",
                "-c",
                "core.fsmonitor=false",
            ])
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .expect("run fixture git");
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    #[test]
    fn dirty_working_tree_freezes_materializes_runs_trusted_checks_and_reduces_decision() {
        let mut fixture = setup_fixture(vec!["src"], vec!["src/secret"]);
        let workspace_path = fixture.workspace_path.clone();
        fs::write(
            workspace_path.join("src/lib.rs"),
            b"pub fn answer() -> u32 {\n    42\n}\n",
        )
        .expect("modify owned file in working tree");
        fs::write(
            workspace_path.join("src/extra.rs"),
            b"pub const EXTRA: bool = true;\n",
        )
        .expect("create new owned file in working tree");

        let closed = prove_closed_workspace(fixture.take_prepared(), fixture.closure.clone())
            .expect("prove closed");
        let candidate =
            materialize_source_candidate(closed, &fixture.candidate_parent, "candidate-dirty")
                .expect("materialize dirty working tree candidate");

        assert_ne!(candidate.source_commit_sha(), candidate.base_commit_sha());
        assert_ne!(candidate.source_tree_sha(), candidate.base_tree_sha());
        assert!(candidate.candidate_ref().starts_with("candidate-"));
        assert_eq!(
            candidate
                .files()
                .iter()
                .map(|f| f.path.as_str())
                .collect::<Vec<_>>(),
            vec!["src/extra.rs", "src/lib.rs"]
        );

        let checkout_meta =
            fs::symlink_metadata(candidate.checkout_path()).expect("checkout metadata");
        assert_eq!(checkout_meta.mode() & 0o077, 0);

        let contract_digest = content_digest(&fixture.contract).expect("contract digest");
        let verifier = PrincipalId::new("independent-verifier").expect("verifier");
        let policy = test_policy(verifier.clone());
        let policy_digest = content_digest(&policy).expect("policy digest");
        let inputs = candidate.verification_inputs(
            fixture.contract.revision,
            &contract_digest,
            &policy_digest,
            &policy.protected_oracle_digest,
        );

        let host = SessionHost::new();
        let executed = execute_checks(
            &host,
            &inputs,
            &fixture.contract,
            &policy,
            candidate.checkout_path(),
            &fixture.spool_root,
            "check-run-dirty",
        )
        .expect("trusted_checks::execute_checks succeeds directly on candidate.checkout_path()");
        assert_eq!(executed.receipt().checks[0].exit_code, Some(0));

        let wrong_verifier = PrincipalId::new("other-verifier").expect("other verifier");
        assert_eq!(
            candidate.decision_evidence(&executed, wrong_verifier),
            Err(SourceCandidateError::VerificationBindingMismatch)
        );

        let evidence = candidate
            .decision_evidence(&executed, verifier.clone())
            .expect("decision evidence bound to candidate and executed checks");
        assert_eq!(evidence.source_commit_sha, candidate.source_commit_sha());
        assert_eq!(evidence.source_tree_sha, candidate.source_tree_sha());
        assert_eq!(evidence.custody_digest, candidate.custody_digest());
        assert_eq!(evidence.closure_digest, candidate.closure_digest());

        let worker = PrincipalId::new("worker-1").expect("worker");
        let mut projection = Projection::default();
        projection = reduce(
            &projection,
            &DomainEvent::TaskSubmitted {
                contract: fixture.contract.clone(),
            },
        )
        .expect("submit task");
        projection = reduce(
            &projection,
            &DomainEvent::TaskClaimed {
                task_id: fixture.contract.task_id.clone(),
                revision: fixture.contract.revision,
                attempt_id: candidate.attempt_id().clone(),
                owner: worker,
            },
        )
        .expect("claim task");
        projection = reduce(
            &projection,
            &DomainEvent::TaskStarted {
                task_id: fixture.contract.task_id.clone(),
                attempt_id: candidate.attempt_id().clone(),
            },
        )
        .expect("start task");
        projection = reduce(&projection, &candidate.candidate_submitted_event())
            .expect("submit source candidate");
        let task_before_decision = projection
            .tasks
            .get(&fixture.contract.task_id)
            .expect("task in projection");
        let expected_task_digest =
            content_digest(task_before_decision).expect("expected task digest");
        projection = reduce(
            &projection,
            &DomainEvent::TaskLifecycle {
                command: LifecycleCommand {
                    protocol_version: 2,
                    task_id: fixture.contract.task_id.clone(),
                    expected_task_digest,
                    actor: verifier,
                    action: LifecycleAction::DecideCandidate {
                        attempt_id: candidate.attempt_id().clone(),
                        candidate_ref: candidate.candidate_ref().to_owned(),
                        evidence,
                        accepted: true,
                    },
                },
            },
        )
        .expect("decide candidate");
        assert_eq!(
            projection.tasks[&fixture.contract.task_id].state,
            SchedulingState::Accepted
        );
    }

    #[test]
    fn precommitted_head_materializes_and_preserves_exact_commit_identity() {
        let mut fixture = setup_fixture(vec!["src"], vec!["src/secret"]);
        let workspace_path = fixture.workspace_path.clone();
        fs::write(
            workspace_path.join("src/lib.rs"),
            b"pub fn answer() -> u32 {\n    42\n}\n",
        )
        .expect("edit src/lib.rs");
        run_fixture_git(&workspace_path, &["add", "src/lib.rs"]);
        run_fixture_git(
            &workspace_path,
            &["commit", "--quiet", "-m", "provider commit"],
        );
        let provider_commit = run_fixture_git(&workspace_path, &["rev-parse", "HEAD"]);
        make_git_dir_private(&workspace_path.join(".git")).expect("private .git after commit");

        let closed = prove_closed_workspace(fixture.take_prepared(), fixture.closure.clone())
            .expect("prove closed");
        let candidate =
            materialize_source_candidate(closed, &fixture.candidate_parent, "candidate-committed")
                .expect("materialize precommitted candidate");

        assert_eq!(candidate.source_commit_sha(), provider_commit);
        assert_eq!(candidate.files().len(), 1);
        assert_eq!(candidate.files()[0].path, "src/lib.rs");
        assert!(candidate.files()[0].hunks.is_some());
    }

    #[test]
    fn prove_closed_workspace_rejects_failed_exit_or_mismatched_identity() {
        let mut fixture = setup_fixture(vec!["src"], vec![]);
        let mut failed_closure = fixture.closure.clone();
        failed_closure.success = false;
        failed_closure.exit_code = Some(1);
        assert_eq!(
            prove_closed_workspace(fixture.take_prepared(), failed_closure).unwrap_err(),
            SourceCandidateError::ExecutionNotSucceeded
        );

        let mut fixture = setup_fixture(vec!["src"], vec![]);
        let mut wrong_task = fixture.closure.clone();
        wrong_task.identity.identity.task_id = TaskId::new("OTHER-TASK").expect("other task");
        assert_eq!(
            prove_closed_workspace(fixture.take_prepared(), wrong_task).unwrap_err(),
            SourceCandidateError::ClosureIdentityMismatch
        );

        let mut fixture = setup_fixture(vec!["src"], vec![]);
        let mut wrong_cwd = fixture.closure.clone();
        wrong_cwd.observation.actual_cwd = fixture.root.join("other-cwd");
        assert_eq!(
            prove_closed_workspace(fixture.take_prepared(), wrong_cwd).unwrap_err(),
            SourceCandidateError::ClosureIdentityMismatch
        );
    }

    #[test]
    fn materialize_source_candidate_rejects_noop_execution() {
        let mut fixture = setup_fixture(vec!["src"], vec!["src/secret"]);
        let closed = prove_closed_workspace(fixture.take_prepared(), fixture.closure.clone())
            .expect("prove closed");
        let err = materialize_source_candidate(closed, &fixture.candidate_parent, "candidate-noop")
            .unwrap_err();
        assert_eq!(err, SourceCandidateError::NoSourceChanges);
    }

    #[test]
    fn materialize_source_candidate_rejects_outside_owned_paths_and_forbidden_paths() {
        let mut fixture = setup_fixture(vec!["src"], vec!["src/secret"]);
        fs::write(
            fixture.workspace_path.join("docs/readme.md"),
            b"# Unauthorized doc edit\n",
        )
        .expect("modify unowned docs/readme.md");
        let closed = prove_closed_workspace(fixture.take_prepared(), fixture.closure.clone())
            .expect("prove closed");
        let err = materialize_source_candidate(
            closed,
            &fixture.candidate_parent,
            "candidate-outside-owned",
        )
        .unwrap_err();
        assert_eq!(
            err,
            SourceCandidateError::ScopeViolation {
                path: "docs/readme.md".into()
            }
        );

        let mut fixture = setup_fixture(vec!["src"], vec!["src/secret"]);
        fs::write(
            fixture.workspace_path.join("src/secret/key.txt"),
            b"Compromised\n",
        )
        .expect("modify forbidden src/secret/key.txt");
        let closed = prove_closed_workspace(fixture.take_prepared(), fixture.closure.clone())
            .expect("prove closed");
        let err =
            materialize_source_candidate(closed, &fixture.candidate_parent, "candidate-forbidden")
                .unwrap_err();
        assert_eq!(
            err,
            SourceCandidateError::ScopeViolation {
                path: "src/secret/key.txt".into()
            }
        );
    }

    #[test]
    fn materialize_source_candidate_rejects_symlinks_and_unsafe_git_config() {
        let mut fixture = setup_fixture(vec!["src"], vec!["src/secret"]);
        std::os::unix::fs::symlink(
            fixture.workspace_path.join("src/lib.rs"),
            fixture.workspace_path.join("src/escape_link.rs"),
        )
        .expect("create symlink inside working tree");
        let closed = prove_closed_workspace(fixture.take_prepared(), fixture.closure.clone())
            .expect("prove closed");
        let err =
            materialize_source_candidate(closed, &fixture.candidate_parent, "candidate-symlink")
                .unwrap_err();
        assert_eq!(err, SourceCandidateError::UnsafeWorkspaceMetadata);

        let mut fixture = setup_fixture(vec!["src"], vec!["src/secret"]);
        fs::write(
            fixture.workspace_path.join("src/lib.rs"),
            b"pub fn answer() -> u32 {\n    42\n}\n",
        )
        .expect("edit owned file");
        let config_path = fixture.workspace_path.join(".git/config");
        let mut config_bytes = fs::read(&config_path).expect("read .git/config");
        config_bytes.extend_from_slice(b"\n[core]\n\tfsmonitor = /bin/sh\n");
        fs::write(&config_path, config_bytes).expect("inject unsafe fsmonitor config");
        fs::set_permissions(&config_path, fs::Permissions::from_mode(0o600))
            .expect("keep .git/config mode 0600");
        let closed = prove_closed_workspace(fixture.take_prepared(), fixture.closure.clone())
            .expect("prove closed");
        let err = materialize_source_candidate(
            closed,
            &fixture.candidate_parent,
            "candidate-unsafe-config",
        )
        .unwrap_err();
        assert_eq!(err, SourceCandidateError::UnsafeWorkspaceMetadata);

        // Also verify trusted_checks rejects tampered source_commit_sha against our checkout
        let mut fixture = setup_fixture(vec!["src"], vec!["src/secret"]);
        fs::write(
            fixture.workspace_path.join("src/lib.rs"),
            b"pub fn answer() -> u32 {\n    42\n}\n",
        )
        .expect("edit owned file");
        let closed = prove_closed_workspace(fixture.take_prepared(), fixture.closure.clone())
            .expect("prove closed");
        let candidate =
            materialize_source_candidate(closed, &fixture.candidate_parent, "candidate-valid")
                .expect("materialize valid candidate");
        let contract_digest = content_digest(&fixture.contract).expect("contract digest");
        let verifier = PrincipalId::new("independent-verifier").expect("verifier");
        let policy = test_policy(verifier);
        let policy_digest = content_digest(&policy).expect("policy digest");
        let mut bad_inputs = candidate.verification_inputs(
            fixture.contract.revision,
            &contract_digest,
            &policy_digest,
            &policy.protected_oracle_digest,
        );
        bad_inputs.source_commit_sha = candidate.base_commit_sha().to_owned();
        let host = SessionHost::new();
        let check_err = execute_checks(
            &host,
            &bad_inputs,
            &fixture.contract,
            &policy,
            candidate.checkout_path(),
            &fixture.spool_root,
            "check-run-mismatch",
        )
        .unwrap_err();
        assert!(matches!(check_err, TrustedCheckError::CandidateDrift));
    }
}
