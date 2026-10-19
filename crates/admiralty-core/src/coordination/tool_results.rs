//! Deterministic structured tool/test-result summaries and failed-approach memory.
//!
//! This module compiles and validates bounded machine-readable check summaries,
//! lossless repeated-diagnostic groupings with byte-offset artifact references,
//! and task-scoped failed-approach records.
//!
//! Invariants:
//! - Caller-supplied check claims never become trusted execution evidence.
//! - Original artifact digests and byte-offset ranges are preserved; raw range
//!   retrieval remains a separately scoped custody/API operation.
//! - Distinct attempts, candidate refs, source commits, or intervening state
//!   transitions (such as an intervening pass, timeout, or parser gap) are never
//!   collapsed together.
//! - Protected facts (negations, units, authority, contract revision, and scope)
//!   are never truncated or silently dropped; absent measurements are never
//!   zero-filled; byte size estimates never claim tokenizer savings; and
//!   summaries never authorize a retry or model escalation.

use crate::coordination::envelope::{ContextEpoch, Sha256Digest};
use crate::economics::{DecimalU64, DecimalU128};
use crate::limits::bounded_json_size;
use crate::{
    AttemptId, CheckSpec, PrincipalId, ProjectId, RootId, TaskContract, TaskId, content_digest,
    safe_relative_path,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::io::Write;

pub const STRUCTURED_RESULT_PROTOCOL_VERSION: u32 = 1;
pub const MAX_STRUCTURED_RESULT_BYTES: usize = 64 * 1024;
pub const MAX_STRUCTURED_RESULT_DEPTH: usize = 16;
pub const MAX_CHECK_SUMMARIES: usize = 16;
pub const MAX_DIAGNOSTICS_PER_CHECK: usize = 32;
pub const MAX_OCCURRENCE_REFS_PER_GROUP: usize = 16;
pub const MAX_FAILED_APPROACHES: usize = 16;
pub const MAX_APPROACH_EVIDENCE_ITEMS: usize = 16;
pub const MAX_UNCERTAINTY_ITEMS: usize = 16;
pub const MAX_EXCERPT_BYTES: usize = 1024;
pub const MAX_TEXT_FIELD_BYTES: usize = 4096;
pub const MAX_PATH_BYTES: usize = 512;
pub const MAX_ARTIFACT_SIZE_BYTES: u64 = 64 * 1024 * 1024;

const NEGATION_TOKENS: &[&str] = &[
    "not", "no", "never", "without", "cannot", "forbid", "reject", "deny", "<=", ">=", "!=",
];

const UNIT_TOKENS: &[&str] = &[
    "ms", "ns", "us", "s", "sec", "secs", "seconds", "b", "bytes", "kib", "mib", "gib", "kb", "mb",
    "gb", "tokens", "usd", "%",
];

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum StructuredResultError {
    #[error("structured result exceeds total byte, depth, item, or reference bounds")]
    BoundExceeded,
    #[error("invalid structured result token budget")]
    InvalidBudget,
    #[error("structured result JSON is malformed, duplicated, unknown, or unsupported")]
    Json,
    #[error("invalid structured result field: {0}")]
    InvalidField(&'static str),
    #[error("malformed artifact or source reference: {0}")]
    MalformedReference(&'static str),
    #[error("incompatible source, attempt, candidate, or contract identity: {0}")]
    IncompatibleSourceIdentity(&'static str),
    #[error("protected fact omitted or altered: {0}")]
    ProtectedFactOmitted(&'static str),
    #[error("untrusted claim or summary cannot assert authority, proof, or savings: {0}")]
    FalseAuthorityClaim(&'static str),
    #[error("compacted or lost baseline requires a fresh complete capsule: {reason}")]
    FreshCapsuleRequired { reason: String },
    #[error("structured result capsule does not match independently retained bindings: {0}")]
    BindingMismatch(&'static str),
}

/// Content-addressed byte range into an immutable raw log or artifact.
/// Possession of this reference never establishes filesystem or Store custody.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ArtifactByteRangeRef {
    pub artifact_digest: Sha256Digest,
    pub artifact_size_bytes: DecimalU64,
    pub offset_bytes: DecimalU64,
    pub length_bytes: DecimalU64,
}

impl ArtifactByteRangeRef {
    pub fn validate(&self) -> Result<(), StructuredResultError> {
        let total = self.artifact_size_bytes.as_u64();
        let offset = self.offset_bytes.as_u64();
        let length = self.length_bytes.as_u64();
        if total == 0 || total > MAX_ARTIFACT_SIZE_BYTES {
            return Err(StructuredResultError::MalformedReference(
                "artifact_size_bytes must be within 1..=MAX_ARTIFACT_SIZE_BYTES",
            ));
        }
        if length == 0 {
            return Err(StructuredResultError::MalformedReference(
                "length_bytes must be nonzero",
            ));
        }
        let Some(end) = offset.checked_add(length) else {
            return Err(StructuredResultError::MalformedReference(
                "artifact byte range offset + length overflow",
            ));
        };
        if end > total {
            return Err(StructuredResultError::MalformedReference(
                "artifact byte range exceeds artifact_size_bytes",
            ));
        }
        Ok(())
    }
}

/// Explicit provenance of a check observation. Neither variant grants authority
/// or substitutes for caller-enforced custody and receipt verification.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "origin", rename_all = "snake_case", deny_unknown_fields)]
pub enum ResultProvenance {
    IndependentVerifier {
        receipt_id: String,
        receipt_digest: Sha256Digest,
        verifier: PrincipalId,
        trusted_execution_proof: bool,
    },
    UntrustedCallerClaim {
        claimant: PrincipalId,
        claim_ref: String,
        trusted_execution_proof: bool,
    },
}

impl ResultProvenance {
    pub fn validate(&self) -> Result<(), StructuredResultError> {
        match self {
            Self::IndependentVerifier {
                receipt_id,
                trusted_execution_proof,
                ..
            } => {
                if *trusted_execution_proof {
                    return Err(StructuredResultError::FalseAuthorityClaim(
                        "wire provenance cannot self-certify trusted_execution_proof",
                    ));
                }
                validate_graphic_id(receipt_id, "receipt_id")
            }
            Self::UntrustedCallerClaim {
                claim_ref,
                trusted_execution_proof,
                ..
            } => {
                if *trusted_execution_proof {
                    return Err(StructuredResultError::FalseAuthorityClaim(
                        "caller claim never becomes trusted execution evidence",
                    ));
                }
                validate_graphic_id(claim_ref, "claim_ref")
            }
        }
    }

    pub const fn is_trusted_execution_evidence(&self) -> bool {
        false
    }
}

/// Explicit measurement that refuses silent zero-filling when a metric is absent.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "observation", rename_all = "snake_case", deny_unknown_fields)]
pub enum MeasurementObservation {
    Measured {
        metric: String,
        value: DecimalU128,
        unit: String,
    },
    Absent {
        metric: String,
        reason: String,
    },
}

impl MeasurementObservation {
    pub fn validate(&self) -> Result<(), StructuredResultError> {
        match self {
            Self::Measured { metric, unit, .. } => {
                validate_non_empty_text(metric, MAX_PATH_BYTES, "measurement metric")?;
                validate_non_empty_text(unit, 64, "measurement unit")
            }
            Self::Absent { metric, reason } => {
                validate_non_empty_text(metric, MAX_PATH_BYTES, "measurement metric")?;
                validate_non_empty_text(reason, MAX_EXCERPT_BYTES, "absent measurement reason")
            }
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DiagnosticTransition {
    CompilerError,
    TestAssertionFailure,
    QualityFinding,
    InterveningPass,
    Timeout,
    PartialOutput,
    UnknownFormat { parser_gap: String },
}

impl DiagnosticTransition {
    pub fn validate(&self) -> Result<(), StructuredResultError> {
        if let Self::UnknownFormat { parser_gap } = self {
            validate_non_empty_text(parser_gap, MAX_EXCERPT_BYTES, "parser_gap")?;
        }
        Ok(())
    }

    fn is_collapsible_failure(&self) -> bool {
        matches!(
            self,
            Self::CompilerError | Self::TestAssertionFailure | Self::QualityFinding
        )
    }
}

/// Single observed diagnostic or transition event prior to lossless grouping.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticObservation {
    pub sequence: DecimalU64,
    pub attempt_id: AttemptId,
    pub candidate_ref: String,
    pub source_commit_sha: String,
    pub check_id: String,
    pub transition: DiagnosticTransition,
    pub fingerprint: String,
    pub assertion_or_code: Option<String>,
    pub file_path: Option<String>,
    pub line_number: Option<DecimalU64>,
    pub environment: String,
    pub excerpt: String,
    pub raw_range: ArtifactByteRangeRef,
}

impl DiagnosticObservation {
    pub fn validate(&self) -> Result<(), StructuredResultError> {
        if self.sequence.as_u64() == 0 {
            return Err(StructuredResultError::InvalidField(
                "diagnostic sequence must be nonzero",
            ));
        }
        validate_graphic_id(&self.candidate_ref, "candidate_ref")?;
        validate_git_sha(&self.source_commit_sha, "source_commit_sha")?;
        validate_graphic_id(&self.check_id, "check_id")?;
        self.transition.validate()?;
        validate_graphic_id(&self.fingerprint, "fingerprint")?;
        if let Some(code) = &self.assertion_or_code {
            validate_non_empty_text(code, MAX_EXCERPT_BYTES, "assertion_or_code")?;
        }
        if let Some(path) = &self.file_path
            && (path.len() > MAX_PATH_BYTES || !safe_relative_path(path))
        {
            return Err(StructuredResultError::MalformedReference(
                "diagnostic file_path must be a bounded safe relative path",
            ));
        }
        if let Some(line) = &self.line_number
            && line.as_u64() == 0
        {
            return Err(StructuredResultError::InvalidField(
                "line_number must be nonzero when present",
            ));
        }
        validate_non_empty_text(&self.environment, MAX_PATH_BYTES, "environment")?;
        validate_non_empty_text(&self.excerpt, MAX_EXCERPT_BYTES, "excerpt")?;
        self.raw_range.validate()
    }
}

/// Losslessly grouped diagnostic entry retaining exact repeat count, sequence
/// bounds, and bounded original byte-range references.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticGroup {
    pub first_sequence: DecimalU64,
    pub last_sequence: DecimalU64,
    pub check_id: String,
    pub transition: DiagnosticTransition,
    pub fingerprint: String,
    pub assertion_or_code: Option<String>,
    pub file_path: Option<String>,
    pub line_number: Option<DecimalU64>,
    pub environment: String,
    pub excerpt: String,
    pub repeat_count: DecimalU128,
    pub first_occurrence: ArtifactByteRangeRef,
    pub last_occurrence: ArtifactByteRangeRef,
    pub occurrences: Vec<ArtifactByteRangeRef>,
}

impl DiagnosticGroup {
    pub fn validate(&self, expected_check_id: &str) -> Result<(), StructuredResultError> {
        if self.first_sequence.as_u64() == 0
            || self.last_sequence.as_u64() < self.first_sequence.as_u64()
        {
            return Err(StructuredResultError::InvalidField(
                "diagnostic group sequence range is invalid",
            ));
        }
        if self.check_id != expected_check_id {
            return Err(StructuredResultError::IncompatibleSourceIdentity(
                "diagnostic group check_id differs from enclosing check summary",
            ));
        }
        self.transition.validate()?;
        validate_graphic_id(&self.fingerprint, "fingerprint")?;
        if let Some(code) = &self.assertion_or_code {
            validate_non_empty_text(code, MAX_EXCERPT_BYTES, "assertion_or_code")?;
        }
        if let Some(path) = &self.file_path
            && (path.len() > MAX_PATH_BYTES || !safe_relative_path(path))
        {
            return Err(StructuredResultError::MalformedReference(
                "diagnostic file_path must be a bounded safe relative path",
            ));
        }
        if let Some(line) = &self.line_number
            && line.as_u64() == 0
        {
            return Err(StructuredResultError::InvalidField(
                "line_number must be nonzero when present",
            ));
        }
        validate_non_empty_text(&self.environment, MAX_PATH_BYTES, "environment")?;
        validate_non_empty_text(&self.excerpt, MAX_EXCERPT_BYTES, "excerpt")?;
        let count = self.repeat_count.as_u128();
        if count == 0 {
            return Err(StructuredResultError::InvalidField(
                "diagnostic repeat_count must be nonzero",
            ));
        }
        if self.occurrences.is_empty() || self.occurrences.len() > MAX_OCCURRENCE_REFS_PER_GROUP {
            return Err(StructuredResultError::BoundExceeded);
        }
        if !self.transition.is_collapsible_failure() && (count != 1 || self.occurrences.len() != 1)
        {
            return Err(StructuredResultError::InvalidField(
                "intervening pass, timeout, partial, or unknown transitions cannot be collapsed",
            ));
        }
        if count <= MAX_OCCURRENCE_REFS_PER_GROUP as u128 && self.occurrences.len() as u128 != count
        {
            return Err(StructuredResultError::InvalidField(
                "occurrences length must match repeat_count within MAX_OCCURRENCE_REFS_PER_GROUP",
            ));
        }
        self.first_occurrence.validate()?;
        self.last_occurrence.validate()?;
        for occurrence in &self.occurrences {
            occurrence.validate()?;
        }
        if self.occurrences.first() != Some(&self.first_occurrence)
            || self.occurrences.last() != Some(&self.last_occurrence)
        {
            return Err(StructuredResultError::InvalidField(
                "first_occurrence and last_occurrence must match retained occurrences",
            ));
        }
        Ok(())
    }
}

/// Deterministically groups adjacent identical failure diagnostics within a
/// single `(attempt_id, candidate_ref, source_commit_sha, check_id)` scope
/// without collapsing across intervening passes, timeouts, partial outputs,
/// unknown formats, or different failures.
pub fn group_diagnostic_observations(
    expected_attempt: &AttemptId,
    expected_candidate_ref: &str,
    expected_commit_sha: &str,
    expected_check_id: &str,
    observations: &[DiagnosticObservation],
) -> Result<Vec<DiagnosticGroup>, StructuredResultError> {
    validate_graphic_id(expected_candidate_ref, "expected_candidate_ref")?;
    validate_git_sha(expected_commit_sha, "expected_commit_sha")?;
    validate_graphic_id(expected_check_id, "expected_check_id")?;
    if observations.len() > MAX_DIAGNOSTICS_PER_CHECK * MAX_OCCURRENCE_REFS_PER_GROUP {
        return Err(StructuredResultError::BoundExceeded);
    }

    let mut groups: Vec<DiagnosticGroup> = Vec::new();
    let mut prev_seq = 0_u64;

    for obs in observations {
        obs.validate()?;
        if &obs.attempt_id != expected_attempt
            || obs.candidate_ref != expected_candidate_ref
            || obs.source_commit_sha != expected_commit_sha
            || obs.check_id != expected_check_id
        {
            return Err(StructuredResultError::IncompatibleSourceIdentity(
                "cannot group diagnostics across distinct attempts, candidates, commits, or checks",
            ));
        }
        let seq = obs.sequence.as_u64();
        if seq <= prev_seq {
            return Err(StructuredResultError::InvalidField(
                "diagnostic observations must have strictly increasing sequences",
            ));
        }
        prev_seq = seq;

        let can_append = groups.last().is_some_and(|last| {
            obs.transition.is_collapsible_failure()
                && last.transition == obs.transition
                && last.check_id == obs.check_id
                && last.fingerprint == obs.fingerprint
                && last.assertion_or_code == obs.assertion_or_code
                && last.file_path == obs.file_path
                && last.line_number == obs.line_number
                && last.environment == obs.environment
                && last.excerpt == obs.excerpt
        });

        if can_append {
            let Some(last) = groups.last_mut() else {
                return Err(StructuredResultError::InvalidField("missing group"));
            };
            if last.occurrences.len() >= MAX_OCCURRENCE_REFS_PER_GROUP {
                return Err(StructuredResultError::BoundExceeded);
            }
            let next_count = last
                .repeat_count
                .as_u128()
                .checked_add(1)
                .ok_or(StructuredResultError::BoundExceeded)?;
            last.repeat_count = DecimalU128(next_count);
            last.last_sequence = obs.sequence.clone();
            last.last_occurrence = obs.raw_range.clone();
            last.occurrences.push(obs.raw_range.clone());
        } else {
            if groups.len() >= MAX_DIAGNOSTICS_PER_CHECK {
                return Err(StructuredResultError::BoundExceeded);
            }
            groups.push(DiagnosticGroup {
                first_sequence: obs.sequence.clone(),
                last_sequence: obs.sequence.clone(),
                check_id: obs.check_id.clone(),
                transition: obs.transition.clone(),
                fingerprint: obs.fingerprint.clone(),
                assertion_or_code: obs.assertion_or_code.clone(),
                file_path: obs.file_path.clone(),
                line_number: obs.line_number.clone(),
                environment: obs.environment.clone(),
                excerpt: obs.excerpt.clone(),
                repeat_count: DecimalU128(1),
                first_occurrence: obs.raw_range.clone(),
                last_occurrence: obs.raw_range.clone(),
                occurrences: vec![obs.raw_range.clone()],
            });
        }
    }

    Ok(groups)
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum CheckOutcomeStatus {
    Passed {
        exit_code: i32,
        duration: MeasurementObservation,
    },
    Failed {
        exit_code: i32,
        failed_criterion_id: String,
        duration: MeasurementObservation,
    },
    TimedOut {
        timeout_secs: u32,
        observed_duration: MeasurementObservation,
        partial_output_retained: bool,
    },
    Partial {
        reason: String,
        observed_duration: MeasurementObservation,
    },
    Unknown {
        reason: String,
    },
}

impl CheckOutcomeStatus {
    pub fn validate(
        &self,
        check_id: &str,
        contract: &TaskContract,
    ) -> Result<(), StructuredResultError> {
        match self {
            Self::Passed {
                exit_code,
                duration,
            } => {
                if *exit_code != 0 {
                    return Err(StructuredResultError::InvalidField(
                        "passed check status requires exit_code == 0",
                    ));
                }
                duration.validate()
            }
            Self::Failed {
                exit_code,
                failed_criterion_id,
                duration,
            } => {
                if !(1..=255).contains(exit_code) {
                    return Err(StructuredResultError::InvalidField(
                        "failed check exit_code must be within 1..=255",
                    ));
                }
                let matches_criterion = contract.criteria.iter().any(|criterion| {
                    criterion.id == *failed_criterion_id
                        && criterion.check_ids.iter().any(|id| id == check_id)
                });
                if !matches_criterion {
                    return Err(StructuredResultError::IncompatibleSourceIdentity(
                        "failed_criterion_id must reference a contract criterion bound to check_id",
                    ));
                }
                duration.validate()
            }
            Self::TimedOut {
                timeout_secs,
                observed_duration,
                ..
            } => {
                if *timeout_secs == 0 {
                    return Err(StructuredResultError::InvalidField(
                        "timeout_secs must be nonzero",
                    ));
                }
                observed_duration.validate()
            }
            Self::Partial {
                reason,
                observed_duration,
            } => {
                validate_non_empty_text(reason, MAX_EXCERPT_BYTES, "partial reason")?;
                observed_duration.validate()
            }
            Self::Unknown { reason } => {
                validate_non_empty_text(reason, MAX_EXCERPT_BYTES, "unknown reason")
            }
        }
    }

    pub const fn is_passing(&self) -> bool {
        matches!(self, Self::Passed { .. })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CheckResultSummary {
    pub check: CheckSpec,
    pub provenance: ResultProvenance,
    pub status: CheckOutcomeStatus,
    pub stdout_log: ArtifactByteRangeRef,
    pub stderr_log: ArtifactByteRangeRef,
    pub diagnostics: Vec<DiagnosticGroup>,
    pub measured_counters: Vec<MeasurementObservation>,
    pub uncertainty: Vec<String>,
}

impl CheckResultSummary {
    pub fn validate(&self, contract: &TaskContract) -> Result<(), StructuredResultError> {
        let Some(expected_check) = contract.checks.iter().find(|c| c.id == self.check.id) else {
            return Err(StructuredResultError::IncompatibleSourceIdentity(
                "check summary references a check not declared in TaskContract",
            ));
        };
        if &self.check != expected_check {
            return Err(StructuredResultError::IncompatibleSourceIdentity(
                "check summary argv/cwd/timeout differs from TaskContract check",
            ));
        }
        self.provenance.validate()?;
        self.status.validate(&self.check.id, contract)?;
        self.stdout_log.validate()?;
        self.stderr_log.validate()?;
        if self.diagnostics.len() > MAX_DIAGNOSTICS_PER_CHECK
            || self.measured_counters.len() > MAX_APPROACH_EVIDENCE_ITEMS
            || self.uncertainty.len() > MAX_UNCERTAINTY_ITEMS
        {
            return Err(StructuredResultError::BoundExceeded);
        }
        let mut prev_last_seq = 0_u64;
        for group in &self.diagnostics {
            group.validate(&self.check.id)?;
            if group.first_sequence.as_u64() <= prev_last_seq {
                return Err(StructuredResultError::InvalidField(
                    "diagnostic groups must have strictly increasing sequence ranges",
                ));
            }
            prev_last_seq = group.last_sequence.as_u64();
        }
        for counter in &self.measured_counters {
            counter.validate()?;
        }
        for item in &self.uncertainty {
            validate_non_empty_text(item, MAX_TEXT_FIELD_BYTES, "check uncertainty")?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HypothesisStatus {
    TentativeUnverified,
}

/// Task-scoped failed-approach record tied to exact hypothesis, action, source,
/// check outcome, and bounded raw evidence references.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FailedApproachRecord {
    pub approach_id: String,
    pub attempt_id: AttemptId,
    pub candidate_ref: String,
    pub source_commit_sha: String,
    pub source_tree_sha: String,
    pub hypothesis: String,
    pub hypothesis_status: HypothesisStatus,
    pub action_taken: String,
    pub check_id: String,
    pub outcome: CheckOutcomeStatus,
    pub raw_evidence_refs: Vec<ArtifactByteRangeRef>,
    pub negative_evidence: Vec<String>,
}

impl FailedApproachRecord {
    pub fn validate(
        &self,
        contract: &TaskContract,
        expected_project_task: (&ProjectId, &RootId, &TaskId),
    ) -> Result<(), StructuredResultError> {
        let _ = expected_project_task;
        validate_graphic_id(&self.approach_id, "approach_id")?;
        validate_graphic_id(&self.candidate_ref, "candidate_ref")?;
        validate_git_commit_and_tree(&self.source_commit_sha, &self.source_tree_sha)?;
        validate_non_empty_text(&self.hypothesis, MAX_TEXT_FIELD_BYTES, "hypothesis")?;
        validate_non_empty_text(&self.action_taken, MAX_TEXT_FIELD_BYTES, "action_taken")?;
        if !contract.checks.iter().any(|c| c.id == self.check_id) {
            return Err(StructuredResultError::IncompatibleSourceIdentity(
                "failed approach check_id is not in TaskContract",
            ));
        }
        self.outcome.validate(&self.check_id, contract)?;
        if self.outcome.is_passing() {
            return Err(StructuredResultError::InvalidField(
                "failed approach record cannot have a passing check outcome",
            ));
        }
        if self.raw_evidence_refs.is_empty()
            || self.raw_evidence_refs.len() > MAX_APPROACH_EVIDENCE_ITEMS
            || self.negative_evidence.is_empty()
            || self.negative_evidence.len() > MAX_APPROACH_EVIDENCE_ITEMS
        {
            return Err(StructuredResultError::BoundExceeded);
        }
        for r in &self.raw_evidence_refs {
            r.validate()?;
        }
        for ev in &self.negative_evidence {
            validate_non_empty_text(ev, MAX_TEXT_FIELD_BYTES, "negative_evidence")?;
        }
        Ok(())
    }
}

/// Criterion constraint summary checked against the authoritative `TaskContract`
/// so no negation, unit, or criterion ID can be dropped.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProtectedConstraintSummary {
    pub criterion_id: String,
    pub summary_text: String,
    pub required_negations: Vec<String>,
    pub required_units: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StructuredResultIdentity {
    pub project_id: ProjectId,
    pub root_id: RootId,
    pub task_id: TaskId,
    pub contract_revision: u32,
    pub contract_digest: Sha256Digest,
    pub authority_ref: String,
    pub attempt_id: AttemptId,
    pub candidate_ref: String,
    pub source_commit_sha: String,
    pub source_tree_sha: String,
    pub environment_fingerprint: String,
}

impl StructuredResultIdentity {
    pub fn validate(&self, contract: &TaskContract) -> Result<(), StructuredResultError> {
        if self.contract_revision == 0 {
            return Err(StructuredResultError::ProtectedFactOmitted(
                "contract_revision must be nonzero",
            ));
        }
        if self.authority_ref.trim().is_empty() || self.authority_ref.len() > MAX_PATH_BYTES {
            return Err(StructuredResultError::ProtectedFactOmitted(
                "authority_ref must be non-empty and bounded",
            ));
        }
        validate_graphic_id(&self.candidate_ref, "candidate_ref")?;
        validate_git_commit_and_tree(&self.source_commit_sha, &self.source_tree_sha)?;
        validate_non_empty_text(
            &self.environment_fingerprint,
            MAX_PATH_BYTES,
            "environment_fingerprint",
        )?;
        if self.project_id != contract.project_id
            || self.root_id != contract.root_id
            || self.task_id != contract.task_id
        {
            return Err(StructuredResultError::IncompatibleSourceIdentity(
                "identity project_id, root_id, or task_id differs from TaskContract",
            ));
        }
        if self.contract_revision != contract.revision {
            return Err(StructuredResultError::ProtectedFactOmitted(
                "identity contract_revision differs from TaskContract revision",
            ));
        }
        if self.authority_ref != contract.authority_ref {
            return Err(StructuredResultError::ProtectedFactOmitted(
                "identity authority_ref differs from TaskContract authority_ref",
            ));
        }
        let actual_digest = content_digest(contract)
            .map_err(|_| StructuredResultError::InvalidField("TaskContract digest"))?;
        if self.contract_digest.as_str() != actual_digest {
            return Err(StructuredResultError::IncompatibleSourceIdentity(
                "identity contract_digest differs from TaskContract digest",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StructuredResultFacts {
    pub identity: StructuredResultIdentity,
    pub contract: TaskContract,
    pub must_carry_constraints: Vec<ProtectedConstraintSummary>,
    pub checks: Vec<CheckResultSummary>,
    pub failed_approaches: Vec<FailedApproachRecord>,
    pub uncertainty: Vec<String>,
    pub next_action: String,
}

impl StructuredResultFacts {
    pub fn validate(&self) -> Result<(), StructuredResultError> {
        self.contract
            .validate()
            .map_err(|_| StructuredResultError::InvalidField("TaskContract validation failed"))?;
        self.identity.validate(&self.contract)?;

        if self.checks.is_empty()
            || self.checks.len() > MAX_CHECK_SUMMARIES
            || self.failed_approaches.len() > MAX_FAILED_APPROACHES
            || self.uncertainty.len() > MAX_UNCERTAINTY_ITEMS
        {
            return Err(StructuredResultError::BoundExceeded);
        }

        validate_constraints(&self.contract, &self.must_carry_constraints)?;

        let mut seen_checks = BTreeSet::new();
        for check_summary in &self.checks {
            if !seen_checks.insert(check_summary.check.id.as_str()) {
                return Err(StructuredResultError::InvalidField(
                    "duplicate check summary ID",
                ));
            }
            check_summary.validate(&self.contract)?;
        }

        let mut seen_approaches = BTreeSet::new();
        for approach in &self.failed_approaches {
            if !seen_approaches.insert(approach.approach_id.as_str()) {
                return Err(StructuredResultError::InvalidField(
                    "duplicate failed approach ID",
                ));
            }
            approach.validate(
                &self.contract,
                (
                    &self.identity.project_id,
                    &self.identity.root_id,
                    &self.identity.task_id,
                ),
            )?;
        }

        for item in &self.uncertainty {
            validate_non_empty_text(item, MAX_TEXT_FIELD_BYTES, "uncertainty")?;
        }
        validate_non_empty_text(&self.next_action, MAX_TEXT_FIELD_BYTES, "next_action")?;
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum BaselineObservation {
    Fresh,
    Retained {
        baseline_digest: Sha256Digest,
        context_epoch: ContextEpoch,
    },
    CompactedOrLost {
        reason: String,
    },
}

impl BaselineObservation {
    pub fn validate(&self) -> Result<(), StructuredResultError> {
        if let Self::CompactedOrLost { reason } = self {
            validate_non_empty_text(reason, MAX_EXCERPT_BYTES, "compaction reason")?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct StructuredResultDocument {
    protocol_version: u32,
    context_epoch: ContextEpoch,
    baseline: BaselineObservation,
    fresh_capsule_requested: bool,
    facts_digest: Sha256Digest,
    authority_granted: bool,
    execution_evidence_trusted: bool,
    artifact_custody_verified: bool,
    retry_or_escalation_authorized: bool,
    token_savings_claimed: bool,
    facts: StructuredResultFacts,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StructuredResultCapsule {
    pub context_epoch: ContextEpoch,
    pub baseline: BaselineObservation,
    pub fresh_capsule_requested: bool,
    pub facts_digest: Sha256Digest,
    pub rendered_digest: Sha256Digest,
    pub utf8_bytes: DecimalU64,
    pub estimated_tokens_div4: DecimalU64,
    pub tokenizer_basis: String,
    pub authority_granted: bool,
    pub execution_evidence_trusted: bool,
    pub artifact_custody_verified: bool,
    pub retry_or_escalation_authorized: bool,
    pub token_savings_claimed: bool,
    pub text: String,
}

/// Independently retained caller expectations used when decoding a rendered
/// `StructuredResultCapsule`.
pub struct ExpectedStructuredResultBindings<'a> {
    pub identity: &'a StructuredResultIdentity,
    pub context_epoch: &'a ContextEpoch,
    pub baseline: &'a BaselineObservation,
    pub facts_digest: &'a Sha256Digest,
}

struct BoundedWriter {
    bytes: Vec<u8>,
    limit: usize,
}

impl Write for BoundedWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self
            .bytes
            .len()
            .checked_add(buf.len())
            .is_none_or(|n| n > self.limit)
        {
            return Err(std::io::Error::other(
                "structured result byte budget exceeded",
            ));
        }
        self.bytes.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Compiles `StructuredResultFacts` into a bounded, deterministic JSON capsule.
///
/// If `baseline` is `CompactedOrLost` and `allow_fresh_capsule_on_compaction` is
/// `false`, returns `Err(StructuredResultError::FreshCapsuleRequired { .. })`
/// so callers explicitly request a fresh complete capsule rather than assuming
/// hidden provider context survived compaction.
pub fn compile_structured_results(
    facts: &StructuredResultFacts,
    context_epoch: &ContextEpoch,
    baseline: &BaselineObservation,
    allow_fresh_capsule_on_compaction: bool,
    max_tokens: usize,
) -> Result<StructuredResultCapsule, StructuredResultError> {
    baseline.validate()?;
    if let BaselineObservation::CompactedOrLost { reason } = baseline
        && !allow_fresh_capsule_on_compaction
    {
        return Err(StructuredResultError::FreshCapsuleRequired {
            reason: reason.clone(),
        });
    }
    if let BaselineObservation::Retained {
        context_epoch: retained_epoch,
        ..
    } = baseline
        && retained_epoch != context_epoch
    {
        return Err(StructuredResultError::FreshCapsuleRequired {
            reason: "retained baseline epoch differs from target context_epoch".into(),
        });
    }

    let limit = max_tokens
        .checked_mul(4)
        .filter(|n| *n > 0)
        .ok_or(StructuredResultError::InvalidBudget)?
        .min(MAX_STRUCTURED_RESULT_BYTES);

    bounded_json_size(facts, MAX_STRUCTURED_RESULT_BYTES)
        .map_err(|_| StructuredResultError::BoundExceeded)?;
    facts.validate()?;
    bounded_json_size(facts, limit).map_err(|_| StructuredResultError::BoundExceeded)?;

    let facts_digest = Sha256Digest::new(
        content_digest(facts).map_err(|_| StructuredResultError::InvalidField("facts_digest"))?,
    )
    .map_err(|_| StructuredResultError::InvalidField("facts_digest"))?;

    let fresh_capsule_requested = matches!(
        baseline,
        BaselineObservation::Fresh | BaselineObservation::CompactedOrLost { .. }
    );

    let document = StructuredResultDocument {
        protocol_version: STRUCTURED_RESULT_PROTOCOL_VERSION,
        context_epoch: context_epoch.clone(),
        baseline: baseline.clone(),
        fresh_capsule_requested,
        facts_digest: facts_digest.clone(),
        authority_granted: false,
        execution_evidence_trusted: false,
        artifact_custody_verified: false,
        retry_or_escalation_authorized: false,
        token_savings_claimed: false,
        facts: facts.clone(),
    };

    let mut writer = BoundedWriter {
        bytes: Vec::new(),
        limit,
    };
    serde_json::to_writer(&mut writer, &document)
        .map_err(|_| StructuredResultError::BoundExceeded)?;
    preflight_bytes_and_depth(&writer.bytes)?;

    let rendered_digest = Sha256Digest::of_bytes(&writer.bytes);
    let utf8_len = writer.bytes.len();
    let est_tokens = utf8_len.div_ceil(4);
    let text = String::from_utf8(writer.bytes).map_err(|_| StructuredResultError::Json)?;

    Ok(StructuredResultCapsule {
        context_epoch: context_epoch.clone(),
        baseline: baseline.clone(),
        fresh_capsule_requested,
        facts_digest,
        rendered_digest,
        utf8_bytes: DecimalU64(utf8_len as u64),
        estimated_tokens_div4: DecimalU64(est_tokens as u64),
        tokenizer_basis: "utf8_bytes_div4_estimate_no_savings_claim".into(),
        authority_granted: false,
        execution_evidence_trusted: false,
        artifact_custody_verified: false,
        retry_or_escalation_authorized: false,
        token_savings_claimed: false,
        text,
    })
}

/// Decodes and validates a rendered `StructuredResultCapsule` against
/// independently retained bindings.
pub fn decode_structured_results(
    raw_bytes: &[u8],
    expected: &ExpectedStructuredResultBindings<'_>,
) -> Result<StructuredResultFacts, StructuredResultError> {
    preflight_bytes_and_depth(raw_bytes)?;
    let document: StructuredResultDocument =
        serde_json::from_slice(raw_bytes).map_err(|_| StructuredResultError::Json)?;

    if document.protocol_version != STRUCTURED_RESULT_PROTOCOL_VERSION {
        return Err(StructuredResultError::BindingMismatch("protocol_version"));
    }
    if document.authority_granted
        || document.execution_evidence_trusted
        || document.artifact_custody_verified
        || document.retry_or_escalation_authorized
        || document.token_savings_claimed
    {
        return Err(StructuredResultError::FalseAuthorityClaim(
            "wire document asserted a forbidden authority, custody, or token-savings flag",
        ));
    }
    if &document.context_epoch != expected.context_epoch {
        return Err(StructuredResultError::BindingMismatch("context_epoch"));
    }
    if &document.baseline != expected.baseline {
        return Err(StructuredResultError::BindingMismatch("baseline"));
    }
    let expected_fresh = matches!(
        expected.baseline,
        BaselineObservation::Fresh | BaselineObservation::CompactedOrLost { .. }
    );
    if document.fresh_capsule_requested != expected_fresh {
        return Err(StructuredResultError::BindingMismatch(
            "fresh_capsule_requested",
        ));
    }
    if &document.facts_digest != expected.facts_digest {
        return Err(StructuredResultError::BindingMismatch("facts_digest"));
    }
    document.facts.validate()?;
    if &document.facts.identity != expected.identity {
        return Err(StructuredResultError::IncompatibleSourceIdentity(
            "decoded facts identity differs from expected identity",
        ));
    }
    let computed_digest =
        content_digest(&document.facts).map_err(|_| StructuredResultError::Json)?;
    if computed_digest != expected.facts_digest.as_str() {
        return Err(StructuredResultError::BindingMismatch(
            "recomputed facts_digest differs from expected facts_digest",
        ));
    }
    Ok(document.facts)
}

fn validate_constraints(
    contract: &TaskContract,
    summaries: &[ProtectedConstraintSummary],
) -> Result<(), StructuredResultError> {
    if summaries.len() != contract.criteria.len() {
        return Err(StructuredResultError::ProtectedFactOmitted(
            "must_carry_constraints must include every TaskContract criterion in order",
        ));
    }
    let objective_negations = extract_matching_tokens(&contract.objective, NEGATION_TOKENS);
    let objective_units = extract_matching_tokens(&contract.objective, UNIT_TOKENS);

    for (criterion, summary) in contract.criteria.iter().zip(summaries) {
        if summary.criterion_id != criterion.id {
            return Err(StructuredResultError::ProtectedFactOmitted(
                "must_carry_constraints criterion_id order or identity mismatch",
            ));
        }
        validate_non_empty_text(
            &summary.summary_text,
            MAX_TEXT_FIELD_BYTES,
            "constraint summary_text",
        )?;
        if summary.required_negations.len() > MAX_UNCERTAINTY_ITEMS
            || summary.required_units.len() > MAX_UNCERTAINTY_ITEMS
        {
            return Err(StructuredResultError::BoundExceeded);
        }

        let mut expected_negations = extract_matching_tokens(&criterion.text, NEGATION_TOKENS);
        expected_negations.extend(objective_negations.iter().cloned());

        let mut expected_units = extract_matching_tokens(&criterion.text, UNIT_TOKENS);
        expected_units.extend(objective_units.iter().cloned());

        let summary_negations = extract_matching_tokens(&summary.summary_text, NEGATION_TOKENS);
        for neg in &expected_negations {
            if !summary_negations.contains(neg)
                || !summary
                    .required_negations
                    .iter()
                    .any(|item| item.eq_ignore_ascii_case(neg))
            {
                return Err(StructuredResultError::ProtectedFactOmitted(
                    "required negation from contract objective or criterion was omitted",
                ));
            }
        }

        let summary_units = extract_matching_tokens(&summary.summary_text, UNIT_TOKENS);
        for unit in &expected_units {
            if !summary_units.contains(unit)
                || !summary
                    .required_units
                    .iter()
                    .any(|item| item.eq_ignore_ascii_case(unit))
            {
                return Err(StructuredResultError::ProtectedFactOmitted(
                    "required unit from contract objective or criterion was omitted",
                ));
            }
        }
    }
    Ok(())
}

fn extract_matching_tokens(text: &str, vocabulary: &[&str]) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let lower = text.to_ascii_lowercase();
    for &op in &["<=", ">=", "!=", "%"] {
        if vocabulary.contains(&op) && lower.contains(op) {
            found.insert(op.to_owned());
        }
    }
    for word in lower.split(|c: char| !c.is_ascii_alphanumeric()) {
        if !word.is_empty() && vocabulary.contains(&word) {
            found.insert(word.to_owned());
        }
    }
    found
}

fn validate_non_empty_text(
    value: &str,
    max_bytes: usize,
    field: &'static str,
) -> Result<(), StructuredResultError> {
    if value.trim().is_empty() || value.len() > max_bytes || value.contains('\0') {
        return Err(StructuredResultError::InvalidField(field));
    }
    Ok(())
}

fn validate_graphic_id(value: &str, field: &'static str) -> Result<(), StructuredResultError> {
    if value.is_empty()
        || value.len() > 128
        || value == "."
        || value == ".."
        || !value.bytes().all(|b| b.is_ascii_graphic())
    {
        return Err(StructuredResultError::InvalidField(field));
    }
    Ok(())
}

fn validate_git_sha(value: &str, field: &'static str) -> Result<(), StructuredResultError> {
    if (value.len() != 40 && value.len() != 64)
        || value.bytes().all(|b| b == b'0')
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(StructuredResultError::MalformedReference(field));
    }
    Ok(())
}

fn validate_git_commit_and_tree(commit: &str, tree: &str) -> Result<(), StructuredResultError> {
    validate_git_sha(commit, "source_commit_sha")?;
    validate_git_sha(tree, "source_tree_sha")?;
    if commit.len() != tree.len() || commit == tree {
        return Err(StructuredResultError::MalformedReference(
            "source_commit_sha and source_tree_sha must be distinct full hex IDs of equal length",
        ));
    }
    Ok(())
}

fn preflight_bytes_and_depth(bytes: &[u8]) -> Result<(), StructuredResultError> {
    if bytes.is_empty() || bytes.len() > MAX_STRUCTURED_RESULT_BYTES {
        return Err(StructuredResultError::BoundExceeded);
    }
    let mut depth = 0_usize;
    let mut in_string = false;
    let mut escaped = false;
    for &byte in bytes {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
        } else {
            match byte {
                b'"' => in_string = true,
                b'{' | b'[' => {
                    depth = depth
                        .checked_add(1)
                        .ok_or(StructuredResultError::BoundExceeded)?;
                    if depth > MAX_STRUCTURED_RESULT_DEPTH {
                        return Err(StructuredResultError::BoundExceeded);
                    }
                }
                b'}' | b']' => {
                    depth = depth.checked_sub(1).ok_or(StructuredResultError::Json)?;
                }
                _ => {}
            }
        }
    }
    if in_string || depth != 0 {
        return Err(StructuredResultError::Json);
    }
    Ok(())
}

/// Computes a canonical `sha256:<64 hex>` digest for raw artifact bytes.
pub fn artifact_bytes_digest(bytes: &[u8]) -> Sha256Digest {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    Sha256Digest::of_bytes(bytes)
}
