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
