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
            || self.authority_ref.trim().is_empty()
            || self.context_epoch.trim().is_empty()
        {
            return Err("message requires supported version, authority and context epoch".into());
        }
        let nonblank = |value: &str| !value.trim().is_empty();
        let valid_payload = match &self.payload {
            MessagePayload::TaskAssigned {
                revision,
                contract_digest,
                ..
            } => *revision > 0 && nonblank(contract_digest),
            MessagePayload::Question {
                requirement,
                question,
                options,
                ..
            } => {
                nonblank(requirement)
                    && nonblank(question)
                    && options.iter().all(|option| nonblank(option))
            }
            MessagePayload::Answer {
                answer,
                decision_ref,
                ..
            } => nonblank(answer) && decision_ref.as_deref().is_none_or(nonblank),
            MessagePayload::ArtifactReady {
                artifact_ref,
                interface_revision,
                ..
            } => nonblank(artifact_ref) && nonblank(interface_revision),
            MessagePayload::Handoff {
                candidate_ref,
                failed_check,
                next_action,
                ..
            } => {
                nonblank(candidate_ref)
                    && nonblank(next_action)
                    && failed_check.as_deref().is_none_or(nonblank)
            }
        };
        if !valid_payload {
            return Err(
                "message requires nonblank payload facts and a positive assignment revision".into(),
            );
        }
        // Count the entire envelope without allocating its serialized copy. An
        // oversized epoch/authority must not bypass the bounded message channel.
        let mut counter = MessageSize { remaining: 16_384 };
        serde_json::to_writer(&mut counter, self).map_err(|_| {
            "message exceeds bounded 16 KiB envelope; use an evidence reference".to_owned()
        })?;
        Ok(())
    }
}

struct MessageSize {
    remaining: usize,
}

impl std::io::Write for MessageSize {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.remaining = self
            .remaining
            .checked_sub(bytes.len())
            .ok_or_else(|| std::io::Error::other("message size limit"))?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> CoordinationMessage {
        CoordinationMessage {
            schema_version: 1,
            message_id: MessageId::new("bounded-message").expect("fixture identity"),
            project_id: ProjectId::new("bounded-project").expect("fixture project"),
            sender: PrincipalId::new("worker").expect("fixture sender"),
            recipient: PrincipalId::new("captain").expect("fixture recipient"),
            causation_id: None,
            context_epoch: "epoch-one".into(),
            authority_ref: "grant-one".into(),
            payload: MessagePayload::Handoff {
                task_id: TaskId::new("BOUNDED-TASK").expect("fixture task"),
                candidate_ref: "candidate:one".into(),
                failed_check: None,
                next_action: "Verify immutable candidate".into(),
            },
        }
    }

    #[test]
    fn entire_envelope_is_bounded_including_epoch_and_authority() {
        let message = fixture();
        assert!(message.validate().is_ok());
        let mut large_epoch = message.clone();
        large_epoch.context_epoch = "e".repeat(16_384);
        assert!(large_epoch.validate().is_err());
        let mut large_authority = message.clone();
        large_authority.authority_ref = "a".repeat(16_384);
        assert!(large_authority.validate().is_err());
        let mut escaped = message;
        escaped.context_epoch = "\n".repeat(8_192);
        assert!(
            escaped.validate().is_err(),
            "JSON escaping counts toward envelope size"
        );
    }

    #[test]
    fn blank_epoch_authority_or_required_payload_facts_are_rejected() {
        let message = fixture();
        let mut blank = message.clone();
        blank.context_epoch = " \n ".into();
        assert!(blank.validate().is_err());
        blank = message.clone();
        blank.authority_ref = "  ".into();
        assert!(blank.validate().is_err());
        let mut blank_handoff = message;
        if let MessagePayload::Handoff { candidate_ref, .. } = &mut blank_handoff.payload {
            *candidate_ref = " \n ".into();
        }
        assert!(blank_handoff.validate().is_err());
    }
}
