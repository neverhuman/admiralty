use crate::{AttemptId, CoordinationMessage, MessageId, PrincipalId, TaskContract, TaskId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DomainEvent {
    TaskSubmitted {
        contract: TaskContract,
    },
    TaskClaimed {
        task_id: TaskId,
        revision: u32,
        attempt_id: AttemptId,
        owner: PrincipalId,
    },
    TaskStarted {
        task_id: TaskId,
        attempt_id: AttemptId,
    },
    CandidateSubmitted {
        task_id: TaskId,
        attempt_id: AttemptId,
        candidate_ref: String,
    },
    MessageQueued {
        message: CoordinationMessage,
    },
    MessageAcknowledged {
        message_id: MessageId,
        recipient: PrincipalId,
        context_epoch: String,
    },
    /// Native protocol v2. Deliberately absent from the frozen v1 schema.
    #[schemars(skip)]
    TaskLifecycle {
        command: LifecycleCommand,
    },
}

/// Schema-only v2 union. The journal's `DomainEvent` is the Rust wire decoder;
/// this type deliberately has no serde codec or competing untagged decoder.
/// It retains old journal bytes without extending the frozen v1 schema.
#[derive(Clone, Debug, JsonSchema)]
#[serde(untagged)]
pub enum DomainEventV2 {
    Legacy(DomainEvent),
    Lifecycle(LifecycleRecord),
}

#[derive(Clone, Debug, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LifecycleRecord {
    TaskLifecycle { command: LifecycleCommand },
}

/// A CAS binds all task state, including contract, candidate and attempt history.
/// This is untrusted input, never proof of the actor's authority or check execution.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LifecycleCommand {
    #[schemars(range(min = 2, max = 2))]
    pub protocol_version: u32,
    pub task_id: TaskId,
    pub expected_task_digest: String,
    pub actor: PrincipalId,
    pub action: LifecycleAction,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum LifecycleAction {
    DecideCandidate {
        attempt_id: AttemptId,
        candidate_ref: String,
        evidence: CandidateDecisionEvidence,
        accepted: bool,
    },
    /// The supervisor must prove closure before recording this fact. Unknown
    /// child/effect outcomes retain ownership and cannot authorize a retry.
    CloseAttempt {
        attempt_id: AttemptId,
        closure_digest: String,
        reason: String,
        cancelled: bool,
    },
    Retry {
        attempt_id: AttemptId,
        closure_digest: String,
    },
    CancelPending {
        reason: String,
    },
    ReviseContract {
        contract: TaskContract,
    },
}

/// Bind trusted verifier execution and custody to the exact candidate. Digests
/// and names alone are claims; Store's configured authority must authenticate
/// these against the runtime's non-deserializable execution evidence.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CandidateDecisionEvidence {
    pub contract_digest: String,
    pub verification_policy_digest: String,
    pub receipt_digest: String,
    pub custody_digest: String,
    /// Independent supervisor closure/fencing evidence, not the check receipt.
    pub closure_digest: String,
    pub source_commit_sha: String,
    pub source_tree_sha: String,
    pub verifier: PrincipalId,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SourceIdentity {
    pub producer_id: String,
    pub incarnation: String,
    /// Decimal string preserves 64-bit precision in TypeScript.
    pub sequence: String,
}

impl SourceIdentity {
    pub fn validate(&self) -> Result<u64, String> {
        if self.producer_id.is_empty() || self.incarnation.is_empty() {
            return Err("producer and incarnation are required".into());
        }
        self.sequence
            .parse::<u64>()
            .ok()
            .filter(|n| *n > 0)
            .filter(|n| n.to_string() == self.sequence)
            .ok_or_else(|| "source sequence must be a canonical positive decimal u64".into())
    }
}
