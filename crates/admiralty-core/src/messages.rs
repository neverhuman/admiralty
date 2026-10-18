use crate::{MessageId, PrincipalId, ProjectId, TaskId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MessagePayload {
    TaskAssigned {
        task_id: TaskId,
        revision: u32,
        contract_digest: String,
    },
    Question {
        task_id: TaskId,
        requirement: String,
        question: String,
        options: Vec<String>,
    },
    Answer {
        question_id: MessageId,
        answer: String,
        decision_ref: Option<String>,
    },
    ArtifactReady {
        task_id: TaskId,
        artifact_ref: String,
        interface_revision: String,
    },
    Handoff {
        task_id: TaskId,
        candidate_ref: String,
        failed_check: Option<String>,
        next_action: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CoordinationMessage {
    pub schema_version: u32,
    pub message_id: MessageId,
    pub project_id: ProjectId,
    pub sender: PrincipalId,
    pub recipient: PrincipalId,
    pub causation_id: Option<MessageId>,
    pub context_epoch: String,
    pub authority_ref: String,
    pub payload: MessagePayload,
}

impl CoordinationMessage {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != 1
            || self.authority_ref.is_empty()
            || self.context_epoch.is_empty()
        {
            return Err("message requires supported version, authority and context epoch".into());
        }
        let text = serde_json::to_vec(&self.payload).map_err(|e| e.to_string())?;
        if text.len() > 16_384 {
            return Err("message exceeds bounded 16 KiB payload; use an evidence reference".into());
        }
        Ok(())
    }
}
