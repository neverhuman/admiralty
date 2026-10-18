use crate::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SchedulingState {
    Blocked,
    Ready,
    Claimed,
    Running,
    AwaitingVerification,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct TaskView {
    pub contract: TaskContract,
    pub state: SchedulingState,
    pub attempt_id: Option<AttemptId>,
    pub owner: Option<PrincipalId>,
    pub candidate_ref: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct Projection {
    pub tasks: BTreeMap<TaskId, TaskView>,
    pub messages: BTreeMap<MessageId, CoordinationMessage>,
    pub acknowledgments: BTreeMap<MessageId, String>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("transition rejected: {0}")]
pub struct TransitionError(pub String);

pub fn reduce(state: &Projection, event: &DomainEvent) -> Result<Projection, TransitionError> {
    let fail = |s: &str| TransitionError(s.into());
    let mut next = state.clone();
    match event {
        DomainEvent::TaskSubmitted { contract } => {
            contract.validate().map_err(|e| fail(&e.to_string()))?;
            if state.tasks.contains_key(&contract.task_id) {
                return Err(fail(
                    "task identity already exists; revisions require an explicit future revision command",
                ));
            }
            if contract
                .dependencies
                .iter()
                .any(|id| !state.tasks.contains_key(id))
            {
                return Err(fail("dependency does not exist"));
            }
            if contract.dependencies.iter().any(|id| {
                state
                    .tasks
                    .get(id)
                    .is_some_and(|t| t.contract.project_id != contract.project_id)
            }) {
                return Err(fail(
                    "cross-project dependencies require a qualified scoped interface contract",
                ));
            }
            next.tasks.insert(
                contract.task_id.clone(),
                TaskView {
                    contract: contract.clone(),
                    state: if contract.dependencies.is_empty() {
                        SchedulingState::Ready
                    } else {
                        SchedulingState::Blocked
                    },
                    attempt_id: None,
                    owner: None,
                    candidate_ref: None,
                },
            );
        }
        DomainEvent::TaskClaimed {
            task_id,
            revision,
            attempt_id,
            owner,
        } => {
            let task = next
                .tasks
                .get_mut(task_id)
                .ok_or_else(|| fail("unknown task"))?;
            if task.contract.revision != *revision
                || task.state != SchedulingState::Ready
                || task.attempt_id.is_some()
            {
                return Err(fail("stale revision, unresolved owner or task not ready"));
            }
            task.attempt_id = Some(attempt_id.clone());
            task.owner = Some(owner.clone());
            task.state = SchedulingState::Claimed;
        }
        DomainEvent::TaskStarted {
            task_id,
            attempt_id,
        } => {
            let task = next
                .tasks
                .get_mut(task_id)
                .ok_or_else(|| fail("unknown task"))?;
            if task.attempt_id.as_ref() != Some(attempt_id)
                || task.state != SchedulingState::Claimed
            {
                return Err(fail("attempt is not the current claim"));
            }
            task.state = SchedulingState::Running;
        }
        DomainEvent::CandidateSubmitted {
            task_id,
            attempt_id,
            candidate_ref,
        } => {
            let task = next
                .tasks
                .get_mut(task_id)
                .ok_or_else(|| fail("unknown task"))?;
            if task.attempt_id.as_ref() != Some(attempt_id)
                || task.state != SchedulingState::Running
                || candidate_ref.is_empty()
            {
                return Err(fail("candidate must belong to current running attempt"));
            }
            task.candidate_ref = Some(candidate_ref.clone());
            task.state = SchedulingState::AwaitingVerification;
        }
        DomainEvent::MessageQueued { message } => {
            message.validate().map_err(|e| fail(&e))?;
            if state.messages.contains_key(&message.message_id) {
                return Err(fail("message ID already exists"));
            }
            let task_id = match &message.payload {
                MessagePayload::TaskAssigned { task_id, .. }
                | MessagePayload::Question { task_id, .. }
                | MessagePayload::ArtifactReady { task_id, .. }
                | MessagePayload::Handoff { task_id, .. } => Some(task_id),
                MessagePayload::Answer { question_id, .. } => {
                    let question = state
                        .messages
                        .get(question_id)
                        .ok_or_else(|| fail("unknown question"))?;
                    if question.project_id != message.project_id
                        || question.sender != message.recipient
                        || question.recipient != message.sender
                        || question.authority_ref != message.authority_ref
                        || message
                            .causation_id
                            .as_ref()
                            .is_some_and(|id| id != question_id)
                        || !matches!(question.payload, MessagePayload::Question { .. })
                    {
                        return Err(fail(
                            "answer must match the question's project, participants, authority and causation",
                        ));
                    }
                    None
                }
            };
            if let Some(id) = task_id {
                let task = state
                    .tasks
                    .get(id)
                    .ok_or_else(|| fail("message references unknown task"))?;
                if task.contract.project_id != message.project_id {
                    return Err(fail("cross-project message scope mismatch"));
                }
                if task.contract.authority_ref != message.authority_ref {
                    return Err(fail("message authority does not match the task contract"));
                }
                if let MessagePayload::TaskAssigned {
                    revision,
                    contract_digest,
                    ..
                } = &message.payload
                    && (*revision != task.contract.revision
                        || content_digest(&task.contract).map_err(|e| fail(&e.to_string()))?
                            != *contract_digest)
                {
                    return Err(fail("task assignment references a stale contract"));
                }
            }
            next.messages
                .insert(message.message_id.clone(), message.clone());
        }
        DomainEvent::MessageAcknowledged {
            message_id,
            recipient,
            context_epoch,
        } => {
            let message = state
                .messages
                .get(message_id)
                .ok_or_else(|| fail("unknown message"))?;
            if &message.recipient != recipient || &message.context_epoch != context_epoch {
                return Err(fail(
                    "recipient or context epoch mismatch; fresh capsule required",
                ));
            }
            next.acknowledgments
                .insert(message_id.clone(), context_epoch.clone());
        }
    }
    Ok(next)
}

#[cfg(test)]
mod authority_tests {
    use super::*;

    fn fixture() -> (Projection, CoordinationMessage) {
        let contract: TaskContract = serde_json::from_value(serde_json::json!({
            "schema_version":1,"project_id":"authority-project","root_id":"authority-root",
            "task_id":"AUTHORITY-TASK","revision":1,"title":"Bound message authority",
            "objective":"Keep task messages inside the originating grant","non_goals":[],
            "owned_paths":["src/messages.rs"],"forbidden_paths":[],"read_first":[],
            "dependencies":[],"criteria":[{"id":"C1","text":"Grant cannot widen","check_ids":["unit"]}],
            "checks":[{"id":"unit","program":"cargo","args":["test"],"cwd":".","timeout_secs":30}],
            "authority_ref":"grant:task-candidate-only","max_attempts":1,"simulated":true
        })).expect("valid bounded authority fixture contract");
        let state = reduce(
            &Projection::default(),
            &DomainEvent::TaskSubmitted {
                contract: contract.clone(),
            },
        )
        .expect("submit bounded authority fixture");
        let message = CoordinationMessage {
            schema_version: 1,
            message_id: MessageId::new("authority-message").expect("fixture message identity"),
            project_id: contract.project_id,
            sender: PrincipalId::new("worker").expect("fixture sender"),
            recipient: PrincipalId::new("captain").expect("fixture recipient"),
            causation_id: None,
            context_epoch: "epoch-one".into(),
            authority_ref: contract.authority_ref,
            payload: MessagePayload::Question {
                task_id: contract.task_id,
                requirement: "C1".into(),
                question: "Which interface applies?".into(),
                options: vec!["v1".into(), "v2".into()],
            },
        };
        (state, message)
    }

    #[test]
    fn task_messages_cannot_substitute_a_broader_authority_reference() {
        let (state, message) = fixture();
        assert!(
            reduce(
                &state,
                &DomainEvent::MessageQueued {
                    message: message.clone()
                }
            )
            .is_ok()
        );
        let mut forged = message;
        forged.authority_ref = "grant:owner-can-publish".into();
        let before = state.clone();
        assert!(reduce(&state, &DomainEvent::MessageQueued { message: forged }).is_err());
        assert_eq!(state, before);
    }

    #[test]
    fn answers_preserve_question_authority_and_causation() {
        let (state, question) = fixture();
        let queued = reduce(
            &state,
            &DomainEvent::MessageQueued {
                message: question.clone(),
            },
        )
        .expect("queue scoped question");
        let mut answer = question.clone();
        answer.message_id = MessageId::new("authority-answer").expect("fixture answer identity");
        answer.sender = question.recipient;
        answer.recipient = question.sender;
        answer.causation_id = Some(question.message_id.clone());
        answer.payload = MessagePayload::Answer {
            question_id: question.message_id,
            answer: "v1".into(),
            decision_ref: None,
        };
        assert!(
            reduce(
                &queued,
                &DomainEvent::MessageQueued {
                    message: answer.clone()
                }
            )
            .is_ok()
        );
        let mut forged = answer.clone();
        forged.authority_ref = "grant:unrelated".into();
        assert!(reduce(&queued, &DomainEvent::MessageQueued { message: forged }).is_err());
        answer.causation_id =
            Some(MessageId::new("unrelated-question").expect("fixture unrelated identity"));
        assert!(reduce(&queued, &DomainEvent::MessageQueued { message: answer }).is_err());
    }
}
