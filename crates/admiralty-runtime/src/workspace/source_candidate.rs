//! Post-execution changed-source candidate custody and detached checkout materialization.
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
//!      [`prove_closed_workspace`] to verify [`PreparedWorkspace::validate_current_custody`]
//!      against the original retained parent/workspace descriptors and bind the
//!      [`PreparedWorkspace`] and [`ClosedAdmission`] into a
//!      [`ClosedWorkspaceExecution`].
//!    - Raw filesystem paths, agent JSON claims, or captured session transcript
//!      bytes/digests are never accepted as workspace custody or candidate source.
//!
//! 2. **`controller` lifecycle flow (`TaskStarted` -> `CandidateSubmitted` -> `DecideCandidate`):**
//!    - After recording `DomainEvent::TaskStarted`, trusted composition calls
//!      [`materialize_source_candidate`] with the [`ClosedWorkspaceExecution`] and
//!      a private mode-`0700` `candidate_parent` directory.
//!    - [`materialize_source_candidate`] re-validates the original
//!      [`PreparedWorkspace`] descriptor custody and `.git` metadata, freezes
//!      dirty working-tree edits or verifies pre-committed provider commits on
//!      top of `base_commit_sha` without external diff/textconv/filter drivers,
//!      enforces `owned_paths` and `forbidden_paths`, validates the resulting
//!      [`CandidateChange`] via [`admiralty_core::conflicts::parse_candidates`],
//!      and materializes a clean, detached mode-`0700` checkout at `source_commit_sha`.
//!    - Because a mode-`0700` checkout remains writable by the same UID,
//!      [`SourceCandidate::revalidate_custody`] and
//!      [`SourceCandidate::decision_evidence`] re-verify both original workspace
//!      and candidate checkout descriptor identity, `.git` safety, clean status,
//!      commit/tree OIDs, and actual on-disk file content digests before minting
//!      [`CandidateDecisionEvidence`].
//!
//! 3. **Qualification Gate:**
//!    - Opaque type names and `compile_fail` serde checks are local component
//!      guards only and do not qualify production candidate acceptance or
//!      `native_execution_qualified`. `NativeExecutor` and `controller` startup
//!      composition remain Root integration work.

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
use rustix::fs::{AtFlags, Mode, OFlags, fchmod, fstat, fstatvfs, mkdirat, open, openat, statat};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, Read, Write},
    os::{
        fd::{AsFd, BorrowedFd, OwnedFd},
        unix::{
            fs::{FileTypeExt, MetadataExt},
            process::CommandExt,
        },
    },
    path::{Component, Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};
use thiserror::Error;

const TRUSTED_GIT: [&str; 3] = ["/usr/bin/git", "/bin/git", "/usr/local/bin/git"];
const GIT_TIMEOUT: Duration = Duration::from_secs(5);
const GIT_TRANSFER_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_GIT_OUTPUT: usize = 1024 * 1024;
const MAX_GIT_STDERR: usize = 16 * 1024;
const MAX_TREE_LISTING: usize = 4 * 1024 * 1024;
const MAX_GIT_CONFIG_BYTES: u64 = 64 * 1024;
const MAX_GIT_HOOK_ENTRIES: usize = 64;
const MAX_GIT_DIR_DEPTH: usize = 16;
const MAX_GIT_DIR_ENTRIES: usize = 4_096;
const MAX_GIT_DIR_BYTES: u64 = 64 * 1024 * 1024;
const MAX_COMMIT_DISTANCE: usize = 64;
const MAX_SOURCE_FILE_BYTES: u64 = 1024 * 1024;
const MAX_WORKING_TREE_ENTRIES: usize = 10_000;
const MAX_WORKING_TREE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_DESTINATION_NAME_BYTES: usize = 128;
const DISK_RESERVE_BYTES: u64 = 16 * 1024 * 1024;
const DISK_BYTES_PER_OBJECT: u64 = 4 * 1024;
const DISK_BYTES_PER_FILE: u64 = 4 * 1024;
const DISK_INODE_RESERVE: u64 = 64;
const DETERMINISTIC_AUTHOR_NAME: &str = "Admiralty Source Candidate";
const DETERMINISTIC_AUTHOR_EMAIL: &str = "source-candidate@admiralty.invalid";
const DETERMINISTIC_COMMIT_DATE: &str = "1970-01-01T00:00:00Z";

mod sealed {
    use super::ClosedAdmission;

    #[derive(Debug)]
    pub struct RetainedClosureProof(pub(super) RetainedClosureProofInner);

    #[derive(Debug)]
    pub(super) enum RetainedClosureProofInner {
        Production(ClosedAdmission),
        #[cfg(test)]
        SyntheticTestFixture(super::tests::FixtureClosedAdmission),
    }

    pub trait Sealed {
        fn into_retained_proof(self) -> RetainedClosureProof;
    }
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

impl sealed::Sealed for ClosedAdmission {
    fn into_retained_proof(self) -> sealed::RetainedClosureProof {
        sealed::RetainedClosureProof(sealed::RetainedClosureProofInner::Production(self))
    }
}

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
    #[error(
        "source working tree was frozen by an earlier failed materialization attempt and requires explicit reconciliation before retry"
    )]
    SourceReconciliationRequired,
    #[error("changed path violates contract write scope: {path}")]
    ScopeViolation { path: String },
    #[error("candidate diff failed conflict model validation: {0}")]
    ConflictValidation(String),
    #[error("trusted check execution receipt does not match source candidate identity")]
    VerificationBindingMismatch,
    #[error("canonical content digest could not be computed")]
    Digest,
}

/// Observation recorded on [`ClosedWorkspaceExecution`] when a materialization
/// attempt froze dirty working-tree changes into a commit on top of
/// `prepared.head_oid()` before a subsequent step failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrozenSourceObservation {
    pub base_commit_sha: String,
    pub frozen_commit_sha: String,
    pub frozen_tree_sha: String,
    pub acknowledged_for_retry: bool,
}

/// Stage at which [`try_materialize_source_candidate`] failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MaterializationFailureStage {
    /// Failed while validating the closed execution workspace, `.git` metadata,
    /// working tree, or base commit closure before any source freeze or
    /// destination creation.
    SourcePreflight,
    /// Failed while validating `candidate_parent` or `destination_name` (including
    /// when `destination_name` already exists beneath `candidate_parent`) before
    /// any source freeze or new destination creation.
    DestinationPreflight,
    /// Failed while staging or committing dirty working-tree changes in the
    /// execution workspace before creating the destination directory.
    SourceFreeze,
    /// Failed while validating the frozen/post-execution source commit, tree
    /// closure, or path scope (`owned_paths` / `forbidden_paths`) before creating
    /// the destination directory.
    SourceValidation,
    /// Failed after creating the destination directory beneath `candidate_parent`
    /// while initializing, fetching, checking out, or validating the detached
    /// candidate checkout.
    DestinationCheckout,
    /// Failed during final candidate conflict-model or custody revalidation after
    /// the destination checkout was materialized.
    PostMaterializationValidation,
}

/// Truthful observation of any side effects or required reconciliation when
/// [`try_materialize_source_candidate`] fails.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MaterializationReconciliation {
    /// Stage at which candidate materialization failed.
    pub stage: MaterializationFailureStage,
    /// Whether `freeze_dirty_working_tree` mutated the execution workspace index
    /// or `HEAD` during this or an earlier unreconciled attempt.
    pub source_working_tree_frozen: bool,
    /// Observed `HEAD` commit OID in the execution workspace after failure, if
    /// readable.
    pub observed_source_head_oid: Option<String>,
    /// Observed `HEAD^{tree}` OID in the execution workspace after failure, if
    /// readable.
    pub observed_source_tree_oid: Option<String>,
    /// Pre-existing destination path when `destination_name` was already occupied
    /// before this attempt (`DestinationPreflight`). Never deleted or overwritten.
    pub occupied_destination_path: Option<PathBuf>,
    /// Newly created partial destination directory path when `mkdirat` succeeded
    /// during this attempt before a subsequent step failed. Preserved on disk for
    /// inspection; callers must use a fresh `destination_name` or explicitly
    /// reconcile before retrying.
    pub partial_destination_path: Option<PathBuf>,
    /// Device and inode identity `(dev, ino)` of `partial_destination_path` if
    /// created during this attempt.
    pub partial_destination_identity: Option<(u64, u64)>,
    /// Whether retrying requires explicit caller reconciliation or a fresh
    /// create-only `destination_name`.
    pub requires_reconciliation: bool,
}

/// Error returned by [`try_prove_closed_workspace`] preserving the caller's
/// original [`PreparedWorkspace`] and generic sealed [`ClosedAdmissionProof`]
/// handles without dropping or recreating them.
#[derive(Debug, Error)]
#[error("{error}")]
pub struct ProveClosedWorkspaceError<C> {
    pub prepared: PreparedWorkspace,
    pub closure: C,
    #[source]
    pub error: SourceCandidateError,
}

impl<C> ProveClosedWorkspaceError<C> {
    pub fn into_parts(self) -> (PreparedWorkspace, C, SourceCandidateError) {
        (self.prepared, self.closure, self.error)
    }

    pub fn prepared(&self) -> &PreparedWorkspace {
        &self.prepared
    }

    pub fn closure(&self) -> &C {
        &self.closure
    }

    pub fn error(&self) -> &SourceCandidateError {
        &self.error
    }
}

/// Error returned by [`try_materialize_source_candidate`] preserving the caller's
/// original [`ClosedWorkspaceExecution`] (including its original [`PreparedWorkspace`]
/// and retained closure proof handles) alongside explicit stage and partial-destination
/// reconciliation metadata.
#[derive(Debug, Error)]
#[error("{error} (stage: {stage:?})", stage = self.reconciliation.stage)]
pub struct MaterializeSourceCandidateError {
    pub execution: ClosedWorkspaceExecution,
    #[source]
    pub error: SourceCandidateError,
    pub reconciliation: MaterializationReconciliation,
}

impl MaterializeSourceCandidateError {
    pub fn into_parts(
        self,
    ) -> (
        ClosedWorkspaceExecution,
        SourceCandidateError,
        MaterializationReconciliation,
    ) {
        (self.execution, self.error, self.reconciliation)
    }

    pub fn execution(&self) -> &ClosedWorkspaceExecution {
        &self.execution
    }

    pub fn into_execution(self) -> ClosedWorkspaceExecution {
        self.execution
    }

    pub fn error(&self) -> &SourceCandidateError {
        &self.error
    }

    pub fn reconciliation(&self) -> &MaterializationReconciliation {
        &self.reconciliation
    }

    pub fn stage(&self) -> MaterializationFailureStage {
        self.reconciliation.stage
    }

    pub fn partial_destination_path(&self) -> Option<&Path> {
        self.reconciliation.partial_destination_path.as_deref()
    }

    pub fn occupied_destination_path(&self) -> Option<&Path> {
        self.reconciliation.occupied_destination_path.as_deref()
    }

    pub fn requires_reconciliation(&self) -> bool {
        self.reconciliation.requires_reconciliation
    }
}

/// Type alias for [`ProveClosedWorkspaceError`].
pub type ProveClosedWorkspaceFailure<C> = ProveClosedWorkspaceError<C>;

/// Type alias for [`MaterializeSourceCandidateError`].
pub type MaterializeSourceCandidateFailure = MaterializeSourceCandidateError;

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
    parent_identity: (u64, u64),
    workspace_identity: (u64, u64),
    identity: RecoveryIdentity,
    observation: EnforcementObservation,
    exit_code: Option<i32>,
    success: bool,
    closure_digest: String,
    workspace_digest: String,
    retained_closure: sealed::RetainedClosureProof,
    freeze_fence_active: bool,
    frozen_source: Option<FrozenSourceObservation>,
}

impl ClosedWorkspaceExecution {
    pub fn prepared(&self) -> &PreparedWorkspace {
        &self.prepared
    }

    pub fn parent_descriptor_identity(&self) -> (u64, u64) {
        self.parent_identity
    }

    pub fn workspace_descriptor_identity(&self) -> (u64, u64) {
        self.workspace_identity
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
        match &self.retained_closure.0 {
            sealed::RetainedClosureProofInner::Production(admission) => Some(admission),
            #[cfg(test)]
            sealed::RetainedClosureProofInner::SyntheticTestFixture(_) => None,
        }
    }

    pub fn closure_proof(&self) -> &dyn ClosedAdmissionProof {
        match &self.retained_closure.0 {
            sealed::RetainedClosureProofInner::Production(admission) => admission,
            #[cfg(test)]
            sealed::RetainedClosureProofInner::SyntheticTestFixture(fixture) => fixture,
        }
    }

    #[cfg(test)]
    fn synthetic_test_closure_proof(&self) -> Option<&tests::FixtureClosedAdmission> {
        match &self.retained_closure.0 {
            sealed::RetainedClosureProofInner::Production(_) => None,
            sealed::RetainedClosureProofInner::SyntheticTestFixture(fixture) => Some(fixture),
        }
    }

    /// Returns `true` if a mutating `SourceFreeze` was initiated on this
    /// execution workspace during this or an earlier materialization attempt.
    /// Once set, this fence remains active across every subsequent failure even
    /// if post-freeze diagnostic observations were unavailable.
    pub fn freeze_fence_active(&self) -> bool {
        self.freeze_fence_active
    }

    pub fn frozen_source(&self) -> Option<&FrozenSourceObservation> {
        self.frozen_source.as_ref()
    }

    /// Explicitly acknowledge and reconcile a dirty source workspace that was
    /// frozen into `(expected_frozen_commit_sha, expected_frozen_tree_sha)` during
    /// an earlier failed materialization attempt, allowing a subsequent
    /// [`try_materialize_source_candidate`] call into a fresh `destination_name`
    /// without re-freezing under a new identity.
    ///
    /// If an earlier `SourceFreeze` failed before producing a verified
    /// [`FrozenSourceObservation`] (`freeze_fence_active == true` and
    /// `frozen_source.is_none()`), the unknown freeze state is conservatively
    /// held and cannot be cleared through a new call or fake hash.
    pub fn acknowledge_frozen_source_reconciliation(
        &mut self,
        expected_frozen_commit_sha: &str,
        expected_frozen_tree_sha: &str,
    ) -> Result<(), SourceCandidateError> {
        self.revalidate_custody()?;
        if !self.freeze_fence_active {
            return Err(SourceCandidateError::SourceReconciliationRequired);
        }
        let Some(frozen) = self.frozen_source.as_ref() else {
            return Err(SourceCandidateError::SourceReconciliationRequired);
        };
        if frozen.frozen_commit_sha != expected_frozen_commit_sha
            || frozen.frozen_tree_sha != expected_frozen_tree_sha
            || !valid_sha1_oid(expected_frozen_commit_sha)
            || !valid_sha1_oid(expected_frozen_tree_sha)
        {
            return Err(SourceCandidateError::SourceReconciliationRequired);
        }
        let git = trusted_git()?;
        let workspace_cwd = PinnedGitDirectory::from_prepared(&self.prepared)?;
        validate_workspace_git_metadata(&git, &workspace_cwd, self.prepared.workspace_dir_fd())?;
        validate_working_tree_entries(&workspace_cwd)?;
        let status = git_status_porcelain(&git, &workspace_cwd)?;
        let current_head = git_text(
            &git,
            Some(&workspace_cwd),
            &["rev-parse", "--verify", "HEAD"],
        )?
        .trim()
        .to_owned();
        let current_tree = git_text(
            &git,
            Some(&workspace_cwd),
            &["rev-parse", "--verify", "HEAD^{tree}"],
        )?
        .trim()
        .to_owned();
        if !status.trim().is_empty()
            || current_head != expected_frozen_commit_sha
            || current_tree != expected_frozen_tree_sha
        {
            return Err(SourceCandidateError::SourceReconciliationRequired);
        }
        if let Some(frozen_mut) = self.frozen_source.as_mut() {
            frozen_mut.acknowledged_for_retry = true;
        }
        Ok(())
    }

    /// Re-verify that the [`PreparedWorkspace`] and its parent directory on disk
    /// still resolve without symlinks to the exact original directory descriptors
    /// retained when the workspace was prepared.
    pub fn revalidate_custody(&self) -> Result<(), SourceCandidateError> {
        self.prepared
            .validate_current_custody()
            .map_err(|_| SourceCandidateError::UnsafeWorkspaceMetadata)?;
        if self.prepared.workspace().custody != Custody::RegisteredOnly
            || self.prepared.workspace().git_metadata != GitMetadata::Directory
            || fd_identity(self.prepared.managed_parent_fd())? != self.parent_identity
            || fd_identity(self.prepared.workspace_dir_fd())? != self.workspace_identity
        {
            return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
        }
        let pinned = PinnedGitDirectory::from_prepared(&self.prepared)?;
        pinned.check()
    }
}

/// Verify that a closed service admission succeeded and matches the exact
/// task, attempt, project, root, node, and original retained descriptor custody
/// of `prepared`, returning both original handles in [`ProveClosedWorkspaceError`]
/// on any validation failure.
#[allow(clippy::result_large_err)]
pub fn try_prove_closed_workspace<C: ClosedAdmissionProof>(
    prepared: PreparedWorkspace,
    closure: C,
) -> Result<ClosedWorkspaceExecution, ProveClosedWorkspaceError<C>> {
    match validate_closed_workspace_proof(&prepared, &closure) {
        Ok((
            parent_identity,
            workspace_identity,
            identity,
            observation,
            exit_code,
            success,
            closure_digest,
            workspace_digest,
        )) => {
            let retained_closure = closure.into_retained_proof();
            Ok(ClosedWorkspaceExecution {
                prepared,
                parent_identity,
                workspace_identity,
                identity,
                observation,
                exit_code,
                success,
                closure_digest,
                workspace_digest,
                retained_closure,
                freeze_fence_active: false,
                frozen_source: None,
            })
        }
        Err(error) => Err(ProveClosedWorkspaceError {
            prepared,
            closure,
            error,
        }),
    }
}

/// Compatibility wrapper around [`try_prove_closed_workspace`] that discards
/// the returned handles on error. New native composition must call
/// [`try_prove_closed_workspace`] to retain custody of `prepared` and `closure`.
pub fn prove_closed_workspace<C: ClosedAdmissionProof>(
    prepared: PreparedWorkspace,
    closure: C,
) -> Result<ClosedWorkspaceExecution, SourceCandidateError> {
    try_prove_closed_workspace(prepared, closure).map_err(|failure| failure.error)
}

#[allow(clippy::type_complexity)]
fn validate_closed_workspace_proof<C: ClosedAdmissionProof>(
    prepared: &PreparedWorkspace,
    closure: &C,
) -> Result<
    (
        (u64, u64),
        (u64, u64),
        RecoveryIdentity,
        EnforcementObservation,
        Option<i32>,
        bool,
        String,
        String,
    ),
    SourceCandidateError,
> {
    if !closure.success() || closure.exit_code() != Some(0) {
        return Err(SourceCandidateError::ExecutionNotSucceeded);
    }
    let closure_digest = closure.closure_digest().to_owned();
    if !valid_sha256_digest(&closure_digest) {
        return Err(SourceCandidateError::ClosureIdentityMismatch);
    }
    prepared
        .validate_current_custody()
        .map_err(|_| SourceCandidateError::UnsafeWorkspaceMetadata)?;
    let ws = prepared.workspace();
    if ws.custody != Custody::RegisteredOnly || ws.git_metadata != GitMetadata::Directory {
        return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
    }
    let parent_identity = fd_identity(prepared.managed_parent_fd())?;
    let workspace_identity = fd_identity(prepared.workspace_dir_fd())?;
    let pinned = PinnedGitDirectory::from_prepared(prepared)?;
    pinned.check()?;

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
    Ok((
        parent_identity,
        workspace_identity,
        identity,
        observation,
        closure.exit_code(),
        closure.success(),
        closure_digest,
        workspace_digest,
    ))
}

/// Opaque, non-deserializable changed-source candidate holding a detached,
/// clean mode-`0700` Git checkout at `source_commit_sha` whose descriptor and
/// content custody are re-validated before acceptance.
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
    candidate_parent_path: PathBuf,
    destination_name: String,
    candidate_parent_identity: (u64, u64),
    candidate_checkout_identity: (u64, u64),
    candidate_config: Vec<u8>,
    working_tree_digest: String,
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

    /// Re-verify original prepared workspace custody, candidate checkout
    /// descriptor custody, `.git` metadata, working-tree cleanliness, and
    /// actual on-disk file content digests.
    pub fn revalidate_custody(&self) -> Result<(), SourceCandidateError> {
        self.execution.revalidate_custody()?;
        if fd_identity(self._candidate_parent_fd.as_fd())? != self.candidate_parent_identity
            || fd_identity(self._candidate_checkout_fd.as_fd())? != self.candidate_checkout_identity
        {
            return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
        }
        let checkout_cwd = PinnedGitDirectory::with_parent(
            self.checkout_path(),
            self._candidate_checkout_fd.as_fd(),
            &self.candidate_parent_path,
            self._candidate_parent_fd.as_fd(),
            &self.destination_name,
        )?;
        let revalidated = WorkspaceSpec {
            node_id: self.checkout_workspace.node_id.clone(),
            root_id: self.checkout_workspace.root_id.clone(),
            project_id: self.checkout_workspace.project_id.clone(),
            workspace: self.checkout_workspace.workspace.clone(),
            owned_paths: self.checkout_workspace.owned_paths.clone(),
            forbidden_paths: self.checkout_workspace.forbidden_paths.clone(),
        }
        .validate()
        .map_err(|_| SourceCandidateError::UnsafeWorkspaceMetadata)?;
        if revalidated != self.checkout_workspace
            || revalidated.custody != Custody::RegisteredOnly
            || revalidated.git_metadata != GitMetadata::Directory
        {
            return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
        }
        let git = trusted_git()?;
        let current_config = validate_workspace_git_metadata(
            &git,
            &checkout_cwd,
            self._candidate_checkout_fd.as_fd(),
        )?;
        if current_config != self.candidate_config {
            return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
        }
        validate_working_tree_entries(&checkout_cwd)?;
        verify_clean_detached_checkout(
            &git,
            &checkout_cwd,
            &self.source_commit_sha,
            &self.source_tree_sha,
        )?;
        validate_commit_tree_entries(
            &git,
            &checkout_cwd,
            &self.source_commit_sha,
            &self.source_tree_sha,
        )?;
        let current_wt_digest =
            compute_working_tree_content_digest(&git, &checkout_cwd, &self.source_tree_sha)?;
        if current_wt_digest != self.working_tree_digest {
            return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
        }
        let current_files = extract_and_validate_changed_files(
            &git,
            &checkout_cwd,
            &self.checkout_workspace,
            &self.base_commit_sha,
            &self.source_commit_sha,
        )?;
        let current_artifact_digest = compute_source_artifact_digest(
            &self.base_commit_sha,
            &self.source_commit_sha,
            &self.source_tree_sha,
            &current_files,
        )?;
        if current_files != self.files || current_artifact_digest != self.source_artifact_digest {
            return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
        }
        checkout_cwd.check()
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

    /// Construct [`VerificationInputs`] bound to this candidate's
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

    /// Re-validate actual candidate checkout and workspace custody on disk and
    /// bind an [`ExecutedChecks`] proof to this candidate's custody and closure
    /// digests for `LifecycleAction::DecideCandidate`.
    pub fn decision_evidence(
        &self,
        executed_checks: &ExecutedChecks,
        verifier: PrincipalId,
    ) -> Result<CandidateDecisionEvidence, SourceCandidateError> {
        self.revalidate_custody()?;
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
/// relative to `prepared.head_oid()`, enforce path scope, and materialize a
/// clean, detached mode-`0700` checkout beneath `candidate_parent`, returning
/// the original [`ClosedWorkspaceExecution`] and explicit reconciliation
/// observations in [`MaterializeSourceCandidateError`] on any failure.
#[allow(clippy::result_large_err)]
pub fn try_materialize_source_candidate(
    execution: ClosedWorkspaceExecution,
    candidate_parent: &Path,
    destination_name: &str,
) -> Result<SourceCandidate, MaterializeSourceCandidateError> {
    try_materialize_source_candidate_with_hook(execution, candidate_parent, destination_name, None)
}

/// Compatibility wrapper around [`try_materialize_source_candidate`] that
/// discards the returned [`ClosedWorkspaceExecution`] and reconciliation details
/// on error. New native composition must call [`try_materialize_source_candidate`].
pub fn materialize_source_candidate(
    execution: ClosedWorkspaceExecution,
    candidate_parent: &Path,
    destination_name: &str,
) -> Result<SourceCandidate, SourceCandidateError> {
    try_materialize_source_candidate(execution, candidate_parent, destination_name)
        .map_err(|failure| failure.error)
}

type PostDestinationInitHook<'a> = Option<&'a dyn Fn(&Path)>;

#[allow(clippy::result_large_err)]
fn try_materialize_source_candidate_with_hook(
    mut execution: ClosedWorkspaceExecution,
    candidate_parent: &Path,
    destination_name: &str,
    post_destination_init_hook: PostDestinationInitHook<'_>,
) -> Result<SourceCandidate, MaterializeSourceCandidateError> {
    let mut source_working_tree_frozen =
        execution.freeze_fence_active || execution.frozen_source.is_some();
    let fail = |mut execution: ClosedWorkspaceExecution,
                error: SourceCandidateError,
                stage: MaterializationFailureStage,
                source_frozen: bool,
                observed_head: Option<String>,
                observed_tree: Option<String>,
                occupied_destination_path: Option<PathBuf>,
                partial_destination_path: Option<PathBuf>,
                partial_destination_identity: Option<(u64, u64)>,
                requires_reconciliation: bool|
     -> MaterializeSourceCandidateError {
        if source_frozen {
            execution.freeze_fence_active = true;
        }
        MaterializeSourceCandidateError {
            execution,
            error,
            reconciliation: MaterializationReconciliation {
                stage,
                source_working_tree_frozen: source_frozen,
                observed_source_head_oid: observed_head,
                observed_source_tree_oid: observed_tree,
                occupied_destination_path,
                partial_destination_path,
                partial_destination_identity,
                requires_reconciliation,
            },
        }
    };

    let git = match trusted_git() {
        Ok(git) => git,
        Err(error) => {
            return Err(fail(
                execution,
                error,
                MaterializationFailureStage::SourcePreflight,
                source_working_tree_frozen,
                None,
                None,
                None,
                None,
                None,
                source_working_tree_frozen,
            ));
        }
    };

    let mut preflight_validated_observation: Option<(String, String)> = None;
    let preflight_result: Result<
        (String, String, PreflightTreeClosure, String, String, String),
        SourceCandidateError,
    > = (|| {
        execution.revalidate_custody()?;
        if execution.freeze_fence_active && execution.frozen_source.is_none() {
            return Err(SourceCandidateError::SourceReconciliationRequired);
        }
        let workspace_fd = execution.prepared.workspace_dir_fd();
        let workspace_cwd = PinnedGitDirectory::from_prepared(&execution.prepared)?;
        validate_workspace_git_metadata(&git, &workspace_cwd, workspace_fd)?;
        validate_working_tree_entries(&workspace_cwd)?;

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
        let resolved_base_tree = git_text(
            &git,
            Some(&workspace_cwd),
            &[
                "rev-parse",
                "--verify",
                &format!("{base_commit_sha}^{{tree}}"),
            ],
        )?;
        if resolved_base_tree.trim() != base_tree_sha {
            return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
        }
        let base_closure =
            validate_commit_tree_entries(&git, &workspace_cwd, &base_commit_sha, &base_tree_sha)?;

        let status_before = git_status_porcelain(&git, &workspace_cwd)?;
        let current_head = git_text(
            &git,
            Some(&workspace_cwd),
            &["rev-parse", "--verify", "HEAD"],
        )?
        .trim()
        .to_owned();
        let current_tree = git_text(
            &git,
            Some(&workspace_cwd),
            &["rev-parse", "--verify", "HEAD^{tree}"],
        )?
        .trim()
        .to_owned();
        if !valid_sha1_oid(&current_head) || !valid_sha1_oid(&current_tree) {
            return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
        }
        preflight_validated_observation = Some((current_head.clone(), current_tree.clone()));
        if execution.freeze_fence_active {
            let Some(frozen) = execution.frozen_source.as_ref() else {
                return Err(SourceCandidateError::SourceReconciliationRequired);
            };
            if !frozen.acknowledged_for_retry
                || !status_before.trim().is_empty()
                || current_head != frozen.frozen_commit_sha
                || current_tree != frozen.frozen_tree_sha
            {
                return Err(SourceCandidateError::SourceReconciliationRequired);
            }
        }
        if current_head != base_commit_sha {
            verify_commit_ancestry(&git, &workspace_cwd, &base_commit_sha, &current_head)?;
        }
        if status_before.trim().is_empty() && current_head == base_commit_sha {
            return Err(SourceCandidateError::NoSourceChanges);
        }
        Ok((
            base_commit_sha,
            base_tree_sha,
            base_closure,
            status_before,
            current_head,
            current_tree,
        ))
    })();

    let (base_commit_sha, base_tree_sha, base_closure, status_before, current_head, current_tree) =
        match preflight_result {
            Ok(values) => values,
            Err(error) => {
                let (obs_head, obs_tree) = match preflight_validated_observation {
                    Some((head, tree)) => (Some(head), Some(tree)),
                    None => (None, None),
                };
                return Err(fail(
                    execution,
                    error,
                    MaterializationFailureStage::SourcePreflight,
                    source_working_tree_frozen,
                    obs_head,
                    obs_tree,
                    None,
                    None,
                    None,
                    source_working_tree_frozen,
                ));
            }
        };

    // Validate candidate_parent and destination_name (including occupied-destination check)
    // BEFORE freezing any dirty working-tree edits or creating a destination directory.
    if let Err((error, occupied_path)) = preflight_candidate_destination(
        execution.prepared.workspace(),
        candidate_parent,
        destination_name,
    ) {
        let requires_reconciliation = source_working_tree_frozen || occupied_path.is_some();
        return Err(fail(
            execution,
            error,
            MaterializationFailureStage::DestinationPreflight,
            source_working_tree_frozen,
            Some(current_head),
            Some(current_tree),
            occupied_path,
            None,
            None,
            requires_reconciliation,
        ));
    }

    if !status_before.trim().is_empty() {
        execution.freeze_fence_active = true;
        source_working_tree_frozen = true;
        let freeze_res = (|| {
            let workspace_cwd = PinnedGitDirectory::from_prepared(&execution.prepared)?;
            freeze_dirty_working_tree(&git, &workspace_cwd)
        })();
        if let Err(error) = freeze_res {
            return Err(fail(
                execution,
                error,
                MaterializationFailureStage::SourceFreeze,
                true,
                None,
                None,
                None,
                None,
                None,
                true,
            ));
        }
    }

    let mut validated_source_observation: Option<(String, String)> = None;
    let source_validation_result: Result<
        (String, String, PreflightTreeClosure, Vec<FileChange>),
        SourceCandidateError,
    > = (|| {
        execution.revalidate_custody()?;
        let workspace_fd = execution.prepared.workspace_dir_fd();
        let workspace_cwd = PinnedGitDirectory::from_prepared(&execution.prepared)?;
        validate_workspace_git_metadata(&git, &workspace_cwd, workspace_fd)?;
        validate_working_tree_entries(&workspace_cwd)?;
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
        validated_source_observation = Some((source_commit_sha.clone(), source_tree_sha.clone()));
        if source_commit_sha == base_commit_sha || source_tree_sha == base_tree_sha {
            return Err(SourceCandidateError::NoSourceChanges);
        }
        verify_commit_ancestry(&git, &workspace_cwd, &base_commit_sha, &source_commit_sha)?;
        let source_closure = validate_commit_tree_entries(
            &git,
            &workspace_cwd,
            &source_commit_sha,
            &source_tree_sha,
        )?;
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
        Ok((source_commit_sha, source_tree_sha, source_closure, files))
    })();

    let (source_commit_sha, source_tree_sha, source_closure, files) = match source_validation_result
    {
        Ok(values) => values,
        Err(error) => {
            let (obs_head, obs_tree) = match validated_source_observation {
                Some((head, tree)) => (Some(head), Some(tree)),
                None => (None, None),
            };
            return Err(fail(
                execution,
                error,
                MaterializationFailureStage::SourceValidation,
                source_working_tree_frozen,
                obs_head,
                obs_tree,
                None,
                None,
                None,
                source_working_tree_frozen,
            ));
        }
    };

    if source_working_tree_frozen {
        execution.freeze_fence_active = true;
        execution.frozen_source = Some(FrozenSourceObservation {
            base_commit_sha: base_commit_sha.clone(),
            frozen_commit_sha: source_commit_sha.clone(),
            frozen_tree_sha: source_tree_sha.clone(),
            acknowledged_for_retry: false,
        });
    }

    let pre_artifacts: Result<(String, String, String, CandidateChange), SourceCandidateError> =
        (|| {
            let source_artifact_digest = compute_source_artifact_digest(
                &base_commit_sha,
                &source_commit_sha,
                &source_tree_sha,
                &files,
            )?;
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
            Ok((
                source_artifact_digest,
                custody_digest,
                candidate_ref,
                candidate_change,
            ))
        })();

    let (source_artifact_digest, custody_digest, candidate_ref, candidate_change) =
        match pre_artifacts {
            Ok(values) => values,
            Err(error) => {
                return Err(fail(
                    execution,
                    error,
                    MaterializationFailureStage::SourceValidation,
                    source_working_tree_frozen,
                    Some(source_commit_sha),
                    Some(source_tree_sha),
                    None,
                    None,
                    None,
                    source_working_tree_frozen,
                ));
            }
        };

    let checkout_result = (|| -> Result<MaterializedCheckout, DestinationCheckoutFailure> {
        let workspace_cwd =
            PinnedGitDirectory::from_prepared(&execution.prepared).map_err(|error| {
                DestinationCheckoutFailure {
                    error,
                    occupied_destination_path: None,
                    partial_destination_path: None,
                    partial_destination_identity: None,
                }
            })?;
        materialize_detached_checkout(
            &git,
            &workspace_cwd,
            execution.prepared.workspace(),
            candidate_parent,
            destination_name,
            &base_commit_sha,
            &source_commit_sha,
            &source_tree_sha,
            base_closure,
            source_closure,
            post_destination_init_hook,
        )
    })();

    let MaterializedCheckout {
        checkout_workspace,
        candidate_parent_path,
        candidate_parent_identity,
        candidate_checkout_identity,
        candidate_config,
        working_tree_digest,
        candidate_parent_fd,
        candidate_checkout_fd,
    } = match checkout_result {
        Ok(checkout) => checkout,
        Err(checkout_failure) => {
            let stage = if checkout_failure.partial_destination_path.is_some() {
                MaterializationFailureStage::DestinationCheckout
            } else {
                MaterializationFailureStage::DestinationPreflight
            };
            let requires_reconciliation = source_working_tree_frozen
                || checkout_failure.occupied_destination_path.is_some()
                || checkout_failure.partial_destination_path.is_some();
            return Err(fail(
                execution,
                checkout_failure.error,
                stage,
                source_working_tree_frozen,
                Some(source_commit_sha),
                Some(source_tree_sha),
                checkout_failure.occupied_destination_path,
                checkout_failure.partial_destination_path,
                checkout_failure.partial_destination_identity,
                requires_reconciliation,
            ));
        }
    };

    let candidate = SourceCandidate {
        execution,
        checkout_workspace,
        candidate_parent_path,
        destination_name: destination_name.to_owned(),
        candidate_parent_identity,
        candidate_checkout_identity,
        candidate_config,
        working_tree_digest,
        candidate_ref,
        base_commit_sha,
        base_tree_sha,
        source_commit_sha: source_commit_sha.clone(),
        source_tree_sha: source_tree_sha.clone(),
        source_artifact_digest,
        custody_digest,
        files,
        candidate_change,
        _candidate_parent_fd: candidate_parent_fd,
        _candidate_checkout_fd: candidate_checkout_fd,
    };
    if let Err(error) = candidate.revalidate_custody() {
        let partial_path = candidate.checkout_workspace.workspace.clone();
        let partial_identity = candidate.candidate_checkout_identity;
        let recovered_execution = candidate.execution;
        return Err(fail(
            recovered_execution,
            error,
            MaterializationFailureStage::PostMaterializationValidation,
            source_working_tree_frozen,
            Some(source_commit_sha),
            Some(source_tree_sha),
            None,
            Some(partial_path),
            Some(partial_identity),
            true,
        ));
    }
    Ok(candidate)
}

fn compute_source_artifact_digest(
    base_commit_sha: &str,
    source_commit_sha: &str,
    source_tree_sha: &str,
    files: &[FileChange],
) -> Result<String, SourceCandidateError> {
    content_digest(&(
        "source-candidate-artifact-v1",
        base_commit_sha,
        source_commit_sha,
        source_tree_sha,
        files,
    ))
    .map_err(|_| SourceCandidateError::Digest)
}

fn validate_candidate_change(change: &CandidateChange) -> Result<(), SourceCandidateError> {
    let encoded = serde_json::to_vec(&vec![change.clone()])
        .map_err(|error| SourceCandidateError::ConflictValidation(error.to_string()))?;
    parse_candidates(&encoded)
        .map(|_| ())
        .map_err(|error| SourceCandidateError::ConflictValidation(error.to_string()))
}

#[cfg(test)]
type AfterFreezeAddHook = Box<dyn FnOnce(&Path)>;

#[cfg(test)]
std::thread_local! {
    static TEST_AFTER_FREEZE_ADD_HOOK: std::cell::RefCell<Option<AfterFreezeAddHook>> =
        std::cell::RefCell::new(None);
}

#[cfg(test)]
fn set_test_after_freeze_add_hook(hook: impl FnOnce(&Path) + 'static) {
    TEST_AFTER_FREEZE_ADD_HOOK.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(hook));
    });
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
    #[cfg(test)]
    if let Some(hook) = TEST_AFTER_FREEZE_ADD_HOOK.with(|slot| slot.borrow_mut().take()) {
        hook(&workspace_cwd.access_path());
    }
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
    .map_err(|_| SourceCandidateError::InvalidBaseAncestry)?;
    let count_text = git_text(
        git,
        Some(workspace_cwd),
        &[
            "rev-list",
            "--count",
            &format!("{base_commit_sha}..{head_commit_sha}"),
        ],
    )
    .map_err(|_| SourceCandidateError::InvalidBaseAncestry)?;
    let distance = count_text
        .trim()
        .parse::<usize>()
        .map_err(|_| SourceCandidateError::InvalidBaseAncestry)?;
    if distance == 0 || distance > MAX_COMMIT_DISTANCE {
        return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug)]
struct PreflightTreeClosure {
    snapshot_bytes: u64,
    object_count: usize,
    working_tree_bytes: u64,
    materialized_entry_count: usize,
}

fn validate_commit_tree_entries(
    git: &Path,
    workspace_cwd: &PinnedGitDirectory<'_>,
    commit_oid: &str,
    tree_oid: &str,
) -> Result<PreflightTreeClosure, SourceCandidateError> {
    if !valid_sha1_oid(commit_oid) || !valid_sha1_oid(tree_oid) {
        return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
    }
    let listing = run_git_with_options(
        git,
        Some(workspace_cwd),
        &["ls-tree", "-r", "-t", "-l", "-z", "--full-tree", tree_oid],
        None,
        GIT_TIMEOUT,
        MAX_TREE_LISTING,
    )?
    .stdout;
    let mut entries = 0usize;
    let mut total_listed_bytes = 0u64;
    let mut objects = BTreeSet::new();
    let mut blob_references = BTreeMap::<String, usize>::new();
    let mut listed_blob_sizes = BTreeMap::<String, u64>::new();
    objects.insert(commit_oid.to_owned());
    objects.insert(tree_oid.to_owned());
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
        let size_field = fields
            .next()
            .ok_or(SourceCandidateError::UnsafeWorkspaceMetadata)?;
        if fields.next().is_some() || !valid_sha1_oid(oid) {
            return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
        }
        match (mode, kind) {
            ("040000", "tree") => {
                if size_field != "-" {
                    return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
                }
            }
            ("100644" | "100755", "blob") => {
                let blob_size = size_field
                    .parse::<u64>()
                    .map_err(|_| SourceCandidateError::UnsafeWorkspaceMetadata)?;
                if blob_size > MAX_SOURCE_FILE_BYTES {
                    return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
                }
                total_listed_bytes = total_listed_bytes
                    .checked_add(blob_size)
                    .filter(|bytes| *bytes <= MAX_WORKING_TREE_BYTES)
                    .ok_or(SourceCandidateError::UnsafeWorkspaceMetadata)?;
                let copies = blob_references.entry(oid.to_owned()).or_default();
                *copies = copies
                    .checked_add(1)
                    .ok_or(SourceCandidateError::UnsafeWorkspaceMetadata)?;
                if let Some(prev_size) = listed_blob_sizes.insert(oid.to_owned(), blob_size)
                    && prev_size != blob_size
                {
                    return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
                }
            }
            _ => return Err(SourceCandidateError::UnsafeWorkspaceMetadata),
        }
        objects.insert(oid.to_owned());
        if objects.len() > MAX_WORKING_TREE_ENTRIES {
            return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
        }
    }

    let mut batch_input = Vec::with_capacity(objects.len().saturating_mul(41));
    for oid in &objects {
        batch_input.extend_from_slice(oid.as_bytes());
        batch_input.push(b'\n');
    }
    let batch_output = run_git_with_options(
        git,
        Some(workspace_cwd),
        &[
            "cat-file",
            "--batch-check=%(objectname) %(objecttype) %(objectsize)",
        ],
        Some(&batch_input),
        GIT_TIMEOUT,
        MAX_GIT_OUTPUT,
    )?
    .stdout;
    let mut snapshot_bytes = 0u64;
    let mut object_count = 0usize;
    let mut object_sizes = BTreeMap::<String, u64>::new();
    for line in batch_output
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        let line =
            std::str::from_utf8(line).map_err(|_| SourceCandidateError::UnsafeWorkspaceMetadata)?;
        let mut fields = line.split_ascii_whitespace();
        let oid = fields
            .next()
            .ok_or(SourceCandidateError::UnsafeWorkspaceMetadata)?;
        let kind = fields
            .next()
            .ok_or(SourceCandidateError::UnsafeWorkspaceMetadata)?;
        let size = fields
            .next()
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or(SourceCandidateError::UnsafeWorkspaceMetadata)?;
        if fields.next().is_some()
            || !objects.contains(oid)
            || !matches!(kind, "blob" | "tree" | "commit")
        {
            return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
        }
        if kind == "blob"
            && (size > MAX_SOURCE_FILE_BYTES || listed_blob_sizes.get(oid) != Some(&size))
        {
            return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
        }
        snapshot_bytes = snapshot_bytes
            .checked_add(size)
            .filter(|bytes| *bytes <= MAX_WORKING_TREE_BYTES)
            .ok_or(SourceCandidateError::UnsafeWorkspaceMetadata)?;
        object_sizes.insert(oid.to_owned(), size);
        object_count = object_count
            .checked_add(1)
            .filter(|count| *count <= MAX_WORKING_TREE_ENTRIES)
            .ok_or(SourceCandidateError::UnsafeWorkspaceMetadata)?;
    }
    if object_count != objects.len() {
        return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
    }
    let working_tree_bytes = blob_references
        .iter()
        .try_fold(0u64, |sum, (oid, copies)| {
            let size = object_sizes
                .get(oid)
                .copied()
                .ok_or(SourceCandidateError::UnsafeWorkspaceMetadata)?;
            sum.checked_add(
                size.checked_mul(*copies as u64)
                    .ok_or(SourceCandidateError::UnsafeWorkspaceMetadata)?,
            )
            .filter(|bytes| *bytes <= MAX_WORKING_TREE_BYTES)
            .ok_or(SourceCandidateError::UnsafeWorkspaceMetadata)
        })?;
    if working_tree_bytes != total_listed_bytes {
        return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
    }
    Ok(PreflightTreeClosure {
        snapshot_bytes,
        object_count,
        working_tree_bytes,
        materialized_entry_count: entries,
    })
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
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
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
            "--no-textconv",
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
    let (_new_start, _new_count) = parse_hunk_span(new_part)?;
    // Anchor hunks in shared base_commit_sha coordinates so length-changing
    // earlier hunks do not shift later shared-base edits across candidates.
    if old_count > 0 {
        let start = old_start.max(1);
        let end = start.checked_add(old_count.saturating_sub(1))?;
        Some(HunkRange { start, end })
    } else {
        let start = old_start.max(1);
        let end = if old_start == 0 {
            1
        } else {
            old_start.checked_add(1)?
        };
        Some(HunkRange { start, end })
    }
}

fn parse_hunk_span(span: &str) -> Option<(u32, u32)> {
    if let Some((start, count)) = span.split_once(',') {
        Some((start.parse().ok()?, count.parse().ok()?))
    } else {
        Some((span.parse().ok()?, 1))
    }
}

struct MaterializedCheckout {
    checkout_workspace: ValidatedWorkspace,
    candidate_parent_path: PathBuf,
    candidate_parent_identity: (u64, u64),
    candidate_checkout_identity: (u64, u64),
    candidate_config: Vec<u8>,
    working_tree_digest: String,
    candidate_parent_fd: OwnedFd,
    candidate_checkout_fd: OwnedFd,
}

struct DestinationCheckoutFailure {
    error: SourceCandidateError,
    occupied_destination_path: Option<PathBuf>,
    partial_destination_path: Option<PathBuf>,
    partial_destination_identity: Option<(u64, u64)>,
}

fn preflight_candidate_destination(
    source_workspace: &ValidatedWorkspace,
    candidate_parent: &Path,
    destination_name: &str,
) -> Result<(PathBuf, OwnedFd), (SourceCandidateError, Option<PathBuf>)> {
    validate_destination_name(destination_name).map_err(|error| (error, None))?;
    if !candidate_parent.is_absolute() {
        return Err((SourceCandidateError::InvalidCandidateDestination, None));
    }
    let parent_spec = WorkspaceSpec {
        node_id: source_workspace.node_id.clone(),
        root_id: source_workspace.root_id.clone(),
        project_id: source_workspace.project_id.clone(),
        workspace: candidate_parent.to_path_buf(),
        owned_paths: Vec::new(),
        forbidden_paths: Vec::new(),
    }
    .validate()
    .map_err(|_| (SourceCandidateError::InvalidCandidateDestination, None))?;
    if parent_spec.custody != Custody::RegisteredOnly {
        return Err((SourceCandidateError::InvalidCandidateDestination, None));
    }
    let canonical_parent = parent_spec.workspace;
    if canonical_parent != candidate_parent
        || source_workspace.workspace.starts_with(&canonical_parent)
        || canonical_parent.starts_with(&source_workspace.workspace)
    {
        return Err((SourceCandidateError::InvalidCandidateDestination, None));
    }
    let parent_fd = open_directory_chain(&canonical_parent)
        .map_err(|_| (SourceCandidateError::InvalidCandidateDestination, None))?;
    ensure_private_fd(parent_fd.as_fd())
        .map_err(|_| (SourceCandidateError::InvalidCandidateDestination, None))?;
    ensure_path_points_to_fd(&canonical_parent, parent_fd.as_fd())
        .map_err(|_| (SourceCandidateError::InvalidCandidateDestination, None))?;
    match statat(&parent_fd, destination_name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(_) => {
            return Err((
                SourceCandidateError::InvalidCandidateDestination,
                Some(canonical_parent.join(destination_name)),
            ));
        }
        Err(error) if error == rustix::io::Errno::NOENT => {}
        Err(_) => return Err((SourceCandidateError::InvalidCandidateDestination, None)),
    }
    Ok((canonical_parent, parent_fd))
}

#[allow(clippy::too_many_arguments)]
fn materialize_detached_checkout(
    git: &Path,
    source_cwd: &PinnedGitDirectory<'_>,
    source_workspace: &ValidatedWorkspace,
    candidate_parent: &Path,
    destination_name: &str,
    base_commit_sha: &str,
    source_commit_sha: &str,
    source_tree_sha: &str,
    base_closure: PreflightTreeClosure,
    source_closure: PreflightTreeClosure,
    post_destination_init_hook: PostDestinationInitHook<'_>,
) -> Result<MaterializedCheckout, DestinationCheckoutFailure> {
    let (canonical_parent, parent_fd) =
        preflight_candidate_destination(source_workspace, candidate_parent, destination_name)
            .map_err(
                |(error, occupied_destination_path)| DestinationCheckoutFailure {
                    error,
                    occupied_destination_path,
                    partial_destination_path: None,
                    partial_destination_identity: None,
                },
            )?;
    ensure_admitted_capacity(parent_fd.as_fd(), base_closure, source_closure).map_err(|error| {
        DestinationCheckoutFailure {
            error,
            occupied_destination_path: None,
            partial_destination_path: None,
            partial_destination_identity: None,
        }
    })?;
    match statat(&parent_fd, destination_name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(_) => {
            return Err(DestinationCheckoutFailure {
                error: SourceCandidateError::InvalidCandidateDestination,
                occupied_destination_path: Some(canonical_parent.join(destination_name)),
                partial_destination_path: None,
                partial_destination_identity: None,
            });
        }
        Err(error) if error == rustix::io::Errno::NOENT => {}
        Err(_) => {
            return Err(DestinationCheckoutFailure {
                error: SourceCandidateError::InvalidCandidateDestination,
                occupied_destination_path: None,
                partial_destination_path: None,
                partial_destination_identity: None,
            });
        }
    }

    mkdirat(
        &parent_fd,
        destination_name,
        Mode::from_bits_truncate(0o700),
    )
    .map_err(|_| DestinationCheckoutFailure {
        error: SourceCandidateError::InvalidCandidateDestination,
        occupied_destination_path: None,
        partial_destination_path: None,
        partial_destination_identity: None,
    })?;
    let destination_path = canonical_parent.join(destination_name);
    let mut partial_identity = None;
    let post_mkdir_result: Result<MaterializedCheckout, SourceCandidateError> = (|| {
        let destination_fd = openat(
            &parent_fd,
            destination_name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|_| SourceCandidateError::InvalidCandidateDestination)?;
        partial_identity = fd_identity(destination_fd.as_fd()).ok();
        fchmod(&destination_fd, Mode::from_bits_truncate(0o700))
            .map_err(|_| SourceCandidateError::InvalidCandidateDestination)?;
        ensure_private_fd(destination_fd.as_fd())
            .map_err(|_| SourceCandidateError::InvalidCandidateDestination)?;
        ensure_same_directory_borrowed(parent_fd.as_fd(), destination_name, destination_fd.as_fd())
            .map_err(|_| SourceCandidateError::InvalidCandidateDestination)?;
        let destination_cwd = PinnedGitDirectory::with_parent(
            &destination_path,
            destination_fd.as_fd(),
            &canonical_parent,
            parent_fd.as_fd(),
            destination_name,
        )
        .map_err(|_| SourceCandidateError::InvalidCandidateDestination)?;
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
                "--object-format=sha1",
            ],
        )?;
        make_git_dir_private(&destination_cwd.access_path().join(".git"))?;
        if let Some(hook) = post_destination_init_hook {
            hook(&destination_path);
        }
        let initial_config = read_safe_config(&destination_cwd.access_path().join(".git/config"))?;
        source_cwd.check()?;
        let source_path_text = source_cwd
            .canonical_path()
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
                base_commit_sha,
                source_commit_sha,
            ],
            None,
            GIT_TRANSFER_TIMEOUT,
            MAX_GIT_OUTPUT,
        )?;
        source_cwd.check()?;
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
        fchmod(&destination_fd, Mode::from_bits_truncate(0o700))
            .map_err(|_| SourceCandidateError::InvalidCandidateDestination)?;
        ensure_private_fd(destination_fd.as_fd())
            .map_err(|_| SourceCandidateError::InvalidCandidateDestination)?;
        if git_text(
            git,
            Some(&destination_cwd),
            &["rev-parse", "--is-shallow-repository"],
        )?
        .trim()
            != "true"
        {
            return Err(SourceCandidateError::InvalidCandidateDestination);
        }
        let object_stats = git_text(git, Some(&destination_cwd), &["count-objects", "-v"])?;
        validate_object_store_stats(&object_stats, base_closure, source_closure)?;
        let candidate_config =
            validate_workspace_git_metadata(git, &destination_cwd, destination_fd.as_fd())?;
        if candidate_config != initial_config {
            return Err(SourceCandidateError::InvalidCandidateDestination);
        }
        validate_working_tree_entries(&destination_cwd)?;
        verify_clean_detached_checkout(git, &destination_cwd, source_commit_sha, source_tree_sha)?;
        validate_commit_tree_entries(git, &destination_cwd, source_commit_sha, source_tree_sha)?;
        let working_tree_digest =
            compute_working_tree_content_digest(git, &destination_cwd, source_tree_sha)?;

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
        destination_cwd
            .check()
            .map_err(|_| SourceCandidateError::InvalidCandidateDestination)?;
        let candidate_parent_identity = fd_identity(parent_fd.as_fd())
            .map_err(|_| SourceCandidateError::InvalidCandidateDestination)?;
        let candidate_checkout_identity = fd_identity(destination_fd.as_fd())
            .map_err(|_| SourceCandidateError::InvalidCandidateDestination)?;
        Ok(MaterializedCheckout {
            checkout_workspace: validated,
            candidate_parent_path: canonical_parent,
            candidate_parent_identity,
            candidate_checkout_identity,
            candidate_config,
            working_tree_digest,
            candidate_parent_fd: parent_fd,
            candidate_checkout_fd: destination_fd,
        })
    })();

    post_mkdir_result.map_err(|error| DestinationCheckoutFailure {
        error,
        occupied_destination_path: None,
        partial_destination_path: Some(destination_path),
        partial_destination_identity: partial_identity,
    })
}

fn ensure_admitted_capacity(
    parent_fd: BorrowedFd<'_>,
    base_closure: PreflightTreeClosure,
    source_closure: PreflightTreeClosure,
) -> Result<(), SourceCandidateError> {
    let usage =
        fstatvfs(parent_fd).map_err(|_| SourceCandidateError::InvalidCandidateDestination)?;
    let available_bytes = usage
        .f_bavail
        .checked_mul(usage.f_frsize)
        .ok_or(SourceCandidateError::InvalidCandidateDestination)?;
    let available_inodes = usage.f_favail;
    let combined_objects = (base_closure.object_count as u64)
        .checked_add(source_closure.object_count as u64)
        .ok_or(SourceCandidateError::UnsafeWorkspaceMetadata)?;
    let combined_snapshot_bytes = base_closure
        .snapshot_bytes
        .checked_add(source_closure.snapshot_bytes)
        .ok_or(SourceCandidateError::UnsafeWorkspaceMetadata)?;
    let admitted_bytes = combined_snapshot_bytes
        .checked_add(source_closure.working_tree_bytes)
        .and_then(|bytes| bytes.checked_add(combined_objects.checked_mul(DISK_BYTES_PER_OBJECT)?))
        .and_then(|bytes| {
            bytes.checked_add(
                (source_closure.materialized_entry_count as u64)
                    .checked_mul(DISK_BYTES_PER_FILE)?,
            )
        })
        .and_then(|bytes| bytes.checked_add(DISK_RESERVE_BYTES))
        .ok_or(SourceCandidateError::UnsafeWorkspaceMetadata)?;
    let admitted_inodes = combined_objects
        .checked_add(source_closure.materialized_entry_count as u64)
        .and_then(|count| count.checked_add(DISK_INODE_RESERVE))
        .ok_or(SourceCandidateError::UnsafeWorkspaceMetadata)?;
    if available_bytes < admitted_bytes || available_inodes < admitted_inodes {
        return Err(SourceCandidateError::InvalidCandidateDestination);
    }
    Ok(())
}

fn validate_object_store_stats(
    stats: &str,
    base_closure: PreflightTreeClosure,
    source_closure: PreflightTreeClosure,
) -> Result<(), SourceCandidateError> {
    let mut map = BTreeMap::new();
    for (key, value) in stats
        .lines()
        .filter_map(|line| line.split_once(':'))
        .map(|(key, value)| (key.trim(), value.trim()))
    {
        map.insert(
            key,
            value
                .parse::<u64>()
                .map_err(|_| SourceCandidateError::UnsafeWorkspaceMetadata)?,
        );
    }
    let objects = map
        .get("count")
        .copied()
        .unwrap_or(0)
        .checked_add(map.get("in-pack").copied().unwrap_or(0))
        .ok_or(SourceCandidateError::UnsafeWorkspaceMetadata)?;
    let loose_kib = map.get("size").copied().unwrap_or(0);
    let packed_kib = map.get("size-pack").copied().unwrap_or(0);
    let stored_bytes = loose_kib
        .checked_add(packed_kib)
        .and_then(|kib| kib.checked_mul(1024))
        .ok_or(SourceCandidateError::UnsafeWorkspaceMetadata)?;
    let expected_objects = (base_closure.object_count as u64)
        .checked_add(source_closure.object_count as u64)
        .ok_or(SourceCandidateError::UnsafeWorkspaceMetadata)?;
    let expected_bytes = base_closure
        .snapshot_bytes
        .checked_add(source_closure.snapshot_bytes)
        .ok_or(SourceCandidateError::UnsafeWorkspaceMetadata)?;
    if objects > expected_objects
        || stored_bytes > expected_bytes.saturating_add(2 * 1024 * 1024)
        || map.get("garbage").copied().unwrap_or(0) != 0
        || map.get("size-garbage").copied().unwrap_or(0) != 0
    {
        return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
    }
    Ok(())
}

fn verify_clean_detached_checkout(
    git: &Path,
    checkout_cwd: &PinnedGitDirectory<'_>,
    source_commit_sha: &str,
    source_tree_sha: &str,
) -> Result<(), SourceCandidateError> {
    let head = git_text(git, Some(checkout_cwd), &["rev-parse", "--verify", "HEAD"])?
        .trim()
        .to_owned();
    let tree = git_text(
        git,
        Some(checkout_cwd),
        &["rev-parse", "--verify", "HEAD^{tree}"],
    )?
    .trim()
    .to_owned();
    if head != source_commit_sha || tree != source_tree_sha {
        return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
    }
    let branch_status = git_text(
        git,
        Some(checkout_cwd),
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
            "--ignored=matching",
            "--ignore-submodules=none",
        ],
    )?;
    if branch_status
        .lines()
        .any(|line| !line.is_empty() && !line.starts_with("# "))
        || !branch_status
            .lines()
            .any(|line| line.strip_prefix("# branch.oid ") == Some(source_commit_sha))
    {
        return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
    }
    Ok(())
}

fn compute_working_tree_content_digest(
    git: &Path,
    checkout_cwd: &PinnedGitDirectory<'_>,
    tree_oid: &str,
) -> Result<String, SourceCandidateError> {
    checkout_cwd.check()?;
    let listing = run_git_with_options(
        git,
        Some(checkout_cwd),
        &["ls-tree", "-r", "-z", "--full-tree", tree_oid],
        None,
        GIT_TIMEOUT,
        MAX_TREE_LISTING,
    )?
    .stdout;
    let access_root = checkout_cwd.access_path();
    let mut entries = 0usize;
    let mut total_bytes = 0u64;
    let mut records = Vec::<(String, String, String, String)>::new();
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
            || kind != "blob"
            || !matches!(mode, "100644" | "100755")
        {
            return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
        }
        let remaining_cumulative = MAX_WORKING_TREE_BYTES
            .checked_sub(total_bytes)
            .ok_or(SourceCandidateError::UnsafeWorkspaceMetadata)?;
        let file_limit = MAX_SOURCE_FILE_BYTES.min(remaining_cumulative);
        let file_path = access_root.join(path_text);
        let bytes = read_bounded_regular_file(&file_path, file_limit, 0o002)?;
        total_bytes = total_bytes
            .checked_add(bytes.len() as u64)
            .filter(|count| *count <= MAX_WORKING_TREE_BYTES)
            .ok_or(SourceCandidateError::UnsafeWorkspaceMetadata)?;
        let expected_blob = run_git_with_options(
            git,
            Some(checkout_cwd),
            &["cat-file", "blob", oid],
            None,
            GIT_TIMEOUT,
            usize::try_from(file_limit)
                .unwrap_or(MAX_GIT_OUTPUT)
                .min(MAX_GIT_OUTPUT),
        )?
        .stdout;
        if bytes != expected_blob {
            return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
        }
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let file_sha256 = format!("sha256:{:x}", hasher.finalize());
        records.push((
            path_text.to_owned(),
            mode.to_owned(),
            oid.to_owned(),
            file_sha256,
        ));
    }
    checkout_cwd.check()?;
    content_digest(&("working-tree-content-v1", tree_oid, &records))
        .map_err(|_| SourceCandidateError::Digest)
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

fn validate_working_tree_entries(
    workspace_cwd: &PinnedGitDirectory<'_>,
) -> Result<(), SourceCandidateError> {
    workspace_cwd.check()?;
    let workspace_root = workspace_cwd.access_path();
    if !same_directory_as_fd(&workspace_root, workspace_cwd.descriptor())
        || !same_directory_as_fd(workspace_cwd.canonical_path(), workspace_cwd.descriptor())
    {
        return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
    }
    let uid = rustix::process::geteuid().as_raw();
    let mut stack = vec![workspace_root.clone()];
    let mut entries = 0usize;
    let mut total_bytes = 0u64;
    while let Some(directory) = stack.pop() {
        workspace_cwd.check()?;
        let dir_meta = if directory == workspace_root {
            fs::metadata(&directory).map_err(|_| SourceCandidateError::UnsafeWorkspaceMetadata)?
        } else {
            fs::symlink_metadata(&directory)
                .map_err(|_| SourceCandidateError::UnsafeWorkspaceMetadata)?
        };
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
            if stack.len() >= MAX_WORKING_TREE_ENTRIES {
                return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
            }
            let path = entry.path();
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
                let (_file, fd_meta) =
                    open_verified_regular_file(&path, MAX_SOURCE_FILE_BYTES, 0o002)?;
                total_bytes = total_bytes
                    .checked_add(fd_meta.len())
                    .filter(|bytes| *bytes <= MAX_WORKING_TREE_BYTES)
                    .ok_or(SourceCandidateError::UnsafeWorkspaceMetadata)?;
            } else {
                return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
            }
        }
    }
    workspace_cwd.check()
}

fn validate_workspace_git_metadata(
    git: &Path,
    workspace_cwd: &PinnedGitDirectory<'_>,
    workspace_fd: BorrowedFd<'_>,
) -> Result<Vec<u8>, SourceCandidateError> {
    workspace_cwd.check()?;
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
    validate_git_dir_tree(&git_dir)?;
    let config_path = git_dir.join("config");
    let config = read_safe_config(&config_path)?;
    for forbidden in [
        Path::new("objects/info/alternates"),
        Path::new("objects/info/http-alternates"),
        Path::new("info/attributes"),
        Path::new("info/grafts"),
        Path::new("config.worktree"),
        Path::new("modules"),
        Path::new("worktrees"),
    ] {
        if path_entry_exists(&git_dir.join(forbidden))? {
            return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
        }
    }
    let hooks_dir = git_dir.join("hooks");
    if path_entry_exists(&hooks_dir)? {
        let hooks_meta = fs::symlink_metadata(&hooks_dir)
            .map_err(|_| SourceCandidateError::UnsafeWorkspaceMetadata)?;
        if hooks_meta.file_type().is_symlink()
            || !hooks_meta.is_dir()
            || hooks_meta.uid() != uid
            || hooks_meta.mode() & 0o002 != 0
        {
            return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
        }
        let mut hook_entries = 0usize;
        for hook in
            fs::read_dir(&hooks_dir).map_err(|_| SourceCandidateError::UnsafeWorkspaceMetadata)?
        {
            hook_entries = hook_entries
                .checked_add(1)
                .filter(|count| *count <= MAX_GIT_HOOK_ENTRIES)
                .ok_or(SourceCandidateError::UnsafeWorkspaceMetadata)?;
            let hook = hook.map_err(|_| SourceCandidateError::UnsafeWorkspaceMetadata)?;
            let hook_meta = fs::symlink_metadata(hook.path())
                .map_err(|_| SourceCandidateError::UnsafeWorkspaceMetadata)?;
            let hook_name = hook.file_name();
            let hook_name_str = hook_name.to_string_lossy();
            if hook_meta.file_type().is_symlink()
                || !hook_meta.is_file()
                || hook_meta.nlink() != 1
                || hook_meta.uid() != uid
                || hook_meta.mode() & 0o002 != 0
                || (!hook_name_str.ends_with(".sample") && hook_meta.mode() & 0o111 != 0)
            {
                return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
            }
        }
    }
    let index_flags = run_git_with_options(
        git,
        Some(workspace_cwd),
        &["ls-files", "-v", "-z"],
        None,
        GIT_TIMEOUT,
        MAX_TREE_LISTING,
    )?
    .stdout;
    for entry in index_flags
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
    {
        if matches!(entry.first().copied(), Some(b'h' | b'S' | b's')) {
            return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
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
    let config_after = read_safe_config(&config_path)?;
    if config_after != config {
        return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
    }
    workspace_cwd.check()?;
    Ok(config)
}

fn open_verified_regular_file(
    path: &Path,
    max_bytes: u64,
    disallowed_mode_mask: u32,
) -> Result<(fs::File, fs::Metadata), SourceCandidateError> {
    let path_meta =
        fs::symlink_metadata(path).map_err(|_| SourceCandidateError::UnsafeWorkspaceMetadata)?;
    let uid = rustix::process::geteuid().as_raw();
    let path_type = path_meta.file_type();
    if path_type.is_symlink()
        || !path_meta.is_file()
        || path_meta.uid() != uid
        || path_meta.nlink() != 1
        || path_meta.mode() & disallowed_mode_mask != 0
        || path_meta.len() > max_bytes
    {
        return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
    }
    let fd = open(
        path,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
        Mode::empty(),
    )
    .map_err(|_| SourceCandidateError::UnsafeWorkspaceMetadata)?;
    let file = fs::File::from(fd);
    let fd_meta = file
        .metadata()
        .map_err(|_| SourceCandidateError::UnsafeWorkspaceMetadata)?;
    let fd_type = fd_meta.file_type();
    if fd_type.is_symlink()
        || !fd_meta.is_file()
        || fd_meta.uid() != uid
        || fd_meta.nlink() != 1
        || fd_meta.mode() & disallowed_mode_mask != 0
        || fd_meta.dev() != path_meta.dev()
        || fd_meta.ino() != path_meta.ino()
        || fd_meta.len() > max_bytes
    {
        return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
    }
    Ok((file, fd_meta))
}

fn read_bounded_regular_file(
    path: &Path,
    max_bytes: u64,
    disallowed_mode_mask: u32,
) -> Result<Vec<u8>, SourceCandidateError> {
    let (mut file, fd_meta) = open_verified_regular_file(path, max_bytes, disallowed_mode_mask)?;
    let uid = rustix::process::geteuid().as_raw();
    let initial_capacity = usize::try_from(fd_meta.len().min(max_bytes).min(8192)).unwrap_or(0);
    let mut bytes = Vec::with_capacity(initial_capacity);
    (&mut file)
        .take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| SourceCandidateError::UnsafeWorkspaceMetadata)?;
    if bytes.len() as u64 > max_bytes || fd_meta.len() > max_bytes {
        return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
    }
    let fd_after = file
        .metadata()
        .map_err(|_| SourceCandidateError::UnsafeWorkspaceMetadata)?;
    let path_after =
        fs::symlink_metadata(path).map_err(|_| SourceCandidateError::UnsafeWorkspaceMetadata)?;
    if fd_after.file_type().is_symlink()
        || !fd_after.is_file()
        || fd_after.uid() != uid
        || fd_after.nlink() != 1
        || fd_after.mode() & disallowed_mode_mask != 0
        || fd_after.dev() != fd_meta.dev()
        || fd_after.ino() != fd_meta.ino()
        || fd_after.len() != bytes.len() as u64
        || fd_after.len() > max_bytes
        || path_after.file_type().is_symlink()
        || !path_after.is_file()
        || path_after.uid() != uid
        || path_after.nlink() != 1
        || path_after.mode() & disallowed_mode_mask != 0
        || path_after.dev() != fd_meta.dev()
        || path_after.ino() != fd_meta.ino()
        || path_after.len() != bytes.len() as u64
    {
        return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
    }
    Ok(bytes)
}

fn read_safe_config(path: &Path) -> Result<Vec<u8>, SourceCandidateError> {
    let bytes = read_bounded_regular_file(path, MAX_GIT_CONFIG_BYTES, 0o022)?;
    let lower = String::from_utf8_lossy(&bytes).to_ascii_lowercase();
    for forbidden in [
        "[include",
        "include.",
        "includeif.",
        "[filter",
        "filter.",
        "[diff",
        "diff.",
        "[merge",
        "merge.",
        "textconv",
        "extdiff",
        "external",
        "driver",
        "command",
        "clean",
        "smudge",
        "fsmonitor",
        "hookspath",
        "attributesfile",
        "excludesfile",
        "promisor",
        "partialclone",
        "protocol.",
        "insteadof",
        "credential",
        "sshcommand",
        "gitproxy",
        "pager",
        "editor",
        "askpass",
        "program",
        "uploadpack",
        "receivepack",
        "alias.",
        "[alias",
        "worktree",
        "gitdir",
        "alternate",
        "ext::",
    ] {
        if lower.contains(forbidden) {
            return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
        }
    }
    Ok(bytes)
}

fn validate_git_dir_tree(root: &Path) -> Result<(), SourceCandidateError> {
    traverse_git_dir_bounded(root, false)
}

fn make_git_dir_private(root: &Path) -> Result<(), SourceCandidateError> {
    traverse_git_dir_bounded(root, true)
}

fn traverse_git_dir_bounded(
    root: &Path,
    normalize_modes: bool,
) -> Result<(), SourceCandidateError> {
    traverse_git_dir_bounded_with_limits(
        root,
        normalize_modes,
        MAX_GIT_DIR_DEPTH,
        MAX_GIT_DIR_ENTRIES,
        MAX_GIT_DIR_BYTES,
    )
}

fn traverse_git_dir_bounded_with_limits(
    root: &Path,
    normalize_modes: bool,
    max_depth: usize,
    max_entries: usize,
    max_bytes: u64,
) -> Result<(), SourceCandidateError> {
    if max_entries == 0 {
        return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
    }
    let uid = rustix::process::geteuid().as_raw();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    let mut entries = 1usize;
    let mut total_bytes = 0u64;
    while let Some((path, depth)) = stack.pop() {
        if depth > max_depth {
            return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
        }
        let metadata = fs::symlink_metadata(&path)
            .map_err(|_| SourceCandidateError::UnsafeWorkspaceMetadata)?;
        let file_type = metadata.file_type();
        if file_type.is_symlink()
            || file_type.is_fifo()
            || file_type.is_socket()
            || file_type.is_block_device()
            || file_type.is_char_device()
            || metadata.uid() != uid
        {
            return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
        }
        if metadata.is_dir() {
            let dir_fd = open(
                &path,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|_| SourceCandidateError::UnsafeWorkspaceMetadata)?;
            let dir_file = fs::File::from(dir_fd);
            let fd_meta = dir_file
                .metadata()
                .map_err(|_| SourceCandidateError::UnsafeWorkspaceMetadata)?;
            if fd_meta.file_type().is_symlink()
                || !fd_meta.is_dir()
                || fd_meta.uid() != uid
                || fd_meta.dev() != metadata.dev()
                || fd_meta.ino() != metadata.ino()
            {
                return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
            }
            if normalize_modes {
                fchmod(&dir_file, Mode::from_bits_truncate(0o700))
                    .map_err(|_| SourceCandidateError::UnsafeWorkspaceMetadata)?;
            } else if fd_meta.mode() & 0o002 != 0 {
                return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
            }
            let next_depth = depth
                .checked_add(1)
                .filter(|d| *d <= max_depth)
                .ok_or(SourceCandidateError::UnsafeWorkspaceMetadata)?;
            for entry in
                fs::read_dir(&path).map_err(|_| SourceCandidateError::UnsafeWorkspaceMetadata)?
            {
                entries = entries
                    .checked_add(1)
                    .filter(|count| *count <= max_entries)
                    .ok_or(SourceCandidateError::UnsafeWorkspaceMetadata)?;
                if stack.len() >= max_entries {
                    return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
                }
                let entry = entry.map_err(|_| SourceCandidateError::UnsafeWorkspaceMetadata)?;
                stack.push((entry.path(), next_depth));
            }
        } else if metadata.is_file() {
            if metadata.nlink() != 1 {
                return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
            }
            let disallowed_mask = if normalize_modes { 0 } else { 0o002 };
            let (file, fd_meta) = open_verified_regular_file(&path, max_bytes, disallowed_mask)?;
            total_bytes = total_bytes
                .checked_add(fd_meta.len())
                .filter(|bytes| *bytes <= max_bytes)
                .ok_or(SourceCandidateError::UnsafeWorkspaceMetadata)?;
            if normalize_modes {
                fchmod(&file, Mode::from_bits_truncate(0o600))
                    .map_err(|_| SourceCandidateError::UnsafeWorkspaceMetadata)?;
            }
        } else {
            return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
        }
    }
    Ok(())
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
    canonical: &'a Path,
    descriptor: BorrowedFd<'a>,
    parent: Option<(&'a Path, BorrowedFd<'a>, &'a str)>,
}

impl<'a> PinnedGitDirectory<'a> {
    fn from_prepared(prepared: &'a PreparedWorkspace) -> Result<Self, SourceCandidateError> {
        let canonical = prepared.workspace().workspace.as_path();
        let parent_canonical = canonical
            .parent()
            .ok_or(SourceCandidateError::UnsafeWorkspaceMetadata)?;
        let entry_name = canonical
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(SourceCandidateError::UnsafeWorkspaceMetadata)?;
        Self::with_parent(
            canonical,
            prepared.workspace_dir_fd(),
            parent_canonical,
            prepared.managed_parent_fd(),
            entry_name,
        )
    }

    fn with_parent(
        canonical: &'a Path,
        descriptor: BorrowedFd<'a>,
        parent_canonical: &'a Path,
        parent_descriptor: BorrowedFd<'a>,
        entry_name: &'a str,
    ) -> Result<Self, SourceCandidateError> {
        let pinned = Self {
            canonical,
            descriptor,
            parent: Some((parent_canonical, parent_descriptor, entry_name)),
        };
        pinned.check()?;
        Ok(pinned)
    }

    fn descriptor(&self) -> BorrowedFd<'a> {
        self.descriptor
    }

    fn canonical_path(&self) -> &'a Path {
        self.canonical
    }

    fn access_path(&self) -> PathBuf {
        #[cfg(target_os = "linux")]
        {
            use std::os::fd::AsRawFd;
            PathBuf::from(format!("/proc/self/fd/{}", self.descriptor.as_raw_fd()))
        }
        #[cfg(not(target_os = "linux"))]
        {
            self.canonical.to_owned()
        }
    }

    fn check(&self) -> Result<(), SourceCandidateError> {
        ensure_private_fd(self.descriptor)?;
        ensure_path_points_to_fd(self.canonical, self.descriptor)?;
        if let Some((parent_canonical, parent_descriptor, entry_name)) = self.parent {
            ensure_private_fd(parent_descriptor)?;
            ensure_path_points_to_fd(parent_canonical, parent_descriptor)?;
            ensure_same_directory_borrowed(parent_descriptor, entry_name, self.descriptor)
                .map_err(|_| SourceCandidateError::UnsafeWorkspaceMetadata)?;
        }
        Ok(())
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
        ensure_safe_ancestor_fd(current.as_fd())?;
        match component {
            Component::RootDir => {}
            Component::Normal(name) => {
                current = openat(&current, name, flags, Mode::empty())
                    .map_err(|_| SourceCandidateError::UnsafeWorkspaceMetadata)?;
            }
            _ => return Err(SourceCandidateError::UnsafeWorkspaceMetadata),
        }
    }
    ensure_safe_ancestor_fd(current.as_fd())?;
    Ok(current)
}

fn ensure_safe_ancestor_fd(fd: BorrowedFd<'_>) -> Result<(), SourceCandidateError> {
    let stat = fstat(fd).map_err(|_| SourceCandidateError::UnsafeWorkspaceMetadata)?;
    let uid = rustix::process::geteuid().as_raw();
    let mode = stat.st_mode & 0o7777;
    if (stat.st_uid != 0 && stat.st_uid != uid) || (mode & 0o022 != 0 && mode & 0o1000 == 0) {
        return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
    }
    Ok(())
}

fn ensure_private_fd(fd: BorrowedFd<'_>) -> Result<(), SourceCandidateError> {
    let stat = fstat(fd).map_err(|_| SourceCandidateError::UnsafeWorkspaceMetadata)?;
    let is_directory =
        rustix::fs::FileType::from_raw_mode(stat.st_mode) == rustix::fs::FileType::Directory;
    let uid = rustix::process::geteuid().as_raw();
    if !is_directory || stat.st_uid != uid || stat.st_mode & 0o7777 != 0o700 {
        return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
    }
    Ok(())
}

fn ensure_path_points_to_fd(path: &Path, fd: BorrowedFd<'_>) -> Result<(), SourceCandidateError> {
    let opened = open_directory_chain(path)?;
    let left = fstat(&opened).map_err(|_| SourceCandidateError::UnsafeWorkspaceMetadata)?;
    let right = fstat(fd).map_err(|_| SourceCandidateError::UnsafeWorkspaceMetadata)?;
    if left.st_dev != right.st_dev || left.st_ino != right.st_ino {
        return Err(SourceCandidateError::UnsafeWorkspaceMetadata);
    }
    Ok(())
}

fn ensure_same_directory_borrowed(
    parent_fd: BorrowedFd<'_>,
    name: &str,
    child_fd: BorrowedFd<'_>,
) -> Result<(), SourceCandidateError> {
    let child_stat =
        fstat(child_fd).map_err(|_| SourceCandidateError::InvalidCandidateDestination)?;
    let named_stat = statat(parent_fd, name, AtFlags::SYMLINK_NOFOLLOW)
        .map_err(|_| SourceCandidateError::InvalidCandidateDestination)?;
    if child_stat.st_dev != named_stat.st_dev
        || child_stat.st_ino != named_stat.st_ino
        || child_stat.st_uid != named_stat.st_uid
        || rustix::fs::FileType::from_raw_mode(named_stat.st_mode)
            != rustix::fs::FileType::Directory
    {
        return Err(SourceCandidateError::InvalidCandidateDestination);
    }
    Ok(())
}

fn fd_identity(directory: BorrowedFd<'_>) -> Result<(u64, u64), SourceCandidateError> {
    let fd_copy = directory
        .try_clone_to_owned()
        .map_err(|_| SourceCandidateError::UnsafeWorkspaceMetadata)?;
    let fd_metadata = fs::File::from(fd_copy)
        .metadata()
        .map_err(|_| SourceCandidateError::UnsafeWorkspaceMetadata)?;
    Ok((fd_metadata.dev(), fd_metadata.ino()))
}

fn same_directory_as_fd(path: &Path, directory: BorrowedFd<'_>) -> bool {
    let Ok(path_metadata) = fs::metadata(path) else {
        return false;
    };
    let Ok((fd_dev, fd_ino)) = fd_identity(directory) else {
        return false;
    };
    path_metadata.dev() == fd_dev && path_metadata.ino() == fd_ino
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
        .env("XDG_CONFIG_HOME", "/nonexistent")
        .env("PATH", "/usr/bin:/bin")
        .env("GIT_ATTR_NOSYSTEM", "1")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "/bin/false")
        .env("SSH_ASKPASS", "/bin/false")
        .env("GIT_SSH_COMMAND", "/bin/false")
        .env("GIT_EDITOR", "/bin/false")
        .env("GIT_PAGER", "cat")
        .env("PAGER", "cat")
        .env("GIT_EXTERNAL_DIFF", "/bin/false")
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

#[cfg(test)]
std::thread_local! {
    static TEST_GIT_INVOCATIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn reset_test_git_invocations() {
    TEST_GIT_INVOCATIONS.with(|count| count.set(0));
}

#[cfg(test)]
fn test_git_invocations() -> usize {
    TEST_GIT_INVOCATIONS.with(std::cell::Cell::get)
}

fn run_git_with_options(
    git: &Path,
    cwd: Option<&PinnedGitDirectory<'_>>,
    args: &[&str],
    input: Option<&[u8]>,
    timeout: Duration,
    output_limit: usize,
) -> Result<Output, SourceCandidateError> {
    #[cfg(test)]
    TEST_GIT_INVOCATIONS.with(|count| count.set(count.get().saturating_add(1)));
    if let Some(directory) = cwd {
        directory.check()?;
    }
    let command_cwd = cwd.map(PinnedGitDirectory::access_path);
    let mut command = base_git_command(git, command_cwd.as_deref());
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
    let mut stdin = if input.is_some() {
        match child.stdin.take() {
            Some(pipe) => Some(pipe),
            None => {
                return abort_git_child(
                    &mut child,
                    SourceCandidateError::GitCommand("missing stdin pipe".into()),
                );
            }
        }
    } else {
        None
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
    if let Some(pipe) = stdin.as_ref()
        && set_nonblocking(pipe).is_err()
    {
        return abort_git_child(
            &mut child,
            SourceCandidateError::GitCommand("failed to configure nonblocking stdin".into()),
        );
    }
    let input = input.unwrap_or_default();
    let mut input_offset = 0usize;
    if input_offset == input.len() {
        stdin.take();
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
        if status.is_some() && stdin.is_some() {
            return Err(SourceCandidateError::GitCommand(
                "process exited before accepting all input".into(),
            ));
        }
        if let Err(error) = write_pipe(&mut stdin, input, &mut input_offset) {
            return abort_git_child(
                &mut child,
                SourceCandidateError::GitCommand(error.to_string()),
            );
        }
        if stdin.is_none() && status.is_some() && stdout.is_none() && stderr.is_none() {
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

fn write_pipe<W: Write>(pipe: &mut Option<W>, input: &[u8], offset: &mut usize) -> io::Result<()> {
    let Some(writer) = pipe.as_mut() else {
        return Ok(());
    };
    if *offset >= input.len() {
        pipe.take();
        return Ok(());
    }
    match writer.write(&input[*offset..]) {
        Ok(0) => Err(io::ErrorKind::WriteZero.into()),
        Ok(count) => {
            *offset += count;
            if *offset == input.len() {
                pipe.take();
            }
            Ok(())
        }
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
            ) =>
        {
            Ok(())
        }
        Err(error) => Err(error),
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
    use std::{
        os::unix::fs::PermissionsExt,
        sync::atomic::{AtomicU64, Ordering},
    };

    static FIXTURE_SEQ: AtomicU64 = AtomicU64::new(1);
    static SYNTHETIC_CLOSURE_SEQ: AtomicU64 = AtomicU64::new(1000);

    /// Explicitly synthetic in-module unit test fixture proof implementing
    /// [`ClosedAdmissionProof`] without claiming real Linux systemd unit closure.
    #[derive(Clone, Debug)]
    pub(super) struct FixtureClosedAdmission {
        synthetic_handle_id: u64,
        identity: RecoveryIdentity,
        observation: EnforcementObservation,
        exit_code: Option<i32>,
        success: bool,
        closure_digest: String,
    }

    impl sealed::Sealed for FixtureClosedAdmission {
        fn into_retained_proof(self) -> sealed::RetainedClosureProof {
            sealed::RetainedClosureProof(sealed::RetainedClosureProofInner::SyntheticTestFixture(
                self,
            ))
        }
    }

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
            synthetic_handle_id: SYNTHETIC_CLOSURE_SEQ.fetch_add(1, Ordering::Relaxed),
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

    #[test]
    fn rejects_path_replacement_preserving_ids_and_base_commit_with_different_inode() {
        // Case A: Path replaced before prove_closed_workspace handoff
        let mut fixture = setup_fixture(vec!["src"], vec!["src/secret"]);
        let original_prepared = fixture.take_prepared();
        let original_ino = fd_identity(original_prepared.workspace_dir_fd())
            .expect("original workspace identity")
            .1;
        let displaced = fixture.root.join("managed-workspaces/attempt-1-displaced");
        fs::rename(&fixture.workspace_path, &displaced).expect("rename original checkout away");

        let replacement = prepare_workspace(WorkspacePreparation {
            source: fixture.root.join("origin-repo"),
            managed_parent: fixture.root.join("managed-workspaces"),
            destination_name: "attempt-1-work".into(),
            pinned_commit: original_prepared.head_oid().to_owned(),
            node_id: NodeRef::new("node-alpha").expect("node_id"),
            root_id: original_prepared.root_id().clone(),
            project_id: original_prepared.project_id().clone(),
            task_id: original_prepared.task_id().clone(),
            attempt_id: original_prepared.attempt_id().clone(),
            owned_paths: original_prepared.owned_paths().to_vec(),
            forbidden_paths: original_prepared.forbidden_paths().to_vec(),
        })
        .expect("prepare replacement workspace at same path with identical IDs and base commit");
        let replacement_ino = fd_identity(replacement.workspace_dir_fd())
            .expect("replacement workspace identity")
            .1;
        assert_ne!(original_ino, replacement_ino);
        fs::write(
            fixture.workspace_path.join("src/lib.rs"),
            b"pub fn answer() -> u32 {\n    42\n}\n",
        )
        .expect("modify replacement checkout");

        assert!(original_prepared.validate_current_custody().is_err());
        assert_eq!(
            prove_closed_workspace(original_prepared, fixture.closure.clone()).unwrap_err(),
            SourceCandidateError::UnsafeWorkspaceMetadata
        );

        // Case B: Path replaced after prove_closed_workspace but before materialize_source_candidate
        let mut fixture = setup_fixture(vec!["src"], vec!["src/secret"]);
        let closed = prove_closed_workspace(fixture.take_prepared(), fixture.closure.clone())
            .expect("prove closed before path swap");
        let displaced = fixture.root.join("managed-workspaces/attempt-1-displaced");
        fs::rename(&fixture.workspace_path, &displaced).expect("rename closed checkout away");
        let _replacement = prepare_workspace(WorkspacePreparation {
            source: fixture.root.join("origin-repo"),
            managed_parent: fixture.root.join("managed-workspaces"),
            destination_name: "attempt-1-work".into(),
            pinned_commit: closed.prepared().head_oid().to_owned(),
            node_id: NodeRef::new("node-alpha").expect("node_id"),
            root_id: closed.prepared().root_id().clone(),
            project_id: closed.prepared().project_id().clone(),
            task_id: closed.prepared().task_id().clone(),
            attempt_id: closed.prepared().attempt_id().clone(),
            owned_paths: closed.prepared().owned_paths().to_vec(),
            forbidden_paths: closed.prepared().forbidden_paths().to_vec(),
        })
        .expect("prepare replacement workspace after closure handoff");
        fs::write(
            fixture.workspace_path.join("src/lib.rs"),
            b"pub fn answer() -> u32 {\n    42\n}\n",
        )
        .expect("modify replacement checkout");

        assert_eq!(
            closed.revalidate_custody().unwrap_err(),
            SourceCandidateError::UnsafeWorkspaceMetadata
        );
        assert_eq!(
            materialize_source_candidate(closed, &fixture.candidate_parent, "candidate-swapped")
                .unwrap_err(),
            SourceCandidateError::UnsafeWorkspaceMetadata
        );
    }

    #[test]
    fn rejects_textconv_diff_driver_without_executing_marker_command_and_reads_raw_hunks() {
        let mut fixture = setup_fixture(vec!["src"], vec!["src/secret"]);
        let marker_path = fixture.root.join("textconv-marker-ran.txt");
        let base_commit = fixture.prepared.as_ref().unwrap().head_oid().to_owned();

        fs::write(
            fixture.workspace_path.join("src/lib.rs"),
            b"pub fn answer() -> u32 {\n    42\n}\n",
        )
        .expect("edit src/lib.rs");
        fs::write(
            fixture.workspace_path.join("src/.gitattributes"),
            b"*.rs diff=marker\n",
        )
        .expect("write .gitattributes selecting marker diff driver");
        run_fixture_git(
            &fixture.workspace_path,
            &["add", "src/lib.rs", "src/.gitattributes"],
        );
        run_fixture_git(
            &fixture.workspace_path,
            &["commit", "--quiet", "-m", "commit with gitattributes"],
        );
        let head_commit = run_fixture_git(&fixture.workspace_path, &["rev-parse", "HEAD"]);

        let config_path = fixture.workspace_path.join(".git/config");
        let mut config_bytes = fs::read(&config_path).expect("read .git/config");
        let textconv_stanza = format!(
            "\n[diff \"marker\"]\n\ttextconv = /bin/sh -c 'echo invoked > \"{}\"; printf \"transformed\\\\n\"' --\n",
            marker_path.display()
        );
        config_bytes.extend_from_slice(textconv_stanza.as_bytes());
        fs::write(&config_path, config_bytes).expect("inject diff.marker.textconv config");
        make_git_dir_private(&fixture.workspace_path.join(".git")).expect("private .git");

        // Direct hunk extraction must pass --no-textconv, never execute the marker command,
        // and read the untransformed source hunks (line 2 of src/lib.rs).
        let git = trusted_git().expect("trusted git");
        let prepared_ref = fixture.prepared.as_ref().expect("prepared ref");
        let pinned_cwd = PinnedGitDirectory::from_prepared(prepared_ref).expect("pinned cwd");
        let hunks = extract_file_hunks(&git, &pinned_cwd, &base_commit, &head_commit, "src/lib.rs")
            .expect("extract_file_hunks succeeds without running textconv");
        assert_eq!(hunks, Some(vec![HunkRange { start: 2, end: 2 }]));
        assert!(
            !marker_path.exists(),
            "textconv marker command must never execute during hunk extraction"
        );

        // Full materialization must also reject the textconv config and never run the marker command.
        let closed = prove_closed_workspace(fixture.take_prepared(), fixture.closure.clone())
            .expect("prove closed");
        let err = materialize_source_candidate(
            closed,
            &fixture.candidate_parent,
            "candidate-textconv-rejected",
        )
        .unwrap_err();
        assert_eq!(err, SourceCandidateError::UnsafeWorkspaceMetadata);
        assert!(
            !marker_path.exists(),
            "textconv marker command must never execute during candidate materialization"
        );
    }

    #[test]
    fn decision_evidence_revalidates_actual_checkout_content_and_rejects_post_materialization_edits()
     {
        let mut fixture = setup_fixture(vec!["src"], vec!["src/secret"]);
        fs::write(
            fixture.workspace_path.join("src/lib.rs"),
            b"pub fn answer() -> u32 {\n    42\n}\n",
        )
        .expect("edit src/lib.rs");

        let closed = prove_closed_workspace(fixture.take_prepared(), fixture.closure.clone())
            .expect("prove closed");
        let candidate =
            materialize_source_candidate(closed, &fixture.candidate_parent, "candidate-tamper")
                .expect("materialize candidate");

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
            "check-run-before-tamper",
        )
        .expect("execute_checks before tamper");

        // Tamper with the owner-writable mode-0700 candidate checkout using identical byte length.
        fs::write(
            candidate.checkout_path().join("src/lib.rs"),
            b"pub fn answer() -> u32 {\n    99\n}\n",
        )
        .expect("overwrite candidate file in place");

        assert_eq!(
            candidate.revalidate_custody().unwrap_err(),
            SourceCandidateError::UnsafeWorkspaceMetadata
        );
        assert_eq!(
            candidate
                .decision_evidence(&executed, verifier)
                .unwrap_err(),
            SourceCandidateError::UnsafeWorkspaceMetadata
        );
    }

    #[test]
    fn rejects_oversized_or_hardlinked_git_config_and_excess_hooks_without_huge_allocation() {
        // 1. Oversized .git/config (MAX_GIT_CONFIG_BYTES + 1 bytes via take(limit + 1))
        let mut fixture = setup_fixture(vec!["src"], vec!["src/secret"]);
        fs::write(
            fixture.workspace_path.join("src/lib.rs"),
            b"pub fn answer() -> u32 {\n    42\n}\n",
        )
        .expect("edit src/lib.rs");
        let config_path = fixture.workspace_path.join(".git/config");
        let mut oversized_config = fs::read(&config_path).expect("read .git/config");
        let pad_len = (MAX_GIT_CONFIG_BYTES as usize + 1).saturating_sub(oversized_config.len());
        oversized_config.push(b'#');
        oversized_config.extend(std::iter::repeat_n(b'a', pad_len.saturating_sub(1)));
        fs::write(&config_path, &oversized_config).expect("write MAX_GIT_CONFIG_BYTES + 1 config");
        fs::set_permissions(&config_path, fs::Permissions::from_mode(0o600))
            .expect("chmod 0600 .git/config");
        assert_eq!(
            read_safe_config(&config_path).unwrap_err(),
            SourceCandidateError::UnsafeWorkspaceMetadata
        );
        let closed = prove_closed_workspace(fixture.take_prepared(), fixture.closure.clone())
            .expect("prove closed");
        assert_eq!(
            materialize_source_candidate(
                closed,
                &fixture.candidate_parent,
                "candidate-oversized-config"
            )
            .unwrap_err(),
            SourceCandidateError::UnsafeWorkspaceMetadata
        );

        // 2. Sparse 64 MiB .git/config rejected without allocating 64 MiB
        let fixture = setup_fixture(vec!["src"], vec!["src/secret"]);
        let sparse_config = fixture.workspace_path.join(".git/config");
        let file = fs::OpenOptions::new()
            .write(true)
            .open(&sparse_config)
            .expect("open .git/config for sparse extend");
        file.set_len(64 * 1024 * 1024)
            .expect("set sparse 64 MiB length");
        drop(file);
        assert_eq!(
            read_safe_config(&sparse_config).unwrap_err(),
            SourceCandidateError::UnsafeWorkspaceMetadata
        );

        // 3. Hardlinked .git/config (st_nlink > 1) rejected
        let fixture = setup_fixture(vec!["src"], vec!["src/secret"]);
        let config_path = fixture.workspace_path.join(".git/config");
        let hardlink_path = fixture.root.join("config-hardlink");
        fs::hard_link(&config_path, &hardlink_path).expect("hardlink .git/config");
        assert_eq!(
            read_safe_config(&config_path).unwrap_err(),
            SourceCandidateError::UnsafeWorkspaceMetadata
        );

        // 4. Excess entries in .git/hooks (> MAX_GIT_HOOK_ENTRIES) rejected
        let mut fixture = setup_fixture(vec!["src"], vec!["src/secret"]);
        fs::write(
            fixture.workspace_path.join("src/lib.rs"),
            b"pub fn answer() -> u32 {\n    42\n}\n",
        )
        .expect("edit src/lib.rs");
        let hooks_dir = fixture.workspace_path.join(".git/hooks");
        fs::create_dir_all(&hooks_dir).expect("ensure .git/hooks");
        fs::set_permissions(&hooks_dir, fs::Permissions::from_mode(0o700))
            .expect("chmod 0700 .git/hooks");
        for idx in 0..=MAX_GIT_HOOK_ENTRIES {
            let sample = hooks_dir.join(format!("extra-{idx}.sample"));
            fs::write(&sample, b"#!/bin/sh\n").expect("write hook sample");
            fs::set_permissions(&sample, fs::Permissions::from_mode(0o600))
                .expect("chmod 0600 hook sample");
        }
        let closed = prove_closed_workspace(fixture.take_prepared(), fixture.closure.clone())
            .expect("prove closed");
        assert_eq!(
            materialize_source_candidate(
                closed,
                &fixture.candidate_parent,
                "candidate-excess-hooks"
            )
            .unwrap_err(),
            SourceCandidateError::UnsafeWorkspaceMetadata
        );
    }

    #[test]
    fn rejects_oversized_or_hardlinked_source_files_and_preflights_declared_tree_blobs() {
        // 1. Direct bounded regular file reader rejects limit + 1 and hardlinks
        let fixture = setup_fixture(vec!["src"], vec!["src/secret"]);
        let bounded_test_file = fixture.root.join("bounded.txt");
        fs::write(&bounded_test_file, b"0123456789abcdef").expect("write 16-byte file");
        fs::set_permissions(&bounded_test_file, fs::Permissions::from_mode(0o600))
            .expect("chmod 0600");
        assert_eq!(
            read_bounded_regular_file(&bounded_test_file, 16, 0o002)
                .expect("exact 16-byte bound succeeds")
                .len(),
            16
        );
        assert_eq!(
            read_bounded_regular_file(&bounded_test_file, 15, 0o002).unwrap_err(),
            SourceCandidateError::UnsafeWorkspaceMetadata
        );
        let linked_file = fixture.root.join("bounded-link.txt");
        fs::hard_link(&bounded_test_file, &linked_file).expect("create hardlink");
        assert_eq!(
            read_bounded_regular_file(&bounded_test_file, 16, 0o002).unwrap_err(),
            SourceCandidateError::UnsafeWorkspaceMetadata
        );

        // 2. Dirty working-tree file exceeding MAX_SOURCE_FILE_BYTES rejected before freeze/materialization
        let mut fixture = setup_fixture(vec!["src"], vec!["src/secret"]);
        let large_dirty = fs::OpenOptions::new()
            .write(true)
            .open(fixture.workspace_path.join("src/lib.rs"))
            .expect("open src/lib.rs");
        large_dirty
            .set_len(MAX_SOURCE_FILE_BYTES + 1)
            .expect("set src/lib.rs to MAX_SOURCE_FILE_BYTES + 1");
        drop(large_dirty);
        let closed = prove_closed_workspace(fixture.take_prepared(), fixture.closure.clone())
            .expect("prove closed");
        let dest_dirty = "candidate-oversized-dirty";
        assert_eq!(
            materialize_source_candidate(closed, &fixture.candidate_parent, dest_dirty)
                .unwrap_err(),
            SourceCandidateError::UnsafeWorkspaceMetadata
        );
        assert!(
            !fixture.candidate_parent.join(dest_dirty).exists(),
            "oversized dirty working tree must be rejected before creating destination checkout"
        );

        // 3. Pre-committed oversized Git blob with small working-tree replacement + assume-unchanged
        // is rejected during preflight before destination checkout creation.
        let mut fixture = setup_fixture(vec!["src"], vec!["src/secret"]);
        let oversized_payload = vec![b'x'; (MAX_SOURCE_FILE_BYTES as usize) + 1];
        fs::write(
            fixture.workspace_path.join("src/lib.rs"),
            &oversized_payload,
        )
        .expect("write oversized blob to src/lib.rs");
        run_fixture_git(&fixture.workspace_path, &["add", "src/lib.rs"]);
        run_fixture_git(
            &fixture.workspace_path,
            &["commit", "--quiet", "-m", "commit oversized blob"],
        );
        // Shrink on-disk working tree file and hide behind assume-unchanged index flag
        fs::write(
            fixture.workspace_path.join("src/lib.rs"),
            b"pub fn answer() -> u32 {\n    42\n}\n",
        )
        .expect("shrink working-tree file after committing oversized blob");
        run_fixture_git(
            &fixture.workspace_path,
            &["update-index", "--assume-unchanged", "src/lib.rs"],
        );
        make_git_dir_private(&fixture.workspace_path.join(".git")).expect("normalize fixture .git");

        let git = trusted_git().expect("trusted git");
        let prepared_ref = fixture.prepared.as_ref().expect("prepared ref");
        let pinned_cwd = PinnedGitDirectory::from_prepared(prepared_ref).expect("pinned cwd");
        let head_commit = run_fixture_git(&fixture.workspace_path, &["rev-parse", "HEAD"]);
        let head_tree = run_fixture_git(&fixture.workspace_path, &["rev-parse", "HEAD^{tree}"]);
        assert_eq!(
            validate_commit_tree_entries(&git, &pinned_cwd, &head_commit, &head_tree).unwrap_err(),
            SourceCandidateError::UnsafeWorkspaceMetadata
        );

        let closed = prove_closed_workspace(fixture.take_prepared(), fixture.closure.clone())
            .expect("prove closed");
        let dest_blob = "candidate-oversized-blob";
        assert_eq!(
            materialize_source_candidate(closed, &fixture.candidate_parent, dest_blob).unwrap_err(),
            SourceCandidateError::UnsafeWorkspaceMetadata
        );
        assert!(
            !fixture.candidate_parent.join(dest_blob).exists(),
            "oversized committed tree must be rejected before creating destination checkout"
        );
    }

    #[test]
    fn rejects_nested_excess_or_hardlinked_git_dir_metadata_without_touching_linked_target() {
        // 1. Hardlink inside .git is rejected BEFORE fchmod touches the external linked file's mode
        let fixture = setup_fixture(vec!["src"], vec!["src/secret"]);
        let external_target = fixture.root.join("external-target.txt");
        fs::write(&external_target, b"preserve-my-permissions\n").expect("write external file");
        fs::set_permissions(&external_target, fs::Permissions::from_mode(0o644))
            .expect("set external file mode 0644");
        let git_hardlink = fixture.workspace_path.join(".git/objects/info/linked-file");
        fs::hard_link(&external_target, &git_hardlink).expect("hardlink external file into .git");

        assert_eq!(
            make_git_dir_private(&fixture.workspace_path.join(".git")).unwrap_err(),
            SourceCandidateError::UnsafeWorkspaceMetadata
        );
        assert_eq!(
            validate_git_dir_tree(&fixture.workspace_path.join(".git")).unwrap_err(),
            SourceCandidateError::UnsafeWorkspaceMetadata
        );
        let external_mode = fs::symlink_metadata(&external_target)
            .expect("external metadata")
            .mode()
            & 0o777;
        assert_eq!(
            external_mode, 0o644,
            "make_git_dir_private must reject hardlinked file before fchmod alters external file"
        );
        fs::remove_file(&git_hardlink).expect("remove hardlink");

        // 2. Wide .git directory is rejected BEFORE enqueueing unbounded children (both via
        // bounded helper limit and production MAX_GIT_DIR_ENTRIES).
        let wide_dir = fixture.workspace_path.join(".git/objects/info/wide");
        fs::create_dir_all(&wide_dir).expect("create wide dir");
        fs::set_permissions(&wide_dir, fs::Permissions::from_mode(0o700))
            .expect("chmod 0700 wide dir");
        for idx in 0..32 {
            let entry_path = wide_dir.join(format!("f-{idx:04}"));
            fs::write(&entry_path, b"x").expect("write wide child");
            fs::set_permissions(&entry_path, fs::Permissions::from_mode(0o600))
                .expect("chmod 0600 wide child");
        }
        assert_eq!(
            traverse_git_dir_bounded_with_limits(
                &fixture.workspace_path.join(".git"),
                false,
                MAX_GIT_DIR_DEPTH,
                16,
                MAX_GIT_DIR_BYTES,
            )
            .unwrap_err(),
            SourceCandidateError::UnsafeWorkspaceMetadata
        );
        for idx in 32..=MAX_GIT_DIR_ENTRIES {
            let entry_path = wide_dir.join(format!("f-{idx:04}"));
            fs::write(&entry_path, b"").expect("write wide child up to MAX_GIT_DIR_ENTRIES");
            fs::set_permissions(&entry_path, fs::Permissions::from_mode(0o600))
                .expect("chmod 0600 wide child");
        }
        assert_eq!(
            validate_git_dir_tree(&fixture.workspace_path.join(".git")).unwrap_err(),
            SourceCandidateError::UnsafeWorkspaceMetadata
        );
        assert_eq!(
            make_git_dir_private(&fixture.workspace_path.join(".git")).unwrap_err(),
            SourceCandidateError::UnsafeWorkspaceMetadata
        );
        fs::remove_dir_all(&wide_dir).expect("remove wide dir");

        // 3. Excessive directory nesting depth inside .git (> MAX_GIT_DIR_DEPTH) is rejected
        let mut deep_dir = fixture.workspace_path.join(".git");
        for depth in 0..=(MAX_GIT_DIR_DEPTH + 2) {
            deep_dir = deep_dir.join(format!("d{depth}"));
            fs::create_dir(&deep_dir).expect("create nested .git subdir");
            fs::set_permissions(&deep_dir, fs::Permissions::from_mode(0o700))
                .expect("chmod 0700 nested .git subdir");
        }
        assert_eq!(
            make_git_dir_private(&fixture.workspace_path.join(".git")).unwrap_err(),
            SourceCandidateError::UnsafeWorkspaceMetadata
        );
        assert_eq!(
            validate_git_dir_tree(&fixture.workspace_path.join(".git")).unwrap_err(),
            SourceCandidateError::UnsafeWorkspaceMetadata
        );
    }

    #[test]
    fn shared_base_hunk_coordinates_detect_overlapping_hunks_after_earlier_line_insertions() {
        use admiralty_core::conflicts::{ConflictRisk, analyze_conflicts};

        let fixture = setup_fixture(vec!["src"], vec!["src/secret"]);
        // Seed a 10-line src/lib.rs in origin-repo and prepare two workspaces sharing that exact base_sha.
        let ten_line_base =
            "line 1\nline 2\nline 3\nline 4\nline 5\nline 6\nline 7\nline 8\nline 9\nline 10\n";
        let origin_repo = fixture.root.join("origin-repo");
        fs::write(origin_repo.join("src/lib.rs"), ten_line_base).expect("write 10-line base");
        run_fixture_git(&origin_repo, &["add", "src/lib.rs"]);
        run_fixture_git(
            &origin_repo,
            &["commit", "--quiet", "-m", "ten-line shared base"],
        );
        let shared_base = run_fixture_git(&origin_repo, &["rev-parse", "HEAD"]);
        make_git_dir_private(&origin_repo.join(".git")).expect("private origin .git");

        let prepared_a = prepare_workspace(WorkspacePreparation {
            source: origin_repo.clone(),
            managed_parent: fixture.root.join("managed-workspaces"),
            destination_name: "attempt-a-work".into(),
            pinned_commit: shared_base.clone(),
            node_id: NodeRef::new("node-alpha").expect("node_id"),
            root_id: fixture.contract.root_id.clone(),
            project_id: fixture.contract.project_id.clone(),
            task_id: TaskId::new("TASK-HUNK-A").expect("task A"),
            attempt_id: AttemptId::new("attempt-a").expect("attempt A"),
            owned_paths: vec![PathBuf::from("src")],
            forbidden_paths: vec![PathBuf::from("src/secret")],
        })
        .expect("prepare workspace A");
        let prepared_b = prepare_workspace(WorkspacePreparation {
            source: origin_repo,
            managed_parent: fixture.root.join("managed-workspaces"),
            destination_name: "attempt-b-work".into(),
            pinned_commit: shared_base,
            node_id: NodeRef::new("node-alpha").expect("node_id"),
            root_id: fixture.contract.root_id.clone(),
            project_id: fixture.contract.project_id.clone(),
            task_id: TaskId::new("TASK-HUNK-B").expect("task B"),
            attempt_id: AttemptId::new("attempt-b").expect("attempt B"),
            owned_paths: vec![PathBuf::from("src")],
            forbidden_paths: vec![PathBuf::from("src/secret")],
        })
        .expect("prepare workspace B");

        // Candidate A inserts 4 new lines at line 1 AND edits base line 8.
        let edited_a = "ins A\nins B\nins C\nins D\nline 1\nline 2\nline 3\nline 4\nline 5\nline 6\nline 7\nline 8 modified by A\nline 9\nline 10\n";
        fs::write(
            prepared_a.workspace().workspace.join("src/lib.rs"),
            edited_a,
        )
        .expect("write candidate A edits");
        // Candidate B leaves top of file untouched and edits base line 8.
        let edited_b = "line 1\nline 2\nline 3\nline 4\nline 5\nline 6\nline 7\nline 8 modified by B\nline 9\nline 10\n";
        fs::write(
            prepared_b.workspace().workspace.join("src/lib.rs"),
            edited_b,
        )
        .expect("write candidate B edits");

        let mut closure_a = fixture.closure.clone();
        closure_a.identity.identity.task_id = prepared_a.task_id().clone();
        closure_a.identity.identity.attempt_id = prepared_a.attempt_id().clone();
        closure_a.observation.actual_cwd = prepared_a.workspace().workspace.clone();

        let mut closure_b = fixture.closure.clone();
        closure_b.identity.identity.task_id = prepared_b.task_id().clone();
        closure_b.identity.identity.attempt_id = prepared_b.attempt_id().clone();
        closure_b.observation.actual_cwd = prepared_b.workspace().workspace.clone();

        let candidate_a = materialize_source_candidate(
            prove_closed_workspace(prepared_a, closure_a).expect("prove closed A"),
            &fixture.candidate_parent,
            "candidate-hunk-a",
        )
        .expect("materialize candidate A");
        let candidate_b = materialize_source_candidate(
            prove_closed_workspace(prepared_b, closure_b).expect("prove closed B"),
            &fixture.candidate_parent,
            "candidate-hunk-b",
        )
        .expect("materialize candidate B");

        assert_eq!(
            candidate_a.files()[0].hunks,
            Some(vec![
                HunkRange { start: 1, end: 1 },
                HunkRange { start: 8, end: 8 },
            ])
        );
        assert_eq!(
            candidate_b.files()[0].hunks,
            Some(vec![HunkRange { start: 8, end: 8 }])
        );

        let graph = analyze_conflicts(
            &[
                candidate_a.candidate_change().clone(),
                candidate_b.candidate_change().clone(),
            ],
            "policy-v1",
        )
        .expect("analyze conflicts across shared-base candidates");
        assert_eq!(graph.edges.len(), 1);
        assert!(
            graph.edges[0]
                .risks
                .contains(&ConflictRisk::OverlappingHunks),
            "shared-base hunk coordinates must preserve OverlappingHunks across earlier insertions: {:?}",
            graph.edges[0].risks
        );
    }

    #[test]
    fn retained_custody_apis_preserve_original_handles_and_reconciliation_state_across_failures() {
        use std::os::fd::AsRawFd;

        // Note: FixtureClosedAdmission is an explicitly synthetic in-module test fixture proof
        // used to exercise custody retention and error recovery in unit tests; it does not claim
        // real Linux systemd unit execution qualification.
        let mut fixture = setup_fixture(vec!["src"], vec!["src/secret"]);
        let prepared = fixture.take_prepared();
        let original_head = prepared.head_oid().to_owned();
        let original_tree = prepared.tree_oid().to_owned();
        let original_parent_ino =
            fd_identity(prepared.managed_parent_fd()).expect("original parent fd identity");
        let original_workspace_ino =
            fd_identity(prepared.workspace_dir_fd()).expect("original workspace fd identity");
        let original_parent_raw_fd = prepared.managed_parent_fd().as_raw_fd();
        let original_workspace_raw_fd = prepared.workspace_dir_fd().as_raw_fd();
        let original_closure_handle_id = fixture.closure.synthetic_handle_id;

        // 1. Wrong closure (failed exit code and mismatched task identity) returns original
        // PreparedWorkspace and generic synthetic closure proof without dropping or recreating them.
        let mut failed_closure = fixture.closure.clone();
        failed_closure.exit_code = Some(1);
        failed_closure.success = false;
        let prove_err =
            try_prove_closed_workspace(prepared, failed_closure).expect_err("failed exit rejected");
        assert_eq!(
            prove_err.error(),
            &SourceCandidateError::ExecutionNotSucceeded
        );
        let (prepared, returned_failed_closure, _) = prove_err.into_parts();
        assert_eq!(
            returned_failed_closure.synthetic_handle_id,
            original_closure_handle_id
        );
        assert_eq!(prepared.head_oid(), original_head);
        assert_eq!(prepared.tree_oid(), original_tree);
        assert_eq!(
            prepared.managed_parent_fd().as_raw_fd(),
            original_parent_raw_fd
        );
        assert_eq!(
            prepared.workspace_dir_fd().as_raw_fd(),
            original_workspace_raw_fd
        );
        assert_eq!(
            fd_identity(prepared.managed_parent_fd()).expect("parent fd identity"),
            original_parent_ino
        );
        assert_eq!(
            fd_identity(prepared.workspace_dir_fd()).expect("workspace fd identity"),
            original_workspace_ino
        );
        prepared
            .validate_current_custody()
            .expect("retained PreparedWorkspace custody remains valid");

        let mut mismatched_closure = fixture.closure.clone();
        mismatched_closure.identity.identity.task_id =
            TaskId::new("TASK-MISMATCHED").expect("task id");
        let prove_err = try_prove_closed_workspace(prepared, mismatched_closure)
            .expect_err("mismatched task rejected");
        assert_eq!(
            prove_err.error(),
            &SourceCandidateError::ClosureIdentityMismatch
        );
        let (prepared, returned_mismatched_closure, _) = prove_err.into_parts();
        assert_eq!(
            returned_mismatched_closure.synthetic_handle_id,
            original_closure_handle_id
        );
        assert_eq!(
            prepared.managed_parent_fd().as_raw_fd(),
            original_parent_raw_fd
        );
        assert_eq!(
            prepared.workspace_dir_fd().as_raw_fd(),
            original_workspace_raw_fd
        );

        let closed = try_prove_closed_workspace(prepared, fixture.closure.clone())
            .expect("valid closure proof succeeds");
        assert_eq!(
            closed
                .synthetic_test_closure_proof()
                .expect("retained synthetic closure proof")
                .synthetic_handle_id,
            original_closure_handle_id
        );

        // 2. No-change error returns original ClosedWorkspaceExecution with untouched head/tree/inode/custody.
        let no_change_err = try_materialize_source_candidate(
            closed,
            &fixture.candidate_parent,
            "candidate-no-change",
        )
        .expect_err("no source changes rejected");
        assert_eq!(
            no_change_err.error(),
            &SourceCandidateError::NoSourceChanges
        );
        assert_eq!(
            no_change_err.stage(),
            MaterializationFailureStage::SourcePreflight
        );
        assert!(!no_change_err.reconciliation().source_working_tree_frozen);
        assert_eq!(
            no_change_err
                .reconciliation()
                .observed_source_head_oid
                .as_deref(),
            Some(original_head.as_str())
        );
        assert_eq!(
            no_change_err
                .reconciliation()
                .observed_source_tree_oid
                .as_deref(),
            Some(original_tree.as_str())
        );
        assert!(no_change_err.partial_destination_path().is_none());
        assert!(no_change_err.occupied_destination_path().is_none());
        assert!(!no_change_err.requires_reconciliation());
        let closed = no_change_err.into_execution();
        closed
            .revalidate_custody()
            .expect("custody valid after no-change error");
        assert_eq!(
            closed.prepared().managed_parent_fd().as_raw_fd(),
            original_parent_raw_fd
        );
        assert_eq!(
            closed.prepared().workspace_dir_fd().as_raw_fd(),
            original_workspace_raw_fd
        );
        assert_eq!(
            closed
                .synthetic_test_closure_proof()
                .expect("retained synthetic closure proof")
                .synthetic_handle_id,
            original_closure_handle_id
        );

        // Write a dirty source edit inside owned_paths ("src/lib.rs") for subsequent error tests.
        fs::write(
            fixture.workspace_path.join("src/lib.rs"),
            b"pub fn answer() -> u32 {\n    42\n}\n",
        )
        .expect("write dirty edit to src/lib.rs");

        // 3. Unsafe .git/config error rejects in SourcePreflight before freezing dirty edits,
        // never invokes diagnostic Git, and preserves original head/tree/inode/custody and synthetic closure handle.
        let config_path = fixture.workspace_path.join(".git/config");
        let saved_config = fs::read(&config_path).expect("read original .git/config");
        let mut unsafe_config = saved_config.clone();
        unsafe_config.extend_from_slice(b"\n[filter \"evil\"]\n\tsmudge = /bin/true\n");
        fs::write(&config_path, &unsafe_config).expect("write unsafe .git/config");
        fs::set_permissions(&config_path, fs::Permissions::from_mode(0o600))
            .expect("chmod 0600 .git/config");

        reset_test_git_invocations();
        let unsafe_cfg_err = try_materialize_source_candidate(
            closed,
            &fixture.candidate_parent,
            "candidate-unsafe-cfg",
        )
        .expect_err("unsafe .git/config rejected");
        assert_eq!(
            test_git_invocations(),
            0,
            "rejected .git/config must never invoke Git or diagnostic Git"
        );
        assert_eq!(
            unsafe_cfg_err.error(),
            &SourceCandidateError::UnsafeWorkspaceMetadata
        );
        assert_eq!(
            unsafe_cfg_err.stage(),
            MaterializationFailureStage::SourcePreflight
        );
        assert!(!unsafe_cfg_err.reconciliation().source_working_tree_frozen);
        assert!(
            unsafe_cfg_err
                .reconciliation()
                .observed_source_head_oid
                .is_none()
        );
        assert!(
            unsafe_cfg_err
                .reconciliation()
                .observed_source_tree_oid
                .is_none()
        );
        assert!(unsafe_cfg_err.partial_destination_path().is_none());
        let closed = unsafe_cfg_err.into_execution();
        fs::write(&config_path, &saved_config).expect("restore safe .git/config");
        fs::set_permissions(&config_path, fs::Permissions::from_mode(0o600))
            .expect("chmod 0600 restored .git/config");
        closed
            .revalidate_custody()
            .expect("custody valid after restoring .git/config");
        assert_eq!(
            run_fixture_git(&fixture.workspace_path, &["rev-parse", "HEAD"]),
            original_head,
            "dirty working tree must not be frozen when SourcePreflight fails"
        );

        // 4. Invalid/missing candidate_parent error rejects in DestinationPreflight BEFORE freezing
        // dirty working tree, preserving original head/tree/inode/custody and closure handle.
        let missing_parent = fixture.root.join("missing-candidate-parent");
        let missing_parent_err =
            try_materialize_source_candidate(closed, &missing_parent, "candidate-missing-parent")
                .expect_err("missing candidate_parent rejected");
        assert_eq!(
            missing_parent_err.error(),
            &SourceCandidateError::InvalidCandidateDestination
        );
        assert_eq!(
            missing_parent_err.stage(),
            MaterializationFailureStage::DestinationPreflight
        );
        assert!(
            !missing_parent_err
                .reconciliation()
                .source_working_tree_frozen
        );
        assert_eq!(
            missing_parent_err
                .reconciliation()
                .observed_source_head_oid
                .as_deref(),
            Some(original_head.as_str())
        );
        assert_eq!(
            missing_parent_err
                .reconciliation()
                .observed_source_tree_oid
                .as_deref(),
            Some(original_tree.as_str())
        );
        assert!(missing_parent_err.partial_destination_path().is_none());
        assert!(missing_parent_err.occupied_destination_path().is_none());
        assert!(!missing_parent_err.requires_reconciliation());
        let closed = missing_parent_err.into_execution();

        // 5. Occupied destination error rejects in DestinationPreflight BEFORE freezing dirty working
        // tree, preserves the existing directory untouched, and reports occupied_destination_path.
        let occupied_name = "candidate-already-occupied";
        let occupied_path = fixture.candidate_parent.join(occupied_name);
        fs::create_dir(&occupied_path).expect("create pre-existing occupied destination");
        fs::set_permissions(&occupied_path, fs::Permissions::from_mode(0o700))
            .expect("chmod 0700 occupied destination");
        let sentinel = occupied_path.join("keep.txt");
        fs::write(&sentinel, b"do-not-overwrite\n").expect("write sentinel in occupied dir");

        let occupied_err =
            try_materialize_source_candidate(closed, &fixture.candidate_parent, occupied_name)
                .expect_err("occupied destination rejected");
        assert_eq!(
            occupied_err.error(),
            &SourceCandidateError::InvalidCandidateDestination
        );
        assert_eq!(
            occupied_err.stage(),
            MaterializationFailureStage::DestinationPreflight
        );
        assert!(!occupied_err.reconciliation().source_working_tree_frozen);
        assert_eq!(
            occupied_err.occupied_destination_path(),
            Some(occupied_path.as_path())
        );
        assert!(occupied_err.partial_destination_path().is_none());
        assert!(occupied_err.requires_reconciliation());
        assert_eq!(
            fs::read(&sentinel).expect("sentinel preserved"),
            b"do-not-overwrite\n"
        );
        assert_eq!(
            run_fixture_git(&fixture.workspace_path, &["rev-parse", "HEAD"]),
            original_head,
            "occupied destination preflight must not freeze dirty source"
        );
        let closed = occupied_err.into_execution();

        // 6. Partial destination creation + frozen source failure during DestinationCheckout:
        //    - Freezes dirty source into frozen_sha
        //    - Creates partial destination directory via mkdirat/git init
        //    - Fails when post-init config validation sees injected unsafe config in destination
        //    - Preserves partial destination on disk (not deleted or overwritten)
        //    - Records frozen source observation and requires explicit reconciliation + fresh destination
        let partial_name = "candidate-partial-checkout";
        let partial_expected_path = fixture.candidate_parent.join(partial_name);
        let inject_bad_dest_config = |dest_path: &Path| {
            let dest_cfg = dest_path.join(".git/config");
            let mut bytes = fs::read(&dest_cfg).expect("read dest .git/config");
            bytes.extend_from_slice(b"\n[include]\n\tpath = /etc/passwd\n");
            fs::write(&dest_cfg, bytes).expect("inject unsafe dest .git/config");
            fs::set_permissions(&dest_cfg, fs::Permissions::from_mode(0o600))
                .expect("chmod 0600 dest .git/config");
        };
        let partial_err = try_materialize_source_candidate_with_hook(
            closed,
            &fixture.candidate_parent,
            partial_name,
            Some(&inject_bad_dest_config),
        )
        .expect_err("post-mkdir checkout failure rejected");
        assert_eq!(
            partial_err.error(),
            &SourceCandidateError::UnsafeWorkspaceMetadata
        );
        assert_eq!(
            partial_err.stage(),
            MaterializationFailureStage::DestinationCheckout
        );
        assert!(partial_err.reconciliation().source_working_tree_frozen);
        assert_eq!(
            partial_err.partial_destination_path(),
            Some(partial_expected_path.as_path())
        );
        assert!(
            partial_err
                .reconciliation()
                .partial_destination_identity
                .is_some()
        );
        assert!(partial_err.requires_reconciliation());
        assert!(
            partial_expected_path.is_dir(),
            "partially created destination directory must be preserved for inspection"
        );
        let frozen_sha = partial_err
            .reconciliation()
            .observed_source_head_oid
            .clone()
            .expect("observed frozen head sha");
        let frozen_tree_sha = partial_err
            .reconciliation()
            .observed_source_tree_oid
            .clone()
            .expect("observed frozen tree sha");
        assert_ne!(frozen_sha, original_head);
        assert_ne!(frozen_tree_sha, original_tree);

        let closed = partial_err.into_execution();
        // Original PreparedWorkspace base commit, raw fds, and synthetic closure handle are still intact!
        assert_eq!(closed.prepared().head_oid(), original_head);
        assert_eq!(closed.prepared().tree_oid(), original_tree);
        assert_eq!(
            closed.prepared().managed_parent_fd().as_raw_fd(),
            original_parent_raw_fd
        );
        assert_eq!(
            closed.prepared().workspace_dir_fd().as_raw_fd(),
            original_workspace_raw_fd
        );
        assert_eq!(
            closed
                .synthetic_test_closure_proof()
                .expect("retained synthetic closure proof")
                .synthetic_handle_id,
            original_closure_handle_id
        );

        // Blind retry without acknowledging frozen source reconciliation is rejected in SourcePreflight.
        let unreconciled_err = try_materialize_source_candidate(
            closed,
            &fixture.candidate_parent,
            "candidate-after-partial-unreconciled",
        )
        .expect_err("unreconciled frozen source retry must be rejected");
        assert_eq!(
            unreconciled_err.error(),
            &SourceCandidateError::SourceReconciliationRequired
        );
        assert_eq!(
            unreconciled_err.stage(),
            MaterializationFailureStage::SourcePreflight
        );
        assert!(unreconciled_err.requires_reconciliation());
        let mut closed = unreconciled_err.into_execution();

        // Acknowledge frozen source reconciliation, then verify retrying into the SAME partial_name
        // still fails as occupied (never overwriting or claiming idempotent success).
        closed
            .acknowledge_frozen_source_reconciliation(&frozen_sha, &frozen_tree_sha)
            .expect("acknowledge frozen source commit and tree");
        let retry_same_dest_err =
            try_materialize_source_candidate(closed, &fixture.candidate_parent, partial_name)
                .expect_err("retrying into partial destination path must fail as occupied");
        assert_eq!(
            retry_same_dest_err.error(),
            &SourceCandidateError::InvalidCandidateDestination
        );
        assert_eq!(
            retry_same_dest_err.stage(),
            MaterializationFailureStage::DestinationPreflight
        );
        assert_eq!(
            retry_same_dest_err.occupied_destination_path(),
            Some(partial_expected_path.as_path())
        );
        let closed = retry_same_dest_err.into_execution();

        // Retrying with the reconciled ClosedWorkspaceExecution into a fresh create-only destination
        // succeeds without re-freezing under a new commit identity.
        let candidate = try_materialize_source_candidate(
            closed,
            &fixture.candidate_parent,
            "candidate-reconciled-fresh",
        )
        .expect("reconciled materialization into fresh destination succeeds");
        assert_eq!(candidate.base_commit_sha(), original_head);
        assert_eq!(candidate.base_tree_sha(), original_tree);
        assert_eq!(candidate.source_commit_sha(), frozen_sha);
        assert_eq!(candidate.source_tree_sha(), frozen_tree_sha);
        assert_eq!(
            candidate
                .execution()
                .prepared()
                .managed_parent_fd()
                .as_raw_fd(),
            original_parent_raw_fd
        );
        assert_eq!(
            candidate
                .execution()
                .prepared()
                .workspace_dir_fd()
                .as_raw_fd(),
            original_workspace_raw_fd
        );
        assert_eq!(
            candidate
                .execution()
                .synthetic_test_closure_proof()
                .expect("retained synthetic closure proof on candidate")
                .synthetic_handle_id,
            original_closure_handle_id
        );
    }

    #[test]
    fn unknown_freeze_fence_and_rejected_config_never_allow_blind_retry_or_diagnostic_git() {
        use std::os::fd::AsRawFd;

        let mut fixture = setup_fixture(vec!["src"], vec!["src/secret"]);
        let prepared = fixture.take_prepared();
        let original_head = prepared.head_oid().to_owned();
        let original_tree = prepared.tree_oid().to_owned();
        let original_parent_raw_fd = prepared.managed_parent_fd().as_raw_fd();
        let original_workspace_raw_fd = prepared.workspace_dir_fd().as_raw_fd();
        let original_parent_id =
            fd_identity(prepared.managed_parent_fd()).expect("original parent fd identity");
        let original_workspace_id =
            fd_identity(prepared.workspace_dir_fd()).expect("original workspace fd identity");
        let original_closure_handle_id = fixture.closure.synthetic_handle_id;

        let closed = try_prove_closed_workspace(prepared, fixture.closure.clone())
            .expect("prove closed workspace");
        assert!(!closed.freeze_fence_active());
        assert!(closed.frozen_source().is_none());

        fs::write(
            fixture.workspace_path.join("src/lib.rs"),
            b"pub fn answer() -> u32 {\n    77\n}\n",
        )
        .expect("write dirty source edit");

        // 1. Rejected .git/config with [include] and core.fsmonitor must fail in SourcePreflight
        // with zero Git invocations (no diagnostic Git rev-parse) and no observations.
        let config_path = fixture.workspace_path.join(".git/config");
        let saved_config = fs::read(&config_path).expect("read original .git/config");
        let marker_path = fixture.root.join("diagnostic-git-marker");
        let external_include_path = fixture.root.join("external-include.gitconfig");
        fs::write(
            &external_include_path,
            format!("[core]\n\tfsmonitor = touch {}\n", marker_path.display()),
        )
        .expect("write external include config");
        let mut rejected_include_config = saved_config.clone();
        rejected_include_config.extend_from_slice(
            format!(
                "\n[include]\n\tpath = {}\n[core]\n\tfsmonitor = touch {}\n",
                external_include_path.display(),
                marker_path.display()
            )
            .as_bytes(),
        );
        fs::write(&config_path, &rejected_include_config).expect("write rejected .git/config");
        fs::set_permissions(&config_path, fs::Permissions::from_mode(0o600))
            .expect("chmod 0600 rejected .git/config");

        reset_test_git_invocations();
        let cfg_err = try_materialize_source_candidate(
            closed,
            &fixture.candidate_parent,
            "candidate-rejected-include-cfg",
        )
        .expect_err("rejected [include] .git/config must fail");
        assert_eq!(
            test_git_invocations(),
            0,
            "rejected .git/config must never invoke diagnostic Git"
        );
        assert!(
            !marker_path.exists(),
            "rejected .git/config must never execute fsmonitor or included config"
        );
        assert_eq!(
            cfg_err.error(),
            &SourceCandidateError::UnsafeWorkspaceMetadata
        );
        assert_eq!(
            cfg_err.stage(),
            MaterializationFailureStage::SourcePreflight
        );
        assert!(!cfg_err.reconciliation().source_working_tree_frozen);
        assert!(cfg_err.reconciliation().observed_source_head_oid.is_none());
        assert!(cfg_err.reconciliation().observed_source_tree_oid.is_none());
        assert!(!cfg_err.requires_reconciliation());

        let closed = cfg_err.into_execution();
        assert!(!closed.freeze_fence_active());
        assert!(closed.frozen_source().is_none());
        fs::write(&config_path, &saved_config).expect("restore safe .git/config");
        fs::set_permissions(&config_path, fs::Permissions::from_mode(0o600))
            .expect("chmod 0600 restored .git/config");

        // 2. Inject a SourceFreeze failure after `git add` stages into .git/index by making
        // `.git/HEAD` unreadable (`0000`) before `git commit`, leaving an unverified/unknown
        // freeze state with unavailable post-freeze observations (None, None).
        let head_file = fixture.workspace_path.join(".git/HEAD");
        set_test_after_freeze_add_hook(|workspace_path| {
            let head = workspace_path.join(".git/HEAD");
            fs::set_permissions(&head, fs::Permissions::from_mode(0o000))
                .expect("chmod 0000 .git/HEAD during SourceFreeze");
        });

        let freeze_err = try_materialize_source_candidate(
            closed,
            &fixture.candidate_parent,
            "candidate-source-freeze-fail",
        )
        .expect_err("SourceFreeze commit failure must be rejected");
        assert_eq!(
            freeze_err.stage(),
            MaterializationFailureStage::SourceFreeze
        );
        assert!(freeze_err.reconciliation().source_working_tree_frozen);
        assert!(freeze_err.requires_reconciliation());
        assert!(
            freeze_err
                .reconciliation()
                .observed_source_head_oid
                .is_none(),
            "failed SourceFreeze must not return unvalidated diagnostic head observation"
        );
        assert!(
            freeze_err
                .reconciliation()
                .observed_source_tree_oid
                .is_none(),
            "failed SourceFreeze must not return unvalidated diagnostic tree observation"
        );
        assert!(
            !fixture
                .candidate_parent
                .join("candidate-source-freeze-fail")
                .exists(),
            "failed SourceFreeze must not create destination directory"
        );

        let mut closed = freeze_err.into_execution();
        assert!(
            closed.freeze_fence_active(),
            "independent freeze fence must remain active on original execution after SourceFreeze failure"
        );
        assert!(
            closed.frozen_source().is_none(),
            "unverified SourceFreeze failure must keep frozen_source as None"
        );

        // Restore readable .git/HEAD metadata and verify the unknown freeze fence STILL blocks
        // both fake/base reconciliation and blind retry without spawning Git or creating a destination.
        fs::set_permissions(&head_file, fs::Permissions::from_mode(0o600))
            .expect("restore 0600 .git/HEAD");
        assert_eq!(
            closed.acknowledge_frozen_source_reconciliation(&original_head, &original_tree),
            Err(SourceCandidateError::SourceReconciliationRequired),
            "unknown freeze fence must not be cleared by passing base or fake hashes"
        );

        reset_test_git_invocations();
        let retry_err = try_materialize_source_candidate(
            closed,
            &fixture.candidate_parent,
            "candidate-retry-after-unknown-freeze",
        )
        .expect_err("blind retry after unknown SourceFreeze failure must be rejected");
        assert_eq!(
            test_git_invocations(),
            0,
            "held unknown freeze fence must reject in SourcePreflight before invoking Git"
        );
        assert_eq!(
            retry_err.error(),
            &SourceCandidateError::SourceReconciliationRequired
        );
        assert_eq!(
            retry_err.stage(),
            MaterializationFailureStage::SourcePreflight
        );
        assert!(retry_err.reconciliation().source_working_tree_frozen);
        assert!(retry_err.requires_reconciliation());
        assert!(
            retry_err
                .reconciliation()
                .observed_source_head_oid
                .is_none()
        );
        assert!(
            retry_err
                .reconciliation()
                .observed_source_tree_oid
                .is_none()
        );
        assert!(
            !fixture
                .candidate_parent
                .join("candidate-retry-after-unknown-freeze")
                .exists(),
            "blocked retry must not create destination directory"
        );

        let closed = retry_err.into_execution();
        assert!(closed.freeze_fence_active());
        assert!(closed.frozen_source().is_none());
        closed
            .revalidate_custody()
            .expect("original descriptor custody preserved across held unknown-freeze retry");
        assert_eq!(closed.prepared().head_oid(), original_head);
        assert_eq!(closed.prepared().tree_oid(), original_tree);
        assert_eq!(closed.parent_descriptor_identity(), original_parent_id);
        assert_eq!(
            closed.workspace_descriptor_identity(),
            original_workspace_id
        );
        assert_eq!(
            closed.prepared().managed_parent_fd().as_raw_fd(),
            original_parent_raw_fd
        );
        assert_eq!(
            closed.prepared().workspace_dir_fd().as_raw_fd(),
            original_workspace_raw_fd
        );
        assert_eq!(
            closed
                .synthetic_test_closure_proof()
                .expect("retained synthetic closure proof")
                .synthetic_handle_id,
            original_closure_handle_id
        );
        assert_eq!(
            run_fixture_git(&fixture.workspace_path, &["rev-parse", "HEAD"]),
            original_head,
            "no second freeze commit may be created after unknown SourceFreeze failure"
        );
    }
}
