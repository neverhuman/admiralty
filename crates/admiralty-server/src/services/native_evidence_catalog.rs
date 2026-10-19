//! Bounded, process-local custody and evidence catalog implementing
//! [`NativeEvidenceAuthority`] for native candidate acceptance.
//!
//! # Security Model & Explicit Scope Limitations
//!
//! - **Actual opaque proofs required**: Production registration
//!   ([`NativeEvidenceCatalog::record_candidate`] /
//!   [`NativeEvidenceCatalog::register_verified_evidence`]) accepts ONLY an
//!   actual [`SourceCandidate`], actual [`ExecutedChecks`], and actual
//!   [`ClosedAdmission`] (`candidate.execution().closed_admission()`). No
//!   deserialized trust flags, public grant constructors, arbitrary digest
//!   strings, or worker-reported exit claims can create an authenticated entry.
//! - **Pre-callback custody & receipt revalidation**: Disk and Git custody
//!   revalidation (`candidate.revalidate_custody()`), candidate decision
//!   evidence binding (`candidate.decision_evidence(...)`), independent
//!   verifier receipt validation (`validate_receipt(...)`), distinct prepared
//!   workspace vs detached candidate checkout alignment, and `ClosedAdmission`
//!   identity/observation/exit checks all run **outside** Store's writer
//!   callback before a catalog entry is recorded.
//! - **Zero-I/O, non-blocking authority callbacks**:
//!   [`NativeEvidenceAuthority::authenticate_evidence`] and
//!   [`NativeEvidenceAuthority::remains_current`] never perform filesystem,
//!   Git, Store, network, or subprocess I/O. They check the atomic fail-closed
//!   latch before acquiring `state`, after acquiring `state`, and immediately
//!   before returning any positive result while still holding `state`, using
//!   `try_lock()` and comparing every field of [`NativeDecisionBinding`],
//!   [`ExecutedChecks`], and [`ClosedAdmission`] against retained immutable
//!   records in bounded memory.
//! - **Bounded capacity, reserved revocation space & atomic fail-closed latch**:
//!   Total occupied slots (`active + tombstones`) are bounded by `limit`
//!   (`1..=MAX_EVIDENCE_CATALOG_RECORDS`, max `1024`), reserving tombstone
//!   capacity for every active record so revoking an active record never fails
//!   due to capacity. Furthermore, if any revocation or drift revalidation
//!   encounters lock contention, capacity exhaustion, or invalid state, the
//!   catalog atomically latches into a global fail-closed state
//!   ([`NativeEvidenceCatalog::is_fail_closed`]) and all in-flight and future
//!   callbacks/publications/revalidations recheck the latch under the state
//!   guard before returning so authority never remains live after an incomplete
//!   revocation or observed drift.
//! - **Honest limitations**: Catalog authenticity represents process-local
//!   prior-observation custody only. It does **not** claim ongoing on-disk
//!   immutability after registration without revalidation, cross-restart
//!   durability, durable Store accounting, provider qualification, or installed
//!   service authority. Callers must invoke
//!   [`NativeEvidenceCatalog::revalidate_candidate_or_revoke`] with the actual
//!   [`SourceCandidate`] outside the Store callback immediately before grant
//!   issuance and reconcile races.

use super::native_lifecycle::{
    NativeDecisionBinding, NativeEvidenceAuthority, NativeLifecyclePolicy,
};
use admiralty_core::{
    AttemptId, CandidateDecisionEvidence, PrincipalId, ProjectId, RootId, SchedulingState, TaskId,
    TaskView, content_digest,
    verification::{ReceiptProducer, validate_receipt},
};
use admiralty_runtime::{
    resource_gate::{ClosedAdmission, EnforcementObservation, RecoveryIdentity},
    trusted_checks::ExecutedChecks,
    workspace::source_candidate::SourceCandidate,
};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

/// Maximum number of total verified evidence slots (`active + tombstones`)
/// permitted in a single [`NativeEvidenceCatalog`].
pub const MAX_EVIDENCE_CATALOG_RECORDS: usize = 1024;

type AttemptSlotKey = (TaskId, AttemptId);

#[cfg(test)]
type InFlightHook = Box<dyn Fn() + Send + Sync>;

/// Fail-closed error returned by [`NativeEvidenceCatalog`] operations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NativeEvidenceCatalogError {
    /// Policy or capacity configuration is invalid.
    InvalidPolicy(&'static str),
    /// Task state, scope, owner, or candidate identity does not match policy or candidate.
    CandidateMismatch(&'static str),
    /// `SourceCandidate` does not carry a real `ClosedAdmission` proof.
    MissingClosedAdmission,
    /// `ExecutedChecks`, `VerificationInputs`, or `ClosedAdmission` failed verification.
    EvidenceMismatch(&'static str),
    /// On-disk candidate checkout custody failed revalidation; prior record was revoked.
    CustodyDrifted(&'static str),
    /// Evidence or candidate identity has been revoked or superseded and cannot be used or replayed.
    RevokedEvidence(&'static str),
    /// Conflicting record already exists for the same proof or candidate identity.
    RecordConflict(&'static str),
    /// Total catalog slots (`active + tombstones`) reached the configured bounded capacity (`<= 1024`).
    CatalogFull,
    /// Internal catalog lock is contended, poisoned, or globally fail-closed.
    Unavailable,
}

impl std::fmt::Display for NativeEvidenceCatalogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "native evidence catalog rejected: {self:?}")
    }
}

impl std::error::Error for NativeEvidenceCatalogError {}

/// Opaque local handle returned after [`NativeEvidenceCatalog::record_candidate`]
/// validates actual candidate custody, independent check execution, and runtime
/// closure. This handle is a local publication proof only, never task
/// acceptance by itself.
#[derive(Clone, Debug)]
pub struct PublishedCandidateCustody {
    custody_digest: String,
    publication_digest: String,
    proof_key: String,
    source_workspace_path: PathBuf,
    candidate_checkout_path: PathBuf,
    binding: NativeDecisionBinding,
    decision_evidence: CandidateDecisionEvidence,
    recorded_sequence: u64,
}

impl PublishedCandidateCustody {
    /// Authenticated custody digest recorded in [`NativeEvidenceCatalog`].
    pub fn custody_digest(&self) -> &str {
        &self.custody_digest
    }

    /// Canonical digest over the catalog publication record.
    pub fn publication_digest(&self) -> &str {
        &self.publication_digest
    }

    /// Canonical proof key binding `(checks_execution_digest, closure_digest, contract_digest)`.
    pub fn proof_key(&self) -> &str {
        &self.proof_key
    }

    /// Original prepared execution workspace path where `ClosedAdmission` ran.
    pub fn source_workspace_path(&self) -> &Path {
        &self.source_workspace_path
    }

    /// Detached materialized candidate checkout path where `ExecutedChecks` ran.
    pub fn candidate_checkout_path(&self) -> &Path {
        &self.candidate_checkout_path
    }

    /// Exact decision binding authenticated by this catalog entry.
    pub fn binding(&self) -> &NativeDecisionBinding {
        &self.binding
    }

    /// Exact [`CandidateDecisionEvidence`] derived from the verified candidate and checks.
    pub fn decision_evidence(&self) -> &CandidateDecisionEvidence {
        &self.decision_evidence
    }

    /// Monotonic process-local catalog sequence number assigned at registration.
    pub fn recorded_sequence(&self) -> u64 {
        self.recorded_sequence
    }
}

/// Read-only summary of an active verified evidence record for deterministic inspection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedEvidenceSummary {
    pub custody_digest: String,
    pub publication_digest: String,
    pub proof_key: String,
    pub source_workspace_path: PathBuf,
    pub candidate_checkout_path: PathBuf,
    pub project_id: ProjectId,
    pub root_id: RootId,
    pub task_id: TaskId,
    pub attempt_id: AttemptId,
    pub candidate_ref: String,
    pub worker: PrincipalId,
    pub verifier: PrincipalId,
    pub checks_execution_digest: String,
    pub receipt_digest: String,
    pub closure_digest: String,
    pub source_commit_sha: String,
    pub source_tree_sha: String,
    pub source_artifact_digest: String,
    pub recorded_sequence: u64,
}

/// Immutable tombstone recorded when an evidence record or candidate custody
/// digest is revoked, invalidated by disk drift, or superseded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RevocationTombstone {
    pub custody_digest: String,
    pub task_id: Option<TaskId>,
    pub attempt_id: Option<AttemptId>,
    pub candidate_ref: Option<String>,
    pub proof_key: Option<String>,
    pub closure_digest: Option<String>,
    pub checks_execution_digest: Option<String>,
    pub reason: String,
    pub revoked_sequence: u64,
}

#[derive(Clone, Debug)]
struct CatalogRecord {
    custody_digest: String,
    publication_digest: String,
    proof_key: String,
    source_workspace_path: PathBuf,
    candidate_checkout_path: PathBuf,
    binding: NativeDecisionBinding,
    decision_evidence: CandidateDecisionEvidence,
    executed_checks: ExecutedChecks,
    closed_identity: RecoveryIdentity,
    closed_observation: EnforcementObservation,
    closed_exit_code: Option<i32>,
    closed_success: bool,
    recorded_sequence: u64,
}

impl CatalogRecord {
    fn summary(&self) -> VerifiedEvidenceSummary {
        VerifiedEvidenceSummary {
            custody_digest: self.custody_digest.clone(),
            publication_digest: self.publication_digest.clone(),
            proof_key: self.proof_key.clone(),
            source_workspace_path: self.source_workspace_path.clone(),
            candidate_checkout_path: self.candidate_checkout_path.clone(),
            project_id: self.binding.project_id.clone(),
            root_id: self.binding.root_id.clone(),
            task_id: self.binding.task_id.clone(),
            attempt_id: self.binding.attempt_id.clone(),
            candidate_ref: self.binding.candidate_ref.clone(),
            worker: self.binding.worker.clone(),
            verifier: self.binding.verifier.clone(),
            checks_execution_digest: self.binding.checks_execution_digest.clone(),
            receipt_digest: self.binding.receipt_digest.clone(),
            closure_digest: self.binding.closure_digest.clone(),
            source_commit_sha: self.binding.source_commit_sha.clone(),
            source_tree_sha: self.binding.source_tree_sha.clone(),
            source_artifact_digest: self.binding.source_artifact_digest.clone(),
            recorded_sequence: self.recorded_sequence,
        }
    }
}

#[derive(Default)]
struct CatalogState {
    active_by_custody: BTreeMap<String, CatalogRecord>,
    custody_by_proof_key: BTreeMap<String, String>,
    custody_by_attempt: BTreeMap<AttemptSlotKey, String>,
    tombstones_by_custody: BTreeMap<String, RevocationTombstone>,
    revoked_proof_keys: BTreeMap<String, String>,
    revoked_closure_digests: BTreeMap<String, String>,
    revoked_check_digests: BTreeMap<String, String>,
}

/// Concrete bounded process-local [`NativeEvidenceAuthority`] catalog.
pub struct NativeEvidenceCatalog {
    policy: NativeLifecyclePolicy,
    limit: usize,
    fail_closed: AtomicBool,
    sequence: AtomicU64,
    state: Mutex<CatalogState>,
    #[cfg(test)]
    in_flight_hook: Mutex<Option<InFlightHook>>,
}

impl std::fmt::Debug for NativeEvidenceCatalog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeEvidenceCatalog")
            .field("policy", &self.policy)
            .field("limit", &self.limit)
            .field("fail_closed", &self.fail_closed.load(Ordering::SeqCst))
            .finish_non_exhaustive()
    }
}

impl NativeEvidenceCatalog {
    /// Construct a new bounded catalog scoped to `policy` with total slot capacity
    /// `limit` (`1..=MAX_EVIDENCE_CATALOG_RECORDS`).
    pub fn new(
        policy: NativeLifecyclePolicy,
        limit: usize,
    ) -> Result<Self, NativeEvidenceCatalogError> {
        if !(1..=MAX_EVIDENCE_CATALOG_RECORDS).contains(&limit) {
            return Err(NativeEvidenceCatalogError::InvalidPolicy(
                "catalog limit must be within 1..=MAX_EVIDENCE_CATALOG_RECORDS",
            ));
        }
        if !bounded(&policy.authority_ref)
            || !bounded(&policy.source_producer)
            || !bounded(&policy.source_incarnation)
            || policy.verification.schema_version != 1
            || !valid_digest(&policy.verification.protected_oracle_digest)
            || policy.verification.required_negative_checks.is_empty()
        {
            return Err(NativeEvidenceCatalogError::InvalidPolicy(
                "invalid native lifecycle or verification policy",
            ));
        }
        Ok(Self {
            policy,
            limit,
            fail_closed: AtomicBool::new(false),
            sequence: AtomicU64::new(1),
            state: Mutex::new(CatalogState::default()),
            #[cfg(test)]
            in_flight_hook: Mutex::new(None),
        })
    }

    /// Configured maximum number of total catalog slots (`active + tombstones <= 1024`).
    pub fn limit(&self) -> usize {
        self.limit
    }

    /// Reference to the fixed trusted [`NativeLifecyclePolicy`] governing this catalog.
    pub fn policy(&self) -> &NativeLifecyclePolicy {
        &self.policy
    }

    /// Returns `true` if the catalog has latched into a global fail-closed state
    /// due to a contended or failed revocation/invalidation.
    pub fn is_fail_closed(&self) -> bool {
        self.fail_closed.load(Ordering::SeqCst)
    }

    #[cfg(test)]
    fn fire_in_flight_hook(&self) {
        let hook = self
            .in_flight_hook
            .lock()
            .expect("lock in_flight_hook")
            .take();
        if let Some(cb) = hook {
            cb();
        }
    }

    /// Validate and record an actual [`SourceCandidate`], [`ExecutedChecks`], and
    /// [`ClosedAdmission`] (`candidate.execution().closed_admission()`) **outside**
    /// Store's writer callback.
    ///
    /// If `candidate.revalidate_custody()` fails on disk, any previously recorded
    /// entry for `candidate.custody_digest()` is revoked (or the catalog latches
    /// fail-closed if revocation cannot complete) before returning
    /// [`NativeEvidenceCatalogError::CustodyDrifted`].
    pub fn record_candidate(
        &self,
        task: &TaskView,
        candidate: &SourceCandidate,
        checks: &ExecutedChecks,
    ) -> Result<PublishedCandidateCustody, NativeEvidenceCatalogError> {
        if self.is_fail_closed() {
            return Err(NativeEvidenceCatalogError::Unavailable);
        }
        if candidate.revalidate_custody().is_err() {
            if self
                .revoke_custody_digest(
                    candidate.custody_digest(),
                    "candidate checkout custody drifted during record_candidate",
                )
                .is_err()
            {
                self.fail_closed.store(true, Ordering::SeqCst);
            }
            return Err(NativeEvidenceCatalogError::CustodyDrifted(
                "candidate checkout custody revalidation failed",
            ));
        }
        let closed = candidate
            .execution()
            .closed_admission()
            .ok_or(NativeEvidenceCatalogError::MissingClosedAdmission)?;
        let decision_evidence = candidate
            .decision_evidence(checks, self.policy.verification.verifier.clone())
            .map_err(|_| {
                NativeEvidenceCatalogError::EvidenceMismatch(
                    "candidate decision_evidence validation failed",
                )
            })?;
        self.record_verified_parts(
            task,
            candidate.project_id(),
            candidate.root_id(),
            candidate.task_id(),
            candidate.attempt_id(),
            candidate.candidate_ref(),
            &candidate.execution().prepared().workspace().workspace,
            candidate.checkout_path(),
            candidate.source_commit_sha(),
            candidate.source_tree_sha(),
            candidate.source_artifact_digest(),
            candidate.custody_digest(),
            candidate.closure_digest(),
            candidate.execution().identity(),
            candidate.execution().observation(),
            decision_evidence,
            checks,
            closed.identity(),
            closed.observation(),
            closed.exit_code(),
            closed.success(),
            closed.closure_digest(),
        )
    }

    /// Alias for [`Self::record_candidate`] used by lifecycle orchestrators.
    pub fn register_verified_evidence(
        &self,
        task: &TaskView,
        candidate: &SourceCandidate,
        checks: &ExecutedChecks,
    ) -> Result<PublishedCandidateCustody, NativeEvidenceCatalogError> {
        self.record_candidate(task, candidate, checks)
    }

    /// Revalidate on-disk Git and descriptor custody of the actual opaque
    /// [`SourceCandidate`] **outside** Store's writer callback. If custody has
    /// drifted, the candidate's custody digest is atomically revoked and
    /// tombstoned (or the catalog latches globally fail-closed if revocation
    /// cannot complete) so subsequent
    /// [`NativeEvidenceAuthority::remains_current`] and
    /// [`NativeEvidenceAuthority::authenticate_evidence`] calls fail closed.
    pub fn revalidate_candidate_or_revoke(
        &self,
        candidate: &SourceCandidate,
    ) -> Result<(), NativeEvidenceCatalogError> {
        self.revalidate_custody_with(
            candidate.custody_digest(),
            &candidate.execution().prepared().workspace().workspace,
            candidate.checkout_path(),
            || {
                candidate
                    .revalidate_custody()
                    .map_err(|_| "candidate checkout custody failed revalidation")
            },
        )
    }

    fn revalidate_custody_with<F>(
        &self,
        custody_digest: &str,
        expected_source_workspace: &Path,
        expected_candidate_checkout: &Path,
        revalidator: F,
    ) -> Result<(), NativeEvidenceCatalogError>
    where
        F: FnOnce() -> Result<(), &'static str>,
    {
        if self.is_fail_closed() {
            return Err(NativeEvidenceCatalogError::Unavailable);
        }
        if !valid_digest(custody_digest) {
            self.fail_closed.store(true, Ordering::SeqCst);
            return Err(NativeEvidenceCatalogError::InvalidPolicy(
                "custody_digest must be a valid sha256 digest",
            ));
        }
        if let Err(drift_reason) = revalidator() {
            if self
                .revoke_custody_digest(custody_digest, drift_reason)
                .is_err()
            {
                self.fail_closed.store(true, Ordering::SeqCst);
            }
            return Err(NativeEvidenceCatalogError::CustodyDrifted(drift_reason));
        }
        if self.is_fail_closed() {
            return Err(NativeEvidenceCatalogError::Unavailable);
        }
        let Ok(mut state) = self.state.try_lock() else {
            self.fail_closed.store(true, Ordering::SeqCst);
            return Err(NativeEvidenceCatalogError::Unavailable);
        };
        if self.is_fail_closed() {
            return Err(NativeEvidenceCatalogError::Unavailable);
        }
        if state.tombstones_by_custody.contains_key(custody_digest) {
            return Err(NativeEvidenceCatalogError::RevokedEvidence(
                "candidate custody digest is already revoked",
            ));
        }
        let Some(record) = state.active_by_custody.get(custody_digest) else {
            return Err(NativeEvidenceCatalogError::CandidateMismatch(
                "candidate custody digest is not active in catalog",
            ));
        };
        if record.source_workspace_path != expected_source_workspace
            || record.candidate_checkout_path != expected_candidate_checkout
        {
            if Self::revoke_in_state(
                &mut state,
                self.limit,
                &self.sequence,
                custody_digest,
                "candidate workspace or checkout path mismatched recorded custody",
            )
            .is_err()
            {
                self.fail_closed.store(true, Ordering::SeqCst);
            }
            return Err(NativeEvidenceCatalogError::CandidateMismatch(
                "candidate workspace or checkout path does not match active record",
            ));
        }
        #[cfg(test)]
        self.fire_in_flight_hook();
        if self.is_fail_closed() {
            return Err(NativeEvidenceCatalogError::Unavailable);
        }
        Ok(())
    }

    /// Explicitly revoke and tombstone a custody digest so it can never
    /// authenticate or remain current again, even on exact replay.
    ///
    /// If the lock is contended or tombstone capacity is exhausted for an
    /// unrecorded digest, the catalog atomically latches into a global
    /// fail-closed state before returning the error.
    pub fn revoke_custody_digest(
        &self,
        custody_digest: &str,
        reason: &str,
    ) -> Result<RevocationTombstone, NativeEvidenceCatalogError> {
        if !valid_digest(custody_digest) || !bounded(reason) {
            self.fail_closed.store(true, Ordering::SeqCst);
            return Err(NativeEvidenceCatalogError::InvalidPolicy(
                "revocation requires valid sha256 custody_digest and bounded reason",
            ));
        }
        let Ok(mut state) = self.state.try_lock() else {
            self.fail_closed.store(true, Ordering::SeqCst);
            return Err(NativeEvidenceCatalogError::Unavailable);
        };
        match Self::revoke_in_state(
            &mut state,
            self.limit,
            &self.sequence,
            custody_digest,
            reason,
        ) {
            Ok(tombstone) => Ok(tombstone),
            Err(err) => {
                self.fail_closed.store(true, Ordering::SeqCst);
                Err(err)
            }
        }
    }

    /// Explicitly revoke any active evidence record for `(task_id, attempt_id)`.
    pub fn revoke_task_attempt(
        &self,
        task_id: &TaskId,
        attempt_id: &AttemptId,
        reason: &str,
    ) -> Result<Option<RevocationTombstone>, NativeEvidenceCatalogError> {
        if !bounded(reason) {
            self.fail_closed.store(true, Ordering::SeqCst);
            return Err(NativeEvidenceCatalogError::InvalidPolicy(
                "revocation reason must be non-empty and bounded",
            ));
        }
        let Ok(mut state) = self.state.try_lock() else {
            self.fail_closed.store(true, Ordering::SeqCst);
            return Err(NativeEvidenceCatalogError::Unavailable);
        };
        let slot = (task_id.clone(), attempt_id.clone());
        let Some(custody_digest) = state.custody_by_attempt.get(&slot).cloned() else {
            return Ok(None);
        };
        match Self::revoke_in_state(
            &mut state,
            self.limit,
            &self.sequence,
            &custody_digest,
            reason,
        ) {
            Ok(tombstone) => Ok(Some(tombstone)),
            Err(err) => {
                self.fail_closed.store(true, Ordering::SeqCst);
                Err(err)
            }
        }
    }

    /// Deterministic lookup of an active (unrevoked) verified evidence summary by `custody_digest`.
    pub fn lookup_by_custody_digest(
        &self,
        custody_digest: &str,
    ) -> Result<Option<VerifiedEvidenceSummary>, NativeEvidenceCatalogError> {
        if self.is_fail_closed() {
            return Err(NativeEvidenceCatalogError::Unavailable);
        }
        let state = self
            .state
            .try_lock()
            .map_err(|_| NativeEvidenceCatalogError::Unavailable)?;
        if self.is_fail_closed() {
            return Err(NativeEvidenceCatalogError::Unavailable);
        }
        if state.tombstones_by_custody.contains_key(custody_digest) {
            return Ok(None);
        }
        let summary = state
            .active_by_custody
            .get(custody_digest)
            .map(CatalogRecord::summary);
        #[cfg(test)]
        self.fire_in_flight_hook();
        if self.is_fail_closed() {
            return Err(NativeEvidenceCatalogError::Unavailable);
        }
        Ok(summary)
    }

    /// Deterministic lookup of the current active (unrevoked) verified evidence
    /// summary for `(task_id, attempt_id)`.
    pub fn lookup_by_task_attempt(
        &self,
        task_id: &TaskId,
        attempt_id: &AttemptId,
    ) -> Result<Option<VerifiedEvidenceSummary>, NativeEvidenceCatalogError> {
        if self.is_fail_closed() {
            return Err(NativeEvidenceCatalogError::Unavailable);
        }
        let state = self
            .state
            .try_lock()
            .map_err(|_| NativeEvidenceCatalogError::Unavailable)?;
        if self.is_fail_closed() {
            return Err(NativeEvidenceCatalogError::Unavailable);
        }
        let slot = (task_id.clone(), attempt_id.clone());
        let Some(custody_digest) = state.custody_by_attempt.get(&slot) else {
            return Ok(None);
        };
        if state.tombstones_by_custody.contains_key(custody_digest) {
            return Ok(None);
        }
        let summary = state
            .active_by_custody
            .get(custody_digest)
            .map(CatalogRecord::summary);
        #[cfg(test)]
        self.fire_in_flight_hook();
        if self.is_fail_closed() {
            return Err(NativeEvidenceCatalogError::Unavailable);
        }
        Ok(summary)
    }

    /// Deterministic lookup of revocation tombstone status for `custody_digest`.
    pub fn revocation_status(
        &self,
        custody_digest: &str,
    ) -> Result<Option<RevocationTombstone>, NativeEvidenceCatalogError> {
        let state = self
            .state
            .try_lock()
            .map_err(|_| NativeEvidenceCatalogError::Unavailable)?;
        Ok(state.tombstones_by_custody.get(custody_digest).cloned())
    }

    /// Number of currently active (unrevoked) verified records in the catalog.
    pub fn active_count(&self) -> Result<usize, NativeEvidenceCatalogError> {
        let state = self
            .state
            .try_lock()
            .map_err(|_| NativeEvidenceCatalogError::Unavailable)?;
        Ok(state.active_by_custody.len())
    }

    /// Number of permanent revocation tombstones retained in the catalog.
    pub fn tombstone_count(&self) -> Result<usize, NativeEvidenceCatalogError> {
        let state = self
            .state
            .try_lock()
            .map_err(|_| NativeEvidenceCatalogError::Unavailable)?;
        Ok(state.tombstones_by_custody.len())
    }

    #[allow(clippy::too_many_arguments)]
    fn record_verified_parts(
        &self,
        task: &TaskView,
        candidate_project_id: &ProjectId,
        candidate_root_id: &RootId,
        candidate_task_id: &TaskId,
        candidate_attempt_id: &AttemptId,
        candidate_ref_str: &str,
        source_workspace_path: &Path,
        candidate_checkout_path: &Path,
        candidate_commit_sha: &str,
        candidate_tree_sha: &str,
        candidate_artifact_digest: &str,
        candidate_custody_digest: &str,
        candidate_closure_digest: &str,
        execution_identity: &RecoveryIdentity,
        execution_observation: &EnforcementObservation,
        decision_evidence: CandidateDecisionEvidence,
        checks: &ExecutedChecks,
        closed_identity: &RecoveryIdentity,
        closed_observation: &EnforcementObservation,
        closed_exit_code: Option<i32>,
        closed_success: bool,
        closed_closure_digest: &str,
    ) -> Result<PublishedCandidateCustody, NativeEvidenceCatalogError> {
        if self.is_fail_closed() {
            return Err(NativeEvidenceCatalogError::Unavailable);
        }
        if task.contract.simulated
            || task.state != SchedulingState::AwaitingVerification
            || task.contract.project_id != self.policy.project_id
            || task.contract.root_id != self.policy.root_id
            || task.contract.authority_ref != self.policy.authority_ref
        {
            return Err(NativeEvidenceCatalogError::CandidateMismatch(
                "task state, simulation flag, or policy scope mismatch",
            ));
        }
        let worker = task
            .owner
            .clone()
            .ok_or(NativeEvidenceCatalogError::CandidateMismatch(
                "task awaiting verification has no worker owner",
            ))?;
        if worker == self.policy.verification.verifier {
            return Err(NativeEvidenceCatalogError::CandidateMismatch(
                "task worker cannot be the independent verifier",
            ));
        }
        let attempt_id =
            task.attempt_id
                .clone()
                .ok_or(NativeEvidenceCatalogError::CandidateMismatch(
                    "task awaiting verification has no attempt_id",
                ))?;
        let task_candidate_ref = task.candidate_ref.clone().filter(|v| bounded(v)).ok_or(
            NativeEvidenceCatalogError::CandidateMismatch(
                "task awaiting verification has missing or invalid candidate_ref",
            ),
        )?;

        if candidate_project_id != &task.contract.project_id
            || candidate_root_id != &task.contract.root_id
            || candidate_task_id != &task.contract.task_id
            || candidate_attempt_id != &attempt_id
            || candidate_ref_str != task_candidate_ref
            || !source_workspace_path.is_absolute()
            || !candidate_checkout_path.is_absolute()
            || source_workspace_path == candidate_checkout_path
            || !valid_git_oid(candidate_commit_sha)
            || !valid_git_oid(candidate_tree_sha)
            || !valid_digest(candidate_artifact_digest)
            || !valid_digest(candidate_custody_digest)
            || !valid_digest(candidate_closure_digest)
        {
            return Err(NativeEvidenceCatalogError::CandidateMismatch(
                "SourceCandidate identity, paths, or digest fields do not match task",
            ));
        }

        if !closed_success
            || closed_exit_code != Some(0)
            || closed_closure_digest != candidate_closure_digest
            || closed_identity != execution_identity
            || !observations_equal(closed_observation, execution_observation)
        {
            return Err(NativeEvidenceCatalogError::EvidenceMismatch(
                "ClosedAdmission does not match candidate execution or did not exit 0",
            ));
        }
        validate_closed_observations(
            closed_identity,
            closed_observation,
            &task.contract.project_id,
            &task.contract.task_id,
            &attempt_id,
            source_workspace_path,
            candidate_checkout_path,
        )?;

        if !valid_digest(checks.execution_digest()) {
            return Err(NativeEvidenceCatalogError::EvidenceMismatch(
                "ExecutedChecks execution_digest is not a valid sha256 digest",
            ));
        }
        let contract_digest = content_digest(&task.contract).map_err(|_| {
            NativeEvidenceCatalogError::EvidenceMismatch("failed to digest task contract")
        })?;
        let policy_digest = content_digest(&self.policy.verification).map_err(|_| {
            NativeEvidenceCatalogError::EvidenceMismatch("failed to digest verification policy")
        })?;
        let inputs = &checks.receipt().inputs;
        if checks.receipt().producer != ReceiptProducer::IndependentVerifier
            || checks.receipt().verifier != self.policy.verification.verifier
            || inputs.project_id != task.contract.project_id
            || inputs.root_id != task.contract.root_id
            || inputs.task_id != task.contract.task_id
            || inputs.attempt_id != attempt_id
            || inputs.candidate_ref != task_candidate_ref
            || inputs.contract_revision != task.contract.revision
            || inputs.contract_digest != contract_digest
            || inputs.verification_policy_digest != policy_digest
            || inputs.protected_oracle_digest != self.policy.verification.protected_oracle_digest
            || inputs.source_commit_sha != candidate_commit_sha
            || inputs.source_tree_sha != candidate_tree_sha
            || inputs.source_artifact_digest != candidate_artifact_digest
        {
            return Err(NativeEvidenceCatalogError::EvidenceMismatch(
                "ExecutedChecks receipt inputs or verifier do not match candidate and policy",
            ));
        }
        let validated = validate_receipt(
            &task.contract,
            inputs,
            &self.policy.verification,
            checks.receipt(),
        )
        .map_err(|_| {
            NativeEvidenceCatalogError::EvidenceMismatch("verification receipt validation failed")
        })?;
        if !validated.all_checks_passed() {
            return Err(NativeEvidenceCatalogError::EvidenceMismatch(
                "verification receipt did not pass all checks",
            ));
        }
        if decision_evidence.contract_digest != contract_digest
            || decision_evidence.verification_policy_digest != policy_digest
            || decision_evidence.receipt_digest != validated.receipt_digest()
            || decision_evidence.custody_digest != candidate_custody_digest
            || decision_evidence.closure_digest != candidate_closure_digest
            || decision_evidence.source_commit_sha != candidate_commit_sha
            || decision_evidence.source_tree_sha != candidate_tree_sha
            || decision_evidence.verifier != self.policy.verification.verifier
        {
            return Err(NativeEvidenceCatalogError::EvidenceMismatch(
                "CandidateDecisionEvidence does not match validated receipt and policy",
            ));
        }

        let binding = NativeDecisionBinding {
            project_id: task.contract.project_id.clone(),
            root_id: task.contract.root_id.clone(),
            task_id: task.contract.task_id.clone(),
            contract_digest: contract_digest.clone(),
            attempt_id: attempt_id.clone(),
            worker,
            verifier: self.policy.verification.verifier.clone(),
            authority_ref: task.contract.authority_ref.clone(),
            policy_digest,
            checks_execution_digest: checks.execution_digest().to_owned(),
            receipt_digest: validated.receipt_digest().to_owned(),
            closure_digest: closed_closure_digest.to_owned(),
            source_artifact_digest: candidate_artifact_digest.to_owned(),
            protected_oracle_digest: self.policy.verification.protected_oracle_digest.clone(),
            candidate_ref: task_candidate_ref,
            source_commit_sha: candidate_commit_sha.to_owned(),
            source_tree_sha: candidate_tree_sha.to_owned(),
        };
        let proof_key = content_digest(&(
            &binding.checks_execution_digest,
            &binding.closure_digest,
            &binding.contract_digest,
        ))
        .map_err(|_| NativeEvidenceCatalogError::EvidenceMismatch("failed to digest proof_key"))?;
        let publication_digest = content_digest(&(
            "native_evidence_catalog_publication_v1",
            candidate_custody_digest,
            &proof_key,
            (
                &binding.project_id,
                &binding.root_id,
                &binding.task_id,
                &binding.attempt_id,
                &binding.worker,
                &binding.verifier,
                &binding.authority_ref,
            ),
            (
                &binding.policy_digest,
                &binding.receipt_digest,
                &binding.source_artifact_digest,
                &binding.protected_oracle_digest,
                &binding.candidate_ref,
                &binding.source_commit_sha,
                &binding.source_tree_sha,
            ),
        ))
        .map_err(|_| {
            NativeEvidenceCatalogError::EvidenceMismatch("failed to digest publication record")
        })?;

        if self.is_fail_closed() {
            return Err(NativeEvidenceCatalogError::Unavailable);
        }
        let mut state = self
            .state
            .try_lock()
            .map_err(|_| NativeEvidenceCatalogError::Unavailable)?;
        if self.is_fail_closed() {
            return Err(NativeEvidenceCatalogError::Unavailable);
        }

        if state
            .tombstones_by_custody
            .contains_key(candidate_custody_digest)
            || state.revoked_proof_keys.contains_key(&proof_key)
            || state
                .revoked_closure_digests
                .contains_key(closed_closure_digest)
            || state
                .revoked_check_digests
                .contains_key(checks.execution_digest())
        {
            return Err(NativeEvidenceCatalogError::RevokedEvidence(
                "cannot register or replay previously revoked or superseded evidence",
            ));
        }

        if let Some(existing) = state.active_by_custody.get(candidate_custody_digest) {
            if existing.proof_key == proof_key
                && existing.source_workspace_path == source_workspace_path
                && existing.candidate_checkout_path == candidate_checkout_path
                && bindings_equal(&existing.binding, &binding)
                && &existing.executed_checks == checks
                && &existing.closed_identity == closed_identity
                && observations_equal(&existing.closed_observation, closed_observation)
                && existing.closed_exit_code == closed_exit_code
                && existing.closed_success == closed_success
            {
                let published = PublishedCandidateCustody {
                    custody_digest: existing.custody_digest.clone(),
                    publication_digest: existing.publication_digest.clone(),
                    proof_key: existing.proof_key.clone(),
                    source_workspace_path: existing.source_workspace_path.clone(),
                    candidate_checkout_path: existing.candidate_checkout_path.clone(),
                    binding: existing.binding.clone(),
                    decision_evidence: existing.decision_evidence.clone(),
                    recorded_sequence: existing.recorded_sequence,
                };
                #[cfg(test)]
                self.fire_in_flight_hook();
                if self.is_fail_closed() {
                    return Err(NativeEvidenceCatalogError::Unavailable);
                }
                return Ok(published);
            }
            return Err(NativeEvidenceCatalogError::RecordConflict(
                "conflicting evidence presented for existing custody_digest",
            ));
        }

        if state.custody_by_proof_key.contains_key(&proof_key) {
            return Err(NativeEvidenceCatalogError::RecordConflict(
                "proof_key is already bound to a different custody_digest",
            ));
        }

        // Reserve one tombstone slot for every active record: total occupied
        // slots (active + tombstones) must never exceed `self.limit`.
        // Superseding an existing active record converts 1 active -> 1 tombstone
        // and adds 1 new active record (+1 net slot).
        let occupied_slots = state
            .active_by_custody
            .len()
            .saturating_add(state.tombstones_by_custody.len());
        if occupied_slots >= self.limit {
            return Err(NativeEvidenceCatalogError::CatalogFull);
        }

        let attempt_slot = (binding.task_id.clone(), binding.attempt_id.clone());
        if let Some(prior_custody) = state.custody_by_attempt.get(&attempt_slot).cloned()
            && Self::revoke_in_state(
                &mut state,
                self.limit,
                &self.sequence,
                &prior_custody,
                "superseded by newer candidate evidence for task attempt",
            )
            .is_err()
        {
            self.fail_closed.store(true, Ordering::SeqCst);
            return Err(NativeEvidenceCatalogError::Unavailable);
        }

        let recorded_sequence = self.sequence.fetch_add(1, Ordering::Relaxed);
        let record = CatalogRecord {
            custody_digest: candidate_custody_digest.to_owned(),
            publication_digest: publication_digest.clone(),
            proof_key: proof_key.clone(),
            source_workspace_path: source_workspace_path.to_path_buf(),
            candidate_checkout_path: candidate_checkout_path.to_path_buf(),
            binding: binding.clone(),
            decision_evidence: decision_evidence.clone(),
            executed_checks: checks.clone(),
            closed_identity: closed_identity.clone(),
            closed_observation: closed_observation.clone(),
            closed_exit_code,
            closed_success,
            recorded_sequence,
        };
        state
            .custody_by_proof_key
            .insert(proof_key.clone(), candidate_custody_digest.to_owned());
        state
            .custody_by_attempt
            .insert(attempt_slot.clone(), candidate_custody_digest.to_owned());
        state
            .active_by_custody
            .insert(candidate_custody_digest.to_owned(), record);

        #[cfg(test)]
        self.fire_in_flight_hook();
        if self.is_fail_closed() {
            state.active_by_custody.remove(candidate_custody_digest);
            state.custody_by_proof_key.remove(&proof_key);
            state.custody_by_attempt.remove(&attempt_slot);
            return Err(NativeEvidenceCatalogError::Unavailable);
        }

        Ok(PublishedCandidateCustody {
            custody_digest: candidate_custody_digest.to_owned(),
            publication_digest,
            proof_key,
            source_workspace_path: source_workspace_path.to_path_buf(),
            candidate_checkout_path: candidate_checkout_path.to_path_buf(),
            binding,
            decision_evidence,
            recorded_sequence,
        })
    }

    fn revoke_in_state(
        state: &mut CatalogState,
        limit: usize,
        sequence: &AtomicU64,
        custody_digest: &str,
        reason: &str,
    ) -> Result<RevocationTombstone, NativeEvidenceCatalogError> {
        if let Some(existing) = state.tombstones_by_custody.get(custody_digest) {
            return Ok(existing.clone());
        }
        // Remove any active record FIRST so revoking a live record never fails
        // due to capacity; only revoking an unrecorded digest checks slot bound.
        let removed = state.active_by_custody.remove(custody_digest);
        if removed.is_none() {
            let occupied_slots = state
                .active_by_custody
                .len()
                .saturating_add(state.tombstones_by_custody.len());
            if occupied_slots >= limit {
                return Err(NativeEvidenceCatalogError::CatalogFull);
            }
        }
        let tombstone = if let Some(record) = removed {
            state.custody_by_proof_key.remove(&record.proof_key);
            let slot = (
                record.binding.task_id.clone(),
                record.binding.attempt_id.clone(),
            );
            if state
                .custody_by_attempt
                .get(&slot)
                .is_some_and(|c| c == custody_digest)
            {
                state.custody_by_attempt.remove(&slot);
            }
            state
                .revoked_proof_keys
                .insert(record.proof_key.clone(), custody_digest.to_owned());
            state.revoked_closure_digests.insert(
                record.binding.closure_digest.clone(),
                custody_digest.to_owned(),
            );
            state.revoked_check_digests.insert(
                record.binding.checks_execution_digest.clone(),
                custody_digest.to_owned(),
            );
            RevocationTombstone {
                custody_digest: custody_digest.to_owned(),
                task_id: Some(record.binding.task_id),
                attempt_id: Some(record.binding.attempt_id),
                candidate_ref: Some(record.binding.candidate_ref),
                proof_key: Some(record.proof_key),
                closure_digest: Some(record.binding.closure_digest),
                checks_execution_digest: Some(record.binding.checks_execution_digest),
                reason: reason.to_owned(),
                revoked_sequence: sequence.fetch_add(1, Ordering::Relaxed),
            }
        } else {
            RevocationTombstone {
                custody_digest: custody_digest.to_owned(),
                task_id: None,
                attempt_id: None,
                candidate_ref: None,
                proof_key: None,
                closure_digest: None,
                checks_execution_digest: None,
                reason: reason.to_owned(),
                revoked_sequence: sequence.fetch_add(1, Ordering::Relaxed),
            }
        };
        state
            .tombstones_by_custody
            .insert(custody_digest.to_owned(), tombstone.clone());
        Ok(tombstone)
    }

    #[allow(clippy::too_many_arguments)]
    fn verify_authenticated_parts(
        &self,
        binding: &NativeDecisionBinding,
        checks: &ExecutedChecks,
        closed_identity: &RecoveryIdentity,
        closed_observation: &EnforcementObservation,
        closed_exit_code: Option<i32>,
        closed_success: bool,
        closed_closure_digest: &str,
    ) -> Option<String> {
        if self.is_fail_closed() || !closed_success || closed_exit_code != Some(0) {
            return None;
        }
        let proof_key = content_digest(&(
            &binding.checks_execution_digest,
            &binding.closure_digest,
            &binding.contract_digest,
        ))
        .ok()?;
        let state = self.state.try_lock().ok()?;
        if self.is_fail_closed() {
            return None;
        }
        if state.revoked_proof_keys.contains_key(&proof_key)
            || state
                .revoked_closure_digests
                .contains_key(closed_closure_digest)
            || state
                .revoked_check_digests
                .contains_key(checks.execution_digest())
        {
            return None;
        }
        let custody_digest = state.custody_by_proof_key.get(&proof_key)?;
        if state.tombstones_by_custody.contains_key(custody_digest) {
            return None;
        }
        let record = state.active_by_custody.get(custody_digest)?;
        let slot = (binding.task_id.clone(), binding.attempt_id.clone());
        if state.custody_by_attempt.get(&slot) != Some(custody_digest) {
            return None;
        }
        if !bindings_equal(&record.binding, binding)
            || checks.execution_digest() != record.binding.checks_execution_digest
            || checks != &record.executed_checks
            || closed_closure_digest != record.binding.closure_digest
            || closed_exit_code != record.closed_exit_code
            || closed_success != record.closed_success
            || closed_identity != &record.closed_identity
            || !observations_equal(closed_observation, &record.closed_observation)
        {
            return None;
        }
        let authenticated_custody = record.custody_digest.clone();
        #[cfg(test)]
        self.fire_in_flight_hook();
        if self.is_fail_closed() {
            return None;
        }
        Some(authenticated_custody)
    }

    /// Component-only test helper for exercising positive catalog authentication,
    /// field-by-field mismatch rejection, revocation, replay, and capacity bounds
    /// without forging a live Linux systemd `ClosedAdmission`. Still requires a
    /// real `TaskView`, distinct `source_workspace_path` and
    /// `candidate_checkout_path`, and real `ExecutedChecks` produced by
    /// `admiralty_runtime::trusted_checks::execute_checks`.
    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub fn record_component_fixture_for_test(
        &self,
        task: &TaskView,
        source_workspace_path: &Path,
        candidate_checkout_path: &Path,
        source_commit_sha: &str,
        source_tree_sha: &str,
        source_artifact_digest: &str,
        custody_digest: &str,
        closure_digest: &str,
        closed_identity: &RecoveryIdentity,
        closed_observation: &EnforcementObservation,
        closed_exit_code: Option<i32>,
        closed_success: bool,
        checks: &ExecutedChecks,
    ) -> Result<PublishedCandidateCustody, NativeEvidenceCatalogError> {
        let receipt_digest = content_digest(checks.receipt())
            .map_err(|_| NativeEvidenceCatalogError::EvidenceMismatch("receipt digest failed"))?;
        let decision_evidence = CandidateDecisionEvidence {
            contract_digest: checks.receipt().inputs.contract_digest.clone(),
            verification_policy_digest: checks.receipt().inputs.verification_policy_digest.clone(),
            receipt_digest,
            custody_digest: custody_digest.to_owned(),
            closure_digest: closure_digest.to_owned(),
            source_commit_sha: source_commit_sha.to_owned(),
            source_tree_sha: source_tree_sha.to_owned(),
            verifier: self.policy.verification.verifier.clone(),
        };
        self.record_verified_parts(
            task,
            &task.contract.project_id,
            &task.contract.root_id,
            &task.contract.task_id,
            task.attempt_id
                .as_ref()
                .ok_or(NativeEvidenceCatalogError::CandidateMismatch(
                    "missing attempt_id",
                ))?,
            task.candidate_ref
                .as_deref()
                .ok_or(NativeEvidenceCatalogError::CandidateMismatch(
                    "missing candidate_ref",
                ))?,
            source_workspace_path,
            candidate_checkout_path,
            source_commit_sha,
            source_tree_sha,
            source_artifact_digest,
            custody_digest,
            closure_digest,
            closed_identity,
            closed_observation,
            decision_evidence,
            checks,
            closed_identity,
            closed_observation,
            closed_exit_code,
            closed_success,
            closure_digest,
        )
    }

    /// Component-only test helper invoking the exact internal verification logic
    /// of [`NativeEvidenceAuthority::authenticate_evidence`] with explicit
    /// `ClosedAdmission` fields.
    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub fn authenticate_parts_for_test(
        &self,
        binding: &NativeDecisionBinding,
        checks: &ExecutedChecks,
        closed_identity: &RecoveryIdentity,
        closed_observation: &EnforcementObservation,
        closed_exit_code: Option<i32>,
        closed_success: bool,
        closed_closure_digest: &str,
    ) -> Option<String> {
        self.verify_authenticated_parts(
            binding,
            checks,
            closed_identity,
            closed_observation,
            closed_exit_code,
            closed_success,
            closed_closure_digest,
        )
    }

    /// Component-only test helper exercising the exact drift revalidation and
    /// fail-closed revocation pipeline of [`Self::revalidate_candidate_or_revoke`].
    #[cfg(test)]
    pub fn revalidate_with_probe_or_revoke_for_test<F>(
        &self,
        custody_digest: &str,
        expected_source_workspace: &Path,
        expected_candidate_checkout: &Path,
        probe: F,
    ) -> Result<(), NativeEvidenceCatalogError>
    where
        F: FnOnce() -> Result<(), &'static str>,
    {
        self.revalidate_custody_with(
            custody_digest,
            expected_source_workspace,
            expected_candidate_checkout,
            probe,
        )
    }

    /// Component-only test helper to simulate mutex contention during revocation
    /// or drift revalidation.
    #[cfg(test)]
    pub fn with_locked_state_for_test<R>(&self, f: impl FnOnce() -> R) -> R {
        let _guard = self.state.lock().expect("lock state for test");
        f()
    }

    /// Component-only test helper to install a one-shot hook executed while a
    /// catalog callback or publication/revalidation method holds the `state`
    /// mutex guard, immediately before the final `is_fail_closed()` recheck.
    #[cfg(test)]
    pub fn set_in_flight_hook_for_test(&self, hook: impl Fn() + Send + Sync + 'static) {
        *self.in_flight_hook.lock().expect("lock in_flight_hook") = Some(Box::new(hook));
    }

    /// Component-only test helper to seed an active record alongside a full
    /// tombstone table (`tombstones == limit`) to verify that revoking an active
    /// record STILL succeeds and removes live authority even when tombstones are
    /// already at `limit`.
    #[cfg(test)]
    pub fn force_insert_tombstone_for_test(&self, custody_digest: &str, reason: &str) {
        let mut state = self.state.lock().expect("lock state for test");
        state.tombstones_by_custody.insert(
            custody_digest.to_owned(),
            RevocationTombstone {
                custody_digest: custody_digest.to_owned(),
                task_id: None,
                attempt_id: None,
                candidate_ref: None,
                proof_key: None,
                closure_digest: None,
                checks_execution_digest: None,
                reason: reason.to_owned(),
                revoked_sequence: self.sequence.fetch_add(1, Ordering::Relaxed),
            },
        );
    }
}

impl NativeEvidenceAuthority for NativeEvidenceCatalog {
    fn authenticate_evidence(
        &self,
        binding: &NativeDecisionBinding,
        checks: &ExecutedChecks,
        closed: &ClosedAdmission,
    ) -> Option<String> {
        self.verify_authenticated_parts(
            binding,
            checks,
            closed.identity(),
            closed.observation(),
            closed.exit_code(),
            closed.success(),
            closed.closure_digest(),
        )
    }

    fn remains_current(&self, binding: &NativeDecisionBinding, custody_digest: &str) -> bool {
        if self.is_fail_closed() || !valid_digest(custody_digest) {
            return false;
        }
        let Ok(state) = self.state.try_lock() else {
            return false;
        };
        if self.is_fail_closed() {
            return false;
        }
        if state.tombstones_by_custody.contains_key(custody_digest) {
            return false;
        }
        let Some(record) = state.active_by_custody.get(custody_digest) else {
            return false;
        };
        if state.revoked_proof_keys.contains_key(&record.proof_key)
            || state
                .revoked_closure_digests
                .contains_key(&record.binding.closure_digest)
            || state
                .revoked_check_digests
                .contains_key(&record.binding.checks_execution_digest)
        {
            return false;
        }
        let slot = (binding.task_id.clone(), binding.attempt_id.clone());
        if state.custody_by_attempt.get(&slot).map(String::as_str) != Some(custody_digest)
            || !bindings_equal(&record.binding, binding)
        {
            return false;
        }
        #[cfg(test)]
        self.fire_in_flight_hook();
        !self.is_fail_closed()
    }
}

fn validate_closed_observations(
    identity: &RecoveryIdentity,
    observation: &EnforcementObservation,
    expected_project_id: &ProjectId,
    expected_task_id: &TaskId,
    expected_attempt_id: &AttemptId,
    expected_source_workspace_path: &Path,
    candidate_checkout_path: &Path,
) -> Result<(), NativeEvidenceCatalogError> {
    if &identity.identity.project_id != expected_project_id
        || &identity.identity.task_id != expected_task_id
        || &identity.identity.attempt_id != expected_attempt_id
        || !bounded(identity.identity.node_id.as_str())
        || !bounded(&identity.identity.lease_id)
        || !valid_digest(&identity.launch_spec_digest)
        || !valid_digest(&identity.worker_environment_digest)
        || !bounded(&identity.unit)
        || !bounded(&identity.run_id)
        || !bounded(&identity.permit_id)
        || !bounded(&identity.reservation_ref)
        || !bounded(&identity.host_incarnation)
        || !valid_digest(&identity.backend_config_digest)
    {
        return Err(NativeEvidenceCatalogError::EvidenceMismatch(
            "ClosedAdmission RecoveryIdentity fields invalid or misaligned with candidate",
        ));
    }
    if observation.unit != identity.unit
        || !bounded(&observation.invocation_id)
        || observation.main_pid == 0
        || !bounded(&observation.process_start)
        || observation.process_ids.is_empty()
        || observation.cpu_set.is_empty()
        || observation.memory_max_bytes == 0
        || observation.tasks_max == 0
        || !observation.actual_executable.is_absolute()
        || observation.actual_cwd != expected_source_workspace_path
        || observation.actual_cwd == candidate_checkout_path
        || !valid_digest(&observation.actual_argv_digest)
        || !valid_digest(&observation.actual_environment_digest)
        || observation.cgroup_path.as_os_str().is_empty()
        || identity.invocation_id.as_deref() != Some(observation.invocation_id.as_str())
        || identity.main_pid != Some(observation.main_pid)
        || identity.process_start.as_deref() != Some(observation.process_start.as_str())
    {
        return Err(NativeEvidenceCatalogError::EvidenceMismatch(
            "ClosedAdmission EnforcementObservation fields invalid or misaligned with source workspace",
        ));
    }
    Ok(())
}

fn observations_equal(a: &EnforcementObservation, b: &EnforcementObservation) -> bool {
    a.unit == b.unit
        && a.invocation_id == b.invocation_id
        && a.main_pid == b.main_pid
        && a.process_start == b.process_start
        && a.process_ids == b.process_ids
        && a.cpu_set == b.cpu_set
        && a.memory_max_bytes == b.memory_max_bytes
        && a.tasks_max == b.tasks_max
        && a.actual_executable == b.actual_executable
        && a.actual_cwd == b.actual_cwd
        && a.actual_argv_digest == b.actual_argv_digest
        && a.actual_environment_digest == b.actual_environment_digest
        && a.cgroup_path == b.cgroup_path
}

fn bindings_equal(a: &NativeDecisionBinding, b: &NativeDecisionBinding) -> bool {
    a.project_id == b.project_id
        && a.root_id == b.root_id
        && a.task_id == b.task_id
        && a.contract_digest == b.contract_digest
        && a.attempt_id == b.attempt_id
        && a.worker == b.worker
        && a.verifier == b.verifier
        && a.authority_ref == b.authority_ref
        && a.policy_digest == b.policy_digest
        && a.checks_execution_digest == b.checks_execution_digest
        && a.receipt_digest == b.receipt_digest
        && a.closure_digest == b.closure_digest
        && a.source_artifact_digest == b.source_artifact_digest
        && a.protected_oracle_digest == b.protected_oracle_digest
        && a.candidate_ref == b.candidate_ref
        && a.source_commit_sha == b.source_commit_sha
        && a.source_tree_sha == b.source_tree_sha
}

fn bounded(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}

fn valid_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|v| {
        v.len() == 64
            && v.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

fn valid_git_oid(value: &str) -> bool {
    (value.len() == 40 || value.len() == 64)
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[cfg(test)]
mod unit_tests {
    use super::*;
    use admiralty_core::{
        CheckSpec,
        verification::{RequiredNegativeCheck, VerificationPolicy},
    };

    #[test]
    fn unit_catalog_constructs_and_enforces_bounds() {
        let policy = NativeLifecyclePolicy {
            project_id: ProjectId::new("project").expect("project"),
            root_id: RootId::new("root").expect("root"),
            authority_ref: "grant:project".into(),
            source_producer: "trusted-verifier-service".into(),
            source_incarnation: "service-boot".into(),
            verification: VerificationPolicy {
                schema_version: 1,
                verifier: PrincipalId::new("independent-verifier").expect("verifier"),
                protected_oracle_digest: content_digest(&"oracle").expect("digest"),
                required_negative_checks: vec![RequiredNegativeCheck {
                    check: CheckSpec {
                        id: "neg".into(),
                        program: "git".into(),
                        args: vec!["status".into()],
                        cwd: ".".into(),
                        timeout_secs: 5,
                    },
                    expected_exit_code: 128,
                    fixture_digest: content_digest(&"fixture").expect("digest"),
                }],
            },
        };
        let catalog = NativeEvidenceCatalog::new(policy, 4).expect("catalog");
        assert_eq!(catalog.limit(), 4);
        assert!(!catalog.is_fail_closed());
    }
}
