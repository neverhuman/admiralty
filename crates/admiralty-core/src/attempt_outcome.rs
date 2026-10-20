//! Independent versioned per-attempt outcome contract (`P02` / `MASTER_PLAN` §6.1).
//!
//! Keeps task scheduling (`blocked/ready/claimed/running/awaiting_decision/closed`)
//! orthogonal to per-attempt outcome (`unknown/succeeded/failed/cancelled`) and retains
//! immutable per-attempt closure evidence, candidate references, decision bindings, and
//! failure/cancellation reasons across retries without mutating closed v1 `TaskView` or
//! `SchedulingState` wire formats.

use crate::{
    AttemptId, CandidateDecisionEvidence, DomainEvent, LifecycleAction, LifecycleCommand,
    PrincipalId, ProjectId, Projection, RootId, SchedulingState, TaskContract, TaskId,
    content_digest, limits::bounded_json_size, reduce,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const ATTEMPT_OUTCOME_SCHEMA_VERSION: u32 = 1;
pub const MAX_ATTEMPT_OUTCOMES_PER_TASK: usize = 256;
pub const MAX_ATTEMPT_REASON_BYTES: usize = 1024;
pub const MAX_ATTEMPT_CANDIDATE_REF_BYTES: usize = 4096;
pub const MAX_ATTEMPT_LEDGER_JSON_BYTES: usize = 512 * 1024;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskSchedulingDimension {
    Blocked,
    Ready,
    Claimed,
    Running,
    AwaitingDecision,
    Closed,
}

impl TaskSchedulingDimension {
    pub fn from_v1_state(state: &SchedulingState) -> Self {
        match state {
            SchedulingState::Blocked => Self::Blocked,
            SchedulingState::Ready => Self::Ready,
            SchedulingState::Claimed => Self::Claimed,
            SchedulingState::Running => Self::Running,
            SchedulingState::AwaitingVerification => Self::AwaitingDecision,
            SchedulingState::Accepted | SchedulingState::Failed | SchedulingState::Cancelled => {
                Self::Closed
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AttemptOutcomeStatus {
    Unknown,
    Succeeded,
    Failed,
    Cancelled,
}

impl AttemptOutcomeStatus {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AttemptOutcomeRecord {
    pub schema_version: u32,
    pub project_id: ProjectId,
    pub root_id: RootId,
    pub task_id: TaskId,
    pub contract_revision: u32,
    pub contract_digest: String,
    pub attempt_id: AttemptId,
    pub attempt_index: u16,
    pub owner: PrincipalId,
    pub started: bool,
    pub candidate_ref: Option<String>,
    pub status: AttemptOutcomeStatus,
    pub decision_evidence: Option<CandidateDecisionEvidence>,
    pub closure_digest: Option<String>,
    pub reason: Option<String>,
    pub superseded_by_retry: bool,
}

impl AttemptOutcomeRecord {
    pub fn validate(&self) -> Result<(), AttemptOutcomeError> {
        if self.schema_version != ATTEMPT_OUTCOME_SCHEMA_VERSION {
            return Err(AttemptOutcomeError::Invalid("unsupported schema_version"));
        }
        if self.contract_revision == 0 || self.attempt_index == 0 {
            return Err(AttemptOutcomeError::Invalid(
                "contract_revision and attempt_index must be positive",
            ));
        }
        if !valid_sha256_digest(&self.contract_digest) {
            return Err(AttemptOutcomeError::Invalid("invalid contract_digest"));
        }
        if let Some(candidate) = &self.candidate_ref
            && (candidate.trim().is_empty()
                || candidate.len() > MAX_ATTEMPT_CANDIDATE_REF_BYTES
                || candidate.chars().any(char::is_control))
        {
            return Err(AttemptOutcomeError::Invalid("invalid candidate_ref"));
        }
        if let Some(reason) = &self.reason
            && (reason.trim().is_empty()
                || reason.len() > MAX_ATTEMPT_REASON_BYTES
                || reason.chars().any(char::is_control))
        {
            return Err(AttemptOutcomeError::Invalid("invalid closure reason"));
        }
        match self.status {
            AttemptOutcomeStatus::Unknown => {
                if self.closure_digest.is_some()
                    || self.decision_evidence.is_some()
                    || self.superseded_by_retry
                {
                    return Err(AttemptOutcomeError::Invalid(
                        "unknown attempt outcome cannot carry terminal closure or retry supersession",
                    ));
                }
            }
            AttemptOutcomeStatus::Succeeded => {
                let Some(evidence) = &self.decision_evidence else {
                    return Err(AttemptOutcomeError::Invalid(
                        "succeeded attempt requires independent decision_evidence",
                    ));
                };
                if !self.started
                    || self.candidate_ref.is_none()
                    || self.closure_digest.as_deref() != Some(evidence.closure_digest.as_str())
                    || !valid_sha256_digest(&evidence.closure_digest)
                    || self.superseded_by_retry
                {
                    return Err(AttemptOutcomeError::Invalid(
                        "succeeded attempt must bind started candidate, valid closure_digest, and cannot be retried",
                    ));
                }
            }
            AttemptOutcomeStatus::Failed | AttemptOutcomeStatus::Cancelled => {
                let Some(closure) = &self.closure_digest else {
                    return Err(AttemptOutcomeError::Invalid(
                        "failed or cancelled attempt outcome requires closure_digest",
                    ));
                };
                if !valid_sha256_digest(closure) {
                    return Err(AttemptOutcomeError::Invalid("invalid closure_digest"));
                }
                if self.status == AttemptOutcomeStatus::Cancelled
                    && self.decision_evidence.is_some()
                {
                    return Err(AttemptOutcomeError::Invalid(
                        "cancelled attempt cannot carry candidate acceptance decision evidence",
                    ));
                }
            }
        }
        Ok(())
    }

    pub fn canonical_digest(&self) -> Result<String, AttemptOutcomeError> {
        self.validate()?;
        content_digest(self).map_err(|_| AttemptOutcomeError::Serialization)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TaskAttemptOutcomeLedger {
    #[schemars(extend("const" = ATTEMPT_OUTCOME_SCHEMA_VERSION))]
    pub schema_version: u32,
    pub project_id: ProjectId,
    pub root_id: RootId,
    pub task_id: TaskId,
    pub current_contract_revision: u32,
    pub current_contract_digest: String,
    pub max_attempts: u16,
    pub scheduling: TaskSchedulingDimension,
    pub active_attempt_id: Option<AttemptId>,
    pub attempt_order: Vec<AttemptId>,
    pub outcomes: BTreeMap<AttemptId, AttemptOutcomeRecord>,
    pub pending_cancel_reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum AttemptOutcomeError {
    #[error("attempt outcome bound exceeded")]
    BoundExceeded,
    #[error("invalid attempt outcome state: {0}")]
    Invalid(&'static str),
    #[error("attempt outcome transition rejected: {0}")]
    Transition(String),
    #[error("attempt outcome serialization failed")]
    Serialization,
}

fn valid_sha256_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

impl TaskAttemptOutcomeLedger {
    pub fn from_contract(
        contract: &TaskContract,
        initial_scheduling: TaskSchedulingDimension,
    ) -> Result<Self, AttemptOutcomeError> {
        contract
            .validate()
            .map_err(|e| AttemptOutcomeError::Transition(e.to_string()))?;
        if !matches!(
            initial_scheduling,
            TaskSchedulingDimension::Blocked | TaskSchedulingDimension::Ready
        ) {
            return Err(AttemptOutcomeError::Invalid(
                "submitted task must start in blocked or ready scheduling state",
            ));
        }
        let digest = content_digest(contract).map_err(|_| AttemptOutcomeError::Serialization)?;
        Ok(Self {
            schema_version: ATTEMPT_OUTCOME_SCHEMA_VERSION,
            project_id: contract.project_id.clone(),
            root_id: contract.root_id.clone(),
            task_id: contract.task_id.clone(),
            current_contract_revision: contract.revision,
            current_contract_digest: digest,
            max_attempts: contract.max_attempts,
            scheduling: initial_scheduling,
            active_attempt_id: None,
            attempt_order: Vec::new(),
            outcomes: BTreeMap::new(),
            pending_cancel_reason: None,
        })
    }

    pub fn validate(&self) -> Result<(), AttemptOutcomeError> {
        bounded_json_size(self, MAX_ATTEMPT_LEDGER_JSON_BYTES)
            .map_err(|_| AttemptOutcomeError::BoundExceeded)?;
        if self.schema_version != ATTEMPT_OUTCOME_SCHEMA_VERSION {
            return Err(AttemptOutcomeError::Invalid("unsupported schema_version"));
        }
        if self.current_contract_revision == 0
            || self.max_attempts == 0
            || !valid_sha256_digest(&self.current_contract_digest)
        {
            return Err(AttemptOutcomeError::Invalid(
                "invalid contract revision, max_attempts, or digest",
            ));
        }
        if self.outcomes.len() > MAX_ATTEMPT_OUTCOMES_PER_TASK
            || self.outcomes.len() > usize::from(self.max_attempts)
            || self.attempt_order.len() != self.outcomes.len()
        {
            return Err(AttemptOutcomeError::BoundExceeded);
        }
        for (idx, attempt_id) in self.attempt_order.iter().enumerate() {
            let Some(record) = self.outcomes.get(attempt_id) else {
                return Err(AttemptOutcomeError::Invalid(
                    "attempt_order references missing attempt record",
                ));
            };
            record.validate()?;
            if record.project_id != self.project_id
                || record.root_id != self.root_id
                || record.task_id != self.task_id
                || usize::from(record.attempt_index) != idx + 1
            {
                return Err(AttemptOutcomeError::Invalid(
                    "attempt record identity or ordinal mismatch",
                ));
            }
        }
        match self.scheduling {
            TaskSchedulingDimension::Blocked | TaskSchedulingDimension::Ready => {
                if self.active_attempt_id.is_some() || self.pending_cancel_reason.is_some() {
                    return Err(AttemptOutcomeError::Invalid(
                        "blocked/ready task cannot have active_attempt_id or pending_cancel_reason",
                    ));
                }
                if self
                    .outcomes
                    .values()
                    .any(|r| !r.superseded_by_retry || r.status == AttemptOutcomeStatus::Unknown)
                {
                    return Err(AttemptOutcomeError::Invalid(
                        "prior attempts on ready/blocked task must be closed and marked superseded_by_retry",
                    ));
                }
            }
            TaskSchedulingDimension::Claimed
            | TaskSchedulingDimension::Running
            | TaskSchedulingDimension::AwaitingDecision => {
                let Some(active_id) = &self.active_attempt_id else {
                    return Err(AttemptOutcomeError::Invalid(
                        "active scheduling state requires active_attempt_id",
                    ));
                };
                let Some(active) = self.outcomes.get(active_id) else {
                    return Err(AttemptOutcomeError::Invalid(
                        "active_attempt_id missing from outcomes",
                    ));
                };
                if active.status != AttemptOutcomeStatus::Unknown || active.superseded_by_retry {
                    return Err(AttemptOutcomeError::Invalid(
                        "active attempt must have Unknown outcome and not be superseded",
                    ));
                }
            }
            TaskSchedulingDimension::Closed => {
                if let Some(active_id) = &self.active_attempt_id {
                    let Some(active) = self.outcomes.get(active_id) else {
                        return Err(AttemptOutcomeError::Invalid(
                            "closed task active_attempt_id missing from outcomes",
                        ));
                    };
                    if !active.status.is_terminal() {
                        return Err(AttemptOutcomeError::Invalid(
                            "closed task with attempt must have terminal AttemptOutcomeStatus",
                        ));
                    }
                } else if self.pending_cancel_reason.is_none() {
                    return Err(AttemptOutcomeError::Invalid(
                        "closed task without attempt requires pending_cancel_reason",
                    ));
                }
            }
        }
        Ok(())
    }

    pub fn latest_attempt_outcome(&self) -> Option<&AttemptOutcomeRecord> {
        self.attempt_order
            .last()
            .and_then(|id| self.outcomes.get(id))
    }

    pub fn canonical_digest(&self) -> Result<String, AttemptOutcomeError> {
        self.validate()?;
        content_digest(self).map_err(|_| AttemptOutcomeError::Serialization)
    }
}

/// Deterministic projection of both the closed v1 `Projection` and the independent
/// per-task `TaskAttemptOutcomeLedger` map across an ordered event stream.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AttemptOutcomeProjection {
    pub projection: Projection,
    pub task_ledgers: BTreeMap<TaskId, TaskAttemptOutcomeLedger>,
}

impl AttemptOutcomeProjection {
    pub fn apply(&self, event: &DomainEvent) -> Result<Self, AttemptOutcomeError> {
        let next_projection = reduce(&self.projection, event)
            .map_err(|e| AttemptOutcomeError::Transition(e.to_string()))?;
        let mut next_ledgers = self.task_ledgers.clone();

        match event {
            DomainEvent::TaskSubmitted { contract } => {
                let v1_task = next_projection.tasks.get(&contract.task_id).ok_or(
                    AttemptOutcomeError::Invalid("missing submitted task in projection"),
                )?;
                let ledger = TaskAttemptOutcomeLedger::from_contract(
                    contract,
                    TaskSchedulingDimension::from_v1_state(&v1_task.state),
                )?;
                next_ledgers.insert(contract.task_id.clone(), ledger);
            }
            DomainEvent::TaskClaimed {
                task_id,
                revision,
                attempt_id,
                owner,
            } => {
                let ledger = next_ledgers.get_mut(task_id).ok_or_else(|| {
                    AttemptOutcomeError::Transition("unknown task in outcome ledger".into())
                })?;
                if ledger.outcomes.contains_key(attempt_id) {
                    return Err(AttemptOutcomeError::Transition(
                        "duplicate attempt_id in outcome ledger".into(),
                    ));
                }
                let next_index = u16::try_from(ledger.outcomes.len() + 1)
                    .map_err(|_| AttemptOutcomeError::BoundExceeded)?;
                if next_index > ledger.max_attempts
                    || usize::from(next_index) > MAX_ATTEMPT_OUTCOMES_PER_TASK
                {
                    return Err(AttemptOutcomeError::BoundExceeded);
                }
                let record = AttemptOutcomeRecord {
                    schema_version: ATTEMPT_OUTCOME_SCHEMA_VERSION,
                    project_id: ledger.project_id.clone(),
                    root_id: ledger.root_id.clone(),
                    task_id: task_id.clone(),
                    contract_revision: *revision,
                    contract_digest: ledger.current_contract_digest.clone(),
                    attempt_id: attempt_id.clone(),
                    attempt_index: next_index,
                    owner: owner.clone(),
                    started: false,
                    candidate_ref: None,
                    status: AttemptOutcomeStatus::Unknown,
                    decision_evidence: None,
                    closure_digest: None,
                    reason: None,
                    superseded_by_retry: false,
                };
                record.validate()?;
                ledger.attempt_order.push(attempt_id.clone());
                ledger.outcomes.insert(attempt_id.clone(), record);
                ledger.active_attempt_id = Some(attempt_id.clone());
                ledger.scheduling = TaskSchedulingDimension::Claimed;
                ledger.validate()?;
            }
            DomainEvent::TaskStarted {
                task_id,
                attempt_id,
            } => {
                let ledger = next_ledgers.get_mut(task_id).ok_or_else(|| {
                    AttemptOutcomeError::Transition("unknown task in outcome ledger".into())
                })?;
                let record = ledger.outcomes.get_mut(attempt_id).ok_or_else(|| {
                    AttemptOutcomeError::Transition("unknown attempt in outcome ledger".into())
                })?;
                record.started = true;
                ledger.scheduling = TaskSchedulingDimension::Running;
                ledger.validate()?;
            }
            DomainEvent::CandidateSubmitted {
                task_id,
                attempt_id,
                candidate_ref,
            } => {
                let ledger = next_ledgers.get_mut(task_id).ok_or_else(|| {
                    AttemptOutcomeError::Transition("unknown task in outcome ledger".into())
                })?;
                let record = ledger.outcomes.get_mut(attempt_id).ok_or_else(|| {
                    AttemptOutcomeError::Transition("unknown attempt in outcome ledger".into())
                })?;
                record.candidate_ref = Some(candidate_ref.clone());
                ledger.scheduling = TaskSchedulingDimension::AwaitingDecision;
                ledger.validate()?;
            }
            DomainEvent::TaskLifecycle { command } => {
                apply_lifecycle_to_ledgers(&next_projection, &mut next_ledgers, command)?;
            }
            DomainEvent::MessageQueued { .. } | DomainEvent::MessageAcknowledged { .. } => {}
        }

        // Sync any newly unblocked dependents from `next_projection`.
        for (task_id, task_view) in &next_projection.tasks {
            if let Some(ledger) = next_ledgers.get_mut(task_id)
                && matches!(
                    ledger.scheduling,
                    TaskSchedulingDimension::Blocked | TaskSchedulingDimension::Ready
                )
            {
                ledger.scheduling = TaskSchedulingDimension::from_v1_state(&task_view.state);
                ledger.validate()?;
            }
        }

        Ok(Self {
            projection: next_projection,
            task_ledgers: next_ledgers,
        })
    }
}

fn apply_lifecycle_to_ledgers(
    next_projection: &Projection,
    ledgers: &mut BTreeMap<TaskId, TaskAttemptOutcomeLedger>,
    command: &LifecycleCommand,
) -> Result<(), AttemptOutcomeError> {
    let ledger = ledgers
        .get_mut(&command.task_id)
        .ok_or_else(|| AttemptOutcomeError::Transition("unknown task in outcome ledger".into()))?;
    match &command.action {
        LifecycleAction::DecideCandidate {
            attempt_id,
            candidate_ref,
            evidence,
            accepted,
        } => {
            let record = ledger.outcomes.get_mut(attempt_id).ok_or_else(|| {
                AttemptOutcomeError::Transition("unknown attempt in DecideCandidate".into())
            })?;
            record.candidate_ref = Some(candidate_ref.clone());
            record.decision_evidence = Some(evidence.clone());
            record.closure_digest = Some(evidence.closure_digest.clone());
            record.status = if *accepted {
                AttemptOutcomeStatus::Succeeded
            } else {
                AttemptOutcomeStatus::Failed
            };
            ledger.scheduling = TaskSchedulingDimension::Closed;
            ledger.validate()?;
        }
        LifecycleAction::CloseAttempt {
            attempt_id,
            closure_digest,
            reason,
            cancelled,
        } => {
            let record = ledger.outcomes.get_mut(attempt_id).ok_or_else(|| {
                AttemptOutcomeError::Transition("unknown attempt in CloseAttempt".into())
            })?;
            record.closure_digest = Some(closure_digest.clone());
            record.reason = Some(reason.clone());
            record.status = if *cancelled {
                AttemptOutcomeStatus::Cancelled
            } else {
                AttemptOutcomeStatus::Failed
            };
            ledger.scheduling = TaskSchedulingDimension::Closed;
            ledger.validate()?;
        }
        LifecycleAction::Retry {
            attempt_id,
            closure_digest,
        } => {
            let record = ledger.outcomes.get_mut(attempt_id).ok_or_else(|| {
                AttemptOutcomeError::Transition("unknown attempt in Retry".into())
            })?;
            if !matches!(
                record.status,
                AttemptOutcomeStatus::Failed | AttemptOutcomeStatus::Cancelled
            ) || record.closure_digest.as_ref() != Some(closure_digest)
            {
                return Err(AttemptOutcomeError::Transition(
                    "retry requires matching closed failed/cancelled attempt outcome".into(),
                ));
            }
            record.superseded_by_retry = true;
            ledger.active_attempt_id = None;
            let v1_task =
                next_projection
                    .tasks
                    .get(&command.task_id)
                    .ok_or(AttemptOutcomeError::Invalid(
                        "missing retried task in projection",
                    ))?;
            ledger.scheduling = TaskSchedulingDimension::from_v1_state(&v1_task.state);
            ledger.validate()?;
        }
        LifecycleAction::CancelPending { reason } => {
            ledger.pending_cancel_reason = Some(reason.clone());
            ledger.scheduling = TaskSchedulingDimension::Closed;
            ledger.validate()?;
        }
        LifecycleAction::ReviseContract { contract } => {
            let digest =
                content_digest(contract).map_err(|_| AttemptOutcomeError::Serialization)?;
            ledger.current_contract_revision = contract.revision;
            ledger.current_contract_digest = digest;
            ledger.max_attempts = contract.max_attempts;
            let v1_task =
                next_projection
                    .tasks
                    .get(&command.task_id)
                    .ok_or(AttemptOutcomeError::Invalid(
                        "missing revised task in projection",
                    ))?;
            ledger.scheduling = TaskSchedulingDimension::from_v1_state(&v1_task.state);
            ledger.validate()?;
        }
    }
    Ok(())
}
