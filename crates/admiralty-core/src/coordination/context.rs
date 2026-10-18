//! Deterministic, complete role capsules. Rendering conveys facts, never grants authority.
//!
//! Callers must authenticate the input scope and retain the expected epoch/digest.
//! Persistence, retrieval, recipient baselines and token accounting are separate.
use crate::{
    AttemptId, PrincipalId, ProjectId, TaskContract, content_digest, economics::DecimalU128,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, io::Write};

pub const MAX_ROLE_CAPSULE_BYTES: usize = 64 * 1024;
pub const MAX_ROLE_ITEMS: usize = 16;
const MAX_TEXT_BYTES: usize = 4096;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RecipientRole {
    Worker,
    Commodore,
    Admiral,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "observation", rename_all = "snake_case", deny_unknown_fields)]
pub enum FactObservation {
    Known { value: String, source_ref: String },
    Unknown { reason: String },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "observation", rename_all = "snake_case", deny_unknown_fields)]
pub enum BudgetObservation {
    Known {
        remaining: DecimalU128,
        unit: String,
        source_ref: String,
    },
    Unknown {
        reason: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ImmutableFactRef {
    pub source_ref: String,
    pub revision: u32,
    pub digest: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum CheckObservation {
    NotRun {
        reason: String,
    },
    Passed {
        receipt_ref: String,
    },
    Failed {
        criterion_id: String,
        receipt_ref: String,
    },
    Unknown {
        reason: String,
    },
}

/// Full immutable task constraints remain essential, including negations and units.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProtectedTaskFacts {
    pub contract: TaskContract,
    pub immutable_refs: Vec<ImmutableFactRef>,
    pub candidate: FactObservation,
    pub check: CheckObservation,
    pub evidence: Vec<String>,
    pub uncertainty: Vec<String>,
    pub budget: BudgetObservation,
    /// A stated permitted action is input data, not an executable or grant.
    pub next_action: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum DecisionObservation {
    Proposed {
        answer: String,
    },
    Recorded {
        answer: String,
        actor: PrincipalId,
        policy_digest: String,
        receipt_ref: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DecisionFacts {
    pub recipient: RecipientRole,
    pub decision_id: String,
    pub expected_revision: u32,
    pub task: ProtectedTaskFacts,
    pub blocking_requirement: String,
    pub requested_authority: String,
    pub deadline: FactObservation,
    pub options: Vec<String>,
    pub decision: DecisionObservation,
    pub work_that_can_proceed: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FailedApproachFact {
    pub hypothesis: String,
    pub candidate_ref: String,
    pub result: CheckObservation,
    pub negative_evidence: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HandoffFacts {
    pub from: PrincipalId,
    pub to: PrincipalId,
    pub attempt_id: AttemptId,
    pub task: ProtectedTaskFacts,
    pub failed_approaches: Vec<FailedApproachFact>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProjectExceptionFacts {
    pub project_id: ProjectId,
    pub revision: u32,
    pub authority_ref: String,
    pub source_ref: String,
    pub exceptions: Vec<ProtectedTaskFacts>,
    pub uncertainty: Vec<String>,
    pub next_action: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FleetExceptionFacts {
    pub revision: u32,
    pub authority_ref: String,
    pub source_ref: String,
    /// Explicit compilation scope; these labels are not authentication evidence.
    pub project_scope: Vec<ProjectId>,
    pub projects: Vec<ProjectExceptionFacts>,
    pub uncertainty: Vec<String>,
    pub next_action: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(
    tag = "kind",
    content = "facts",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum RoleFacts {
    Decision(DecisionFacts),
    Handoff(HandoffFacts),
    ProjectExceptions(ProjectExceptionFacts),
    FleetExceptions(FleetExceptionFacts),
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct RoleDocument {
    protocol_version: u32,
    recipient: RecipientRole,
    context_epoch: String,
    facts_digest: String,
    authority_granted: bool,
    facts: RoleFacts,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RoleCapsule {
    pub recipient: RecipientRole,
    pub context_epoch: String,
    pub facts_digest: String,
    pub rendered_digest: String,
    pub text: String,
    pub estimated_tokens: usize,
    pub tokenizer: String,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RoleCapsuleError {
    #[error("invalid or oversized protected capsule facts")]
    InvalidFacts,
    #[error("invalid role capsule budget")]
    InvalidBudget,
    #[error("essential capsule exceeds budget; expand under policy or split work, never truncate")]
    BudgetExceeded,
    #[error("capsule does not match expected role, epoch, version or protected-fact digest")]
    BindingMismatch,
}

fn text(value: &str) -> Result<(), RoleCapsuleError> {
    if value.trim().is_empty() || value.len() > MAX_TEXT_BYTES {
        return Err(RoleCapsuleError::InvalidFacts);
    }
    Ok(())
}
fn list<T>(values: &[T]) -> Result<(), RoleCapsuleError> {
    if values.len() > MAX_ROLE_ITEMS {
        return Err(RoleCapsuleError::InvalidFacts);
    }
    Ok(())
}
fn texts(values: &[String]) -> Result<(), RoleCapsuleError> {
    list(values)?;
    values.iter().try_for_each(|value| text(value))
}
fn revision(value: u32) -> Result<(), RoleCapsuleError> {
    if value == 0 {
        Err(RoleCapsuleError::InvalidFacts)
    } else {
        Ok(())
    }
}
fn digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}
fn observation(value: &FactObservation) -> Result<(), RoleCapsuleError> {
    match value {
        FactObservation::Known { value, source_ref } => {
            text(value)?;
            text(source_ref)
        }
        FactObservation::Unknown { reason } => text(reason),
    }
}
fn check(value: &CheckObservation, task: &ProtectedTaskFacts) -> Result<(), RoleCapsuleError> {
    match value {
        CheckObservation::NotRun { reason } | CheckObservation::Unknown { reason } => text(reason),
        CheckObservation::Passed { receipt_ref } => text(receipt_ref),
        CheckObservation::Failed {
            criterion_id,
            receipt_ref,
        } => {
            if !task
                .contract
                .criteria
                .iter()
                .any(|criterion| &criterion.id == criterion_id)
            {
                return Err(RoleCapsuleError::InvalidFacts);
            }
            text(receipt_ref)
        }
    }
}
fn task(value: &ProtectedTaskFacts) -> Result<(), RoleCapsuleError> {
    value
        .contract
        .validate()
        .map_err(|_| RoleCapsuleError::InvalidFacts)?;
    list(&value.immutable_refs)?;
    for reference in &value.immutable_refs {
        text(&reference.source_ref)?;
        revision(reference.revision)?;
        if !digest(&reference.digest) {
            return Err(RoleCapsuleError::InvalidFacts);
        }
    }
    observation(&value.candidate)?;
    check(&value.check, value)?;
    texts(&value.evidence)?;
    texts(&value.uncertainty)?;
    match &value.budget {
        BudgetObservation::Known {
            unit, source_ref, ..
        } => {
            text(unit)?;
            text(source_ref)?;
        }
        BudgetObservation::Unknown { reason } => text(reason)?,
    }
    text(&value.next_action)
}
fn project(value: &ProjectExceptionFacts) -> Result<(), RoleCapsuleError> {
    revision(value.revision)?;
    text(&value.authority_ref)?;
    text(&value.source_ref)?;
    list(&value.exceptions)?;
    let mut ids = BTreeSet::new();
    for exception in &value.exceptions {
        if exception.contract.project_id != value.project_id
            || !ids.insert(&exception.contract.task_id)
        {
            return Err(RoleCapsuleError::InvalidFacts);
        }
        task(exception)?;
    }
    texts(&value.uncertainty)?;
    text(&value.next_action)
}
impl RoleFacts {
    fn validate(&self) -> Result<RecipientRole, RoleCapsuleError> {
        match self {
            Self::Decision(value) => {
                if value.recipient == RecipientRole::Worker {
                    return Err(RoleCapsuleError::InvalidFacts);
                }
                text(&value.decision_id)?;
                revision(value.expected_revision)?;
                task(&value.task)?;
                text(&value.blocking_requirement)?;
                text(&value.requested_authority)?;
                observation(&value.deadline)?;
                texts(&value.options)?;
                texts(&value.work_that_can_proceed)?;
                match &value.decision {
                    DecisionObservation::Proposed { answer } => text(answer)?,
                    DecisionObservation::Recorded {
                        answer,
                        policy_digest,
                        receipt_ref,
                        ..
                    } => {
                        text(answer)?;
                        if !digest(policy_digest) {
                            return Err(RoleCapsuleError::InvalidFacts);
                        }
                        text(receipt_ref)?;
                    }
                }
                Ok(value.recipient)
            }
            Self::Handoff(value) => {
                task(&value.task)?;
                list(&value.failed_approaches)?;
                for approach in &value.failed_approaches {
                    text(&approach.hypothesis)?;
                    text(&approach.candidate_ref)?;
                    check(&approach.result, &value.task)?;
                    texts(&approach.negative_evidence)?;
                }
                Ok(RecipientRole::Worker)
            }
            Self::ProjectExceptions(value) => {
                project(value)?;
                Ok(RecipientRole::Commodore)
            }
            Self::FleetExceptions(value) => {
                revision(value.revision)?;
                text(&value.authority_ref)?;
                text(&value.source_ref)?;
                list(&value.project_scope)?;
                list(&value.projects)?;
                let scope: BTreeSet<_> = value.project_scope.iter().collect();
                if scope.len() != value.project_scope.len() {
                    return Err(RoleCapsuleError::InvalidFacts);
                }
                let mut seen = BTreeSet::new();
                for entry in &value.projects {
                    if !scope.contains(&entry.project_id) || !seen.insert(&entry.project_id) {
                        return Err(RoleCapsuleError::InvalidFacts);
                    }
                    project(entry)?;
                }
                texts(&value.uncertainty)?;
                text(&value.next_action)?;
                Ok(RecipientRole::Admiral)
            }
        }
    }
}

/// Streams JSON into an absolute bound; JSON escaping counts before allocation.
struct BoundedBytes {
    bytes: Vec<u8>,
    limit: usize,
}
impl Write for BoundedBytes {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self
            .bytes
            .len()
            .checked_add(bytes.len())
            .is_none_or(|n| n > self.limit)
        {
            return Err(std::io::Error::other("protected capsule budget exceeded"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn compile(
    facts: RoleFacts,
    epoch: &str,
    max_tokens: usize,
) -> Result<RoleCapsule, RoleCapsuleError> {
    text(epoch)?;
    let limit = max_tokens
        .checked_mul(4)
        .filter(|n| *n > 0)
        .ok_or(RoleCapsuleError::InvalidBudget)?
        .min(MAX_ROLE_CAPSULE_BYTES);
    let recipient = facts.validate()?;
    // Bound facts before hashing or copying any complete serialized rendering.
    crate::limits::bounded_json_size(&facts, limit)
        .map_err(|_| RoleCapsuleError::BudgetExceeded)?;
    let facts_digest = content_digest(&facts).map_err(|_| RoleCapsuleError::InvalidFacts)?;
    let document = RoleDocument {
        protocol_version: 1,
        recipient,
        context_epoch: epoch.into(),
        facts_digest: facts_digest.clone(),
        authority_granted: false,
        facts,
    };
    let mut output = BoundedBytes {
        bytes: Vec::new(),
        limit,
    };
    serde_json::to_writer(&mut output, &document).map_err(|_| RoleCapsuleError::BudgetExceeded)?;
    let rendered_digest = content_digest(&document).map_err(|_| RoleCapsuleError::InvalidFacts)?;
    let text = String::from_utf8(output.bytes).map_err(|_| RoleCapsuleError::InvalidFacts)?;
    Ok(RoleCapsule {
        recipient,
        context_epoch: epoch.into(),
        facts_digest,
        rendered_digest,
        estimated_tokens: text.len().div_ceil(4),
        text,
        tokenizer: "utf8_bytes_div4_estimate".into(),
    })
}

pub fn compile_decision_capsule(
    facts: &DecisionFacts,
    epoch: &str,
    max_tokens: usize,
) -> Result<RoleCapsule, RoleCapsuleError> {
    // Validate the serialized bound before cloning caller-controlled vectors/text.
    bounded_input(facts)?;
    compile(RoleFacts::Decision(facts.clone()), epoch, max_tokens)
}
pub fn compile_handoff(
    facts: &HandoffFacts,
    epoch: &str,
    max_tokens: usize,
) -> Result<RoleCapsule, RoleCapsuleError> {
    bounded_input(facts)?;
    compile(RoleFacts::Handoff(facts.clone()), epoch, max_tokens)
}
pub fn compile_project_exceptions(
    facts: &ProjectExceptionFacts,
    epoch: &str,
    max_tokens: usize,
) -> Result<RoleCapsule, RoleCapsuleError> {
    bounded_input(facts)?;
    compile(
        RoleFacts::ProjectExceptions(facts.clone()),
        epoch,
        max_tokens,
    )
}
pub fn compile_fleet_exceptions(
    facts: &FleetExceptionFacts,
    epoch: &str,
    max_tokens: usize,
) -> Result<RoleCapsule, RoleCapsuleError> {
    bounded_input(facts)?;
    compile(RoleFacts::FleetExceptions(facts.clone()), epoch, max_tokens)
}
fn bounded_input<T: Serialize>(facts: &T) -> Result<(), RoleCapsuleError> {
    crate::limits::bounded_json_size(facts, MAX_ROLE_CAPSULE_BYTES)
        .map(|_| ())
        .map_err(|_| RoleCapsuleError::InvalidFacts)
}

/// Decode against independently retained bindings, not a digest supplied in the
/// same untrusted document. This verifies fidelity only, never authority.
pub fn decode_role_capsule(
    text: &str,
    expected_role: RecipientRole,
    expected_epoch: &str,
    expected_facts_digest: &str,
) -> Result<RoleFacts, RoleCapsuleError> {
    if text.len() > MAX_ROLE_CAPSULE_BYTES
        || !digest(expected_facts_digest)
        || self::text(expected_epoch).is_err()
    {
        return Err(RoleCapsuleError::BindingMismatch);
    }
    let document: RoleDocument =
        serde_json::from_str(text).map_err(|_| RoleCapsuleError::BindingMismatch)?;
    if document.protocol_version != 1
        || document.authority_granted
        || document.recipient != expected_role
        || document.context_epoch != expected_epoch
        || document.facts_digest != expected_facts_digest
        || document.facts.validate()? != expected_role
        || content_digest(&document.facts).map_err(|_| RoleCapsuleError::InvalidFacts)?
            != expected_facts_digest
    {
        return Err(RoleCapsuleError::BindingMismatch);
    }
    Ok(document.facts)
}
