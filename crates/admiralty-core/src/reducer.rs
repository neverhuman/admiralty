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
    Accepted,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TaskLifecycleView {
    pub revision: u32,
    pub attempts: BTreeMap<AttemptId, PrincipalId>,
    pub closure_digest: Option<String>,
    pub decision: Option<CandidateDecisionEvidence>,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct TaskView {
    pub contract: TaskContract,
    pub state: SchedulingState,
    pub attempt_id: Option<AttemptId>,
    pub owner: Option<PrincipalId>,
    pub candidate_ref: Option<String>,
    /// Absent for unchanged v1 projections; any native metadata requires v2.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lifecycle: Option<TaskLifecycleView>,
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
                    state: if dependencies_accepted(state, contract) {
                        SchedulingState::Ready
                    } else {
                        SchedulingState::Blocked
                    },
                    attempt_id: None,
                    owner: None,
                    candidate_ref: None,
                    lifecycle: None,
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
                || task.lifecycle.as_ref().is_some_and(|l| {
                    l.attempts.contains_key(attempt_id)
                        || l.attempts.len() >= usize::from(task.contract.max_attempts)
                })
            {
                return Err(fail("stale revision, unresolved owner or task not ready"));
            }
            task.attempt_id = Some(attempt_id.clone());
            task.owner = Some(owner.clone());
            task.state = SchedulingState::Claimed;
            if let Some(lifecycle) = &mut task.lifecycle {
                lifecycle.attempts.insert(attempt_id.clone(), owner.clone());
            }
            advance_lifecycle(task)?;
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
            advance_lifecycle(task)?;
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
            advance_lifecycle(task)?;
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
        DomainEvent::TaskLifecycle { command } => apply_lifecycle(state, &mut next, command)?,
    }
    Ok(next)
}

fn advance_lifecycle(task: &mut TaskView) -> Result<(), TransitionError> {
    if let Some(lifecycle) = &mut task.lifecycle {
        lifecycle.revision = lifecycle
            .revision
            .checked_add(1)
            .ok_or_else(|| TransitionError("lifecycle revision exhausted".into()))?;
    }
    Ok(())
}

fn dependencies_accepted(state: &Projection, contract: &TaskContract) -> bool {
    contract.dependencies.iter().all(|id| {
        state
            .tasks
            .get(id)
            .is_some_and(|task| task.state == SchedulingState::Accepted)
    })
}

fn valid_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|v| {
        v.len() == 64
            && v.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

fn valid_git_oid(value: &str) -> bool {
    matches!(value.len(), 40 | 64)
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn valid_reason(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 1024 && !value.chars().any(char::is_control)
}

fn apply_lifecycle(
    state: &Projection,
    next: &mut Projection,
    command: &LifecycleCommand,
) -> Result<(), TransitionError> {
    let fail = |s: &str| TransitionError(s.into());
    if command.protocol_version != 2 || !valid_digest(&command.expected_task_digest) {
        return Err(fail("unsupported lifecycle protocol or invalid CAS digest"));
    }
    let original = state
        .tasks
        .get(&command.task_id)
        .ok_or_else(|| fail("unknown task"))?;
    if content_digest(original).map_err(|e| fail(&e.to_string()))? != command.expected_task_digest {
        return Err(fail(
            "task CAS changed; read current state before issuing a new command",
        ));
    }
    let task = next
        .tasks
        .get_mut(&command.task_id)
        .ok_or_else(|| fail("unknown task"))?;
    let mut lifecycle = task.lifecycle.clone().unwrap_or_default();
    if let (Some(attempt), Some(owner)) = (&task.attempt_id, &task.owner) {
        lifecycle
            .attempts
            .entry(attempt.clone())
            .or_insert_with(|| owner.clone());
    }
    match &command.action {
        LifecycleAction::DecideCandidate {
            attempt_id,
            candidate_ref,
            evidence,
            accepted,
        } => {
            if task.state != SchedulingState::AwaitingVerification
                || task.attempt_id.as_ref() != Some(attempt_id)
                || task.candidate_ref.as_ref() != Some(candidate_ref)
                || candidate_ref.is_empty()
                || candidate_ref.len() > 4096
                || evidence.contract_digest
                    != content_digest(&task.contract).map_err(|e| fail(&e.to_string()))?
                || [
                    &evidence.contract_digest,
                    &evidence.verification_policy_digest,
                    &evidence.receipt_digest,
                    &evidence.custody_digest,
                    &evidence.closure_digest,
                ]
                .iter()
                .any(|v| !valid_digest(v))
                || !valid_git_oid(&evidence.source_commit_sha)
                || !valid_git_oid(&evidence.source_tree_sha)
                || task.owner.as_ref() == Some(&evidence.verifier)
                || command.actor != evidence.verifier
            {
                return Err(fail(
                    "candidate decision does not bind the current independent verification",
                ));
            }
            task.state = if *accepted {
                SchedulingState::Accepted
            } else {
                SchedulingState::Failed
            };
            lifecycle.decision = Some(evidence.clone());
            lifecycle.closure_digest = Some(evidence.closure_digest.clone());
        }
        LifecycleAction::CloseAttempt {
            attempt_id,
            closure_digest,
            reason,
            cancelled,
        } => {
            if !matches!(
                task.state,
                SchedulingState::Claimed
                    | SchedulingState::Running
                    | SchedulingState::AwaitingVerification
            ) || task.attempt_id.as_ref() != Some(attempt_id)
                || !valid_digest(closure_digest)
                || !valid_reason(reason)
            {
                return Err(fail(
                    "attempt closure requires exact active attempt and bounded closure evidence",
                ));
            }
            task.state = if *cancelled {
                SchedulingState::Cancelled
            } else {
                SchedulingState::Failed
            };
            lifecycle.closure_digest = Some(closure_digest.clone());
            lifecycle.reason = Some(reason.clone());
        }
        LifecycleAction::Retry {
            attempt_id,
            closure_digest,
        } => {
            if !matches!(
                task.state,
                SchedulingState::Failed | SchedulingState::Cancelled
            ) || task.attempt_id.as_ref() != Some(attempt_id)
                || lifecycle.closure_digest.as_ref() != Some(closure_digest)
                || !valid_digest(closure_digest)
                || lifecycle.attempts.len() >= usize::from(task.contract.max_attempts)
            {
                return Err(fail(
                    "retry requires current closed attempt and remaining attempt budget",
                ));
            }
            task.state = if dependencies_accepted(state, &task.contract) {
                SchedulingState::Ready
            } else {
                SchedulingState::Blocked
            };
            task.attempt_id = None;
            task.owner = None;
            task.candidate_ref = None;
            lifecycle.closure_digest = None;
            lifecycle.decision = None;
            lifecycle.reason = None;
        }
        LifecycleAction::CancelPending { reason } => {
            if !matches!(
                task.state,
                SchedulingState::Ready | SchedulingState::Blocked
            ) || task.attempt_id.is_some()
                || !valid_reason(reason)
            {
                return Err(fail(
                    "pending cancellation cannot close a live or ambiguous attempt",
                ));
            }
            task.state = SchedulingState::Cancelled;
            lifecycle.reason = Some(reason.clone());
        }
        LifecycleAction::ReviseContract { contract } => {
            contract.validate().map_err(|e| fail(&e.to_string()))?;
            if !matches!(
                task.state,
                SchedulingState::Ready | SchedulingState::Blocked
            ) || task.attempt_id.is_some()
                || !lifecycle.attempts.is_empty()
                || contract.task_id != task.contract.task_id
                || contract.project_id != task.contract.project_id
                || contract.root_id != task.contract.root_id
                || contract.authority_ref != task.contract.authority_ref
                || contract.simulated != task.contract.simulated
                || task.contract.revision.checked_add(1) != Some(contract.revision)
                || state
                    .tasks
                    .values()
                    .any(|t| t.contract.dependencies.contains(&command.task_id))
                || state
                    .messages
                    .values()
                    .any(|message| match &message.payload {
                        MessagePayload::TaskAssigned { task_id, .. }
                        | MessagePayload::Question { task_id, .. }
                        | MessagePayload::ArtifactReady { task_id, .. }
                        | MessagePayload::Handoff { task_id, .. } => task_id == &command.task_id,
                        MessagePayload::Answer { .. } => false,
                    })
                || contract.dependencies.iter().any(|id| {
                    state
                        .tasks
                        .get(id)
                        .is_none_or(|t| t.contract.project_id != contract.project_id)
                })
            {
                return Err(fail(
                    "revision requires an unstarted unreferenced task and immutable identity/authority",
                ));
            }
            // An unreferenced existing task cannot be in a dependency cycle.
            task.contract = contract.clone();
            task.state = if dependencies_accepted(state, contract) {
                SchedulingState::Ready
            } else {
                SchedulingState::Blocked
            };
        }
    }
    task.lifecycle = Some(lifecycle);
    advance_lifecycle(task)?;
    // Accepted dependencies are immutable. Readiness is derived atomically in
    // the same event, never from worker completion prose or child exit status.
    if task.state == SchedulingState::Accepted {
        let ready: Vec<_> = next
            .tasks
            .iter()
            .filter(|(_, t)| {
                t.state == SchedulingState::Blocked && dependencies_accepted(next, &t.contract)
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in ready {
            let dependent = next
                .tasks
                .get_mut(&id)
                .ok_or_else(|| fail("dependent vanished"))?;
            dependent.state = SchedulingState::Ready;
            advance_lifecycle(dependent)?;
        }
    }
    Ok(())
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
