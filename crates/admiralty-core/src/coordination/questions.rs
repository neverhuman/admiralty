//! Pure, scoped question lifecycle reducer.
//!
//! The caller supplies observed time to deadline evaluation. This module never
//! reads a clock or grants authority based on question text.

use crate::{MessageId, PrincipalId, ProjectId, TaskId};
use std::collections::BTreeMap;

pub const MAX_QUESTION_TEXT_BYTES: usize = 16 * 1024;
pub const MAX_OPTIONS: usize = 32;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QuestionStatus {
    Open,
    Routed,
    Answered { answer: String },
    Superseded { by: MessageId },
    Expired,
    Cancelled,
}

impl QuestionStatus {
    fn terminal(&self) -> bool {
        matches!(
            self,
            Self::Answered { .. } | Self::Superseded { .. } | Self::Expired | Self::Cancelled
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuestionState {
    pub question_id: MessageId,
    pub project_id: ProjectId,
    pub task_id: TaskId,
    /// Exact opaque grant/reference string. It is immutable for this question.
    pub authority_ref: String,
    pub asker: PrincipalId,
    pub recipient: PrincipalId,
    pub requirement: String,
    pub question: String,
    pub options: Vec<String>,
    /// Absolute UTC seconds recorded at opening; None means no deadline.
    pub deadline_utc: Option<u64>,
    pub revision: u64,
    pub status: QuestionStatus,
    applied_events: BTreeMap<MessageId, QuestionEvent>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QuestionEvent {
    Open {
        event_id: MessageId,
        question_id: MessageId,
        project_id: ProjectId,
        task_id: TaskId,
        authority_ref: String,
        asker: PrincipalId,
        recipient: PrincipalId,
        requirement: String,
        question: String,
        options: Vec<String>,
        deadline_utc: Option<u64>,
    },
    Route {
        event_id: MessageId,
        expected_revision: u64,
        authority_ref: String,
        actor: PrincipalId,
        recipient: PrincipalId,
    },
    Answer {
        event_id: MessageId,
        expected_revision: u64,
        authority_ref: String,
        actor: PrincipalId,
        answer: String,
    },
    Supersede {
        event_id: MessageId,
        expected_revision: u64,
        authority_ref: String,
        actor: PrincipalId,
        new_question_id: MessageId,
    },
    Cancel {
        event_id: MessageId,
        expected_revision: u64,
        authority_ref: String,
        actor: PrincipalId,
    },
    EvaluateDeadline {
        event_id: MessageId,
        expected_revision: u64,
        authority_ref: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EscalationProposal {
    pub project_id: ProjectId,
    pub task_id: TaskId,
    pub question_id: MessageId,
    pub authority_ref: String,
    pub requested_by: PrincipalId,
    pub current_recipient: PrincipalId,
    pub expired_at_utc: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuestionReduction {
    pub state: QuestionState,
    /// Expiry creates a scoped proposal, never an answer or a decision.
    pub escalation: Option<EscalationProposal>,
    pub replayed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuestionError(pub &'static str);

impl std::fmt::Display for QuestionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for QuestionError {}

/// Apply one lifecycle event. `now_utc` is consulted only for deadline events.
/// Exact event replay is idempotent; reuse of an event ID with changed content
/// and every stale revision are rejected without changing the input state.
pub fn reduce_question(
    current: &Option<QuestionState>,
    event: &QuestionEvent,
    now_utc: Option<u64>,
) -> Result<QuestionReduction, QuestionError> {
    let event_id = event_id(event);
    if let Some(state) = current {
        if let Some(previous) = state.applied_events.get(event_id) {
            if previous == event {
                return Ok(QuestionReduction {
                    state: state.clone(),
                    escalation: None,
                    replayed: true,
                });
            }
            return Err(QuestionError("event ID reused with different content"));
        }
    }

    if let QuestionEvent::Open {
        event_id,
        question_id,
        project_id,
        task_id,
        authority_ref,
        asker,
        recipient,
        requirement,
        question,
        options,
        deadline_utc,
    } = event
    {
        if current.is_some() {
            return Err(QuestionError("question already exists"));
        }
        validate_text(requirement)?;
        validate_text(question)?;
        if options.len() > MAX_OPTIONS {
            return Err(QuestionError("too many options"));
        }
        if options.iter().any(|option| validate_text(option).is_err()) {
            return Err(QuestionError("option text is empty or too large"));
        }
        if authority_ref.trim().is_empty() || asker == recipient {
            return Err(QuestionError(
                "authority and distinct participants are required",
            ));
        }
        let mut applied_events = BTreeMap::new();
        applied_events.insert(event_id.clone(), event.clone());
        return Ok(QuestionReduction {
            state: QuestionState {
                question_id: question_id.clone(),
                project_id: project_id.clone(),
                task_id: task_id.clone(),
                authority_ref: authority_ref.clone(),
                asker: asker.clone(),
                recipient: recipient.clone(),
                requirement: requirement.clone(),
                question: question.clone(),
                options: options.clone(),
                deadline_utc: *deadline_utc,
                revision: 1,
                status: QuestionStatus::Open,
                applied_events,
            },
            escalation: None,
            replayed: false,
        });
    }

    let state = current
        .as_ref()
        .ok_or(QuestionError("question does not exist"))?;
    let (expected_revision, authority_ref) = expected_and_authority(event);
    if *authority_ref != state.authority_ref {
        return Err(QuestionError("stale or mismatched authority"));
    }
    if *expected_revision != state.revision {
        return Err(QuestionError("stale question revision"));
    }
    if state.status.terminal() {
        return Err(QuestionError("terminal question cannot be replaced"));
    }

    let mut next = state.clone();
    let mut escalation = None;
    match event {
        QuestionEvent::Route {
            event_id,
            actor,
            recipient,
            ..
        } => {
            if actor != &state.asker && actor != &state.recipient {
                return Err(QuestionError("actor is outside question participants"));
            }
            if recipient == actor {
                return Err(QuestionError("route recipient must differ from actor"));
            }
            next.recipient = recipient.clone();
            next.status = QuestionStatus::Routed;
            next.applied_events.insert(event_id.clone(), event.clone());
        }
        QuestionEvent::Answer {
            event_id,
            actor,
            answer,
            ..
        } => {
            if actor != &state.recipient {
                return Err(QuestionError("only current recipient may answer"));
            }
            validate_text(answer)?;
            next.status = QuestionStatus::Answered {
                answer: answer.clone(),
            };
            next.applied_events.insert(event_id.clone(), event.clone());
        }
        QuestionEvent::Supersede {
            event_id,
            actor,
            new_question_id,
            ..
        } => {
            if actor != &state.asker {
                return Err(QuestionError("only asker may supersede"));
            }
            if new_question_id == &state.question_id {
                return Err(QuestionError("supersession requires a new question ID"));
            }
            next.status = QuestionStatus::Superseded {
                by: new_question_id.clone(),
            };
            next.applied_events.insert(event_id.clone(), event.clone());
        }
        QuestionEvent::Cancel {
            event_id, actor, ..
        } => {
            if actor != &state.asker {
                return Err(QuestionError("only asker may cancel"));
            }
            next.status = QuestionStatus::Cancelled;
            next.applied_events.insert(event_id.clone(), event.clone());
        }
        QuestionEvent::EvaluateDeadline { event_id, .. } => {
            let now =
                now_utc.ok_or(QuestionError("clock unknown; deadline cannot be evaluated"))?;
            let deadline = state
                .deadline_utc
                .ok_or(QuestionError("question has no deadline"))?;
            if now <= deadline {
                return Err(QuestionError("deadline has not passed"));
            }
            next.status = QuestionStatus::Expired;
            escalation = Some(EscalationProposal {
                project_id: state.project_id.clone(),
                task_id: state.task_id.clone(),
                question_id: state.question_id.clone(),
                authority_ref: state.authority_ref.clone(),
                requested_by: state.asker.clone(),
                current_recipient: state.recipient.clone(),
                expired_at_utc: now,
            });
            next.applied_events.insert(event_id.clone(), event.clone());
        }
        QuestionEvent::Open { .. } => unreachable!(),
    }
    next.revision = next
        .revision
        .checked_add(1)
        .ok_or(QuestionError("revision overflow"))?;
    Ok(QuestionReduction {
        state: next,
        escalation,
        replayed: false,
    })
}

fn validate_text(value: &str) -> Result<(), QuestionError> {
    if value.trim().is_empty() || value.len() > MAX_QUESTION_TEXT_BYTES {
        Err(QuestionError("text is empty or exceeds 16 KiB"))
    } else {
        Ok(())
    }
}

fn event_id(event: &QuestionEvent) -> &MessageId {
    match event {
        QuestionEvent::Open { event_id, .. }
        | QuestionEvent::Route { event_id, .. }
        | QuestionEvent::Answer { event_id, .. }
        | QuestionEvent::Supersede { event_id, .. }
        | QuestionEvent::Cancel { event_id, .. }
        | QuestionEvent::EvaluateDeadline { event_id, .. } => event_id,
    }
}

fn expected_and_authority(event: &QuestionEvent) -> (&u64, &String) {
    match event {
        QuestionEvent::Route {
            expected_revision,
            authority_ref,
            ..
        }
        | QuestionEvent::Answer {
            expected_revision,
            authority_ref,
            ..
        }
        | QuestionEvent::Supersede {
            expected_revision,
            authority_ref,
            ..
        }
        | QuestionEvent::Cancel {
            expected_revision,
            authority_ref,
            ..
        }
        | QuestionEvent::EvaluateDeadline {
            expected_revision,
            authority_ref,
            ..
        } => (expected_revision, authority_ref),
        QuestionEvent::Open { .. } => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id<T>(value: &str, make: impl FnOnce(String) -> Result<T, crate::InvalidId>) -> T {
        make(value.into()).unwrap()
    }
    fn msg(value: &str) -> MessageId {
        id(value, MessageId::new)
    }
    fn principal(value: &str) -> PrincipalId {
        id(value, PrincipalId::new)
    }
    fn project(value: &str) -> ProjectId {
        id(value, ProjectId::new)
    }
    fn task(value: &str) -> TaskId {
        id(value, TaskId::new)
    }
    fn opened() -> QuestionState {
        let open = QuestionEvent::Open {
            event_id: msg("evt-open"),
            question_id: msg("q-1"),
            project_id: project("p-1"),
            task_id: task("t-1"),
            authority_ref: "grant:task-1:r4".into(),
            asker: principal("worker"),
            recipient: principal("owner"),
            requirement: "R1".into(),
            question: "Which interface revision?".into(),
            options: vec!["v1".into(), "v2".into()],
            deadline_utc: Some(100),
        };
        reduce_question(&None, &open, None).unwrap().state
    }
    fn answer(revision: u64, authority: &str, actor: &str) -> QuestionEvent {
        QuestionEvent::Answer {
            event_id: msg("evt-answer"),
            expected_revision: revision,
            authority_ref: authority.into(),
            actor: principal(actor),
            answer: "v2".into(),
        }
    }

    #[test]
    fn only_recipient_with_current_authority_and_revision_can_answer() {
        let state = opened();
        for bad in [
            answer(1, "grant:revoked", "owner"),
            answer(0, "grant:task-1:r4", "owner"),
            answer(1, "grant:task-1:r4", "intruder"),
        ] {
            assert!(reduce_question(&Some(state.clone()), &bad, None).is_err());
        }
        let accepted =
            reduce_question(&Some(state), &answer(1, "grant:task-1:r4", "owner"), None).unwrap();
        assert_eq!(accepted.state.revision, 2);
        assert_eq!(
            accepted.state.status,
            QuestionStatus::Answered {
                answer: "v2".into()
            }
        );
    }

    #[test]
    fn terminal_question_cannot_be_replaced_and_new_question_needs_new_id() {
        let state = opened();
        let answered = reduce_question(&Some(state), &answer(1, "grant:task-1:r4", "owner"), None)
            .unwrap()
            .state;
        let later = QuestionEvent::Cancel {
            event_id: msg("evt-cancel"),
            expected_revision: 2,
            authority_ref: "grant:task-1:r4".into(),
            actor: principal("worker"),
        };
        assert!(reduce_question(&Some(answered), &later, None).is_err());
        let same_id = QuestionEvent::Supersede {
            event_id: msg("evt-super"),
            expected_revision: 1,
            authority_ref: "grant:task-1:r4".into(),
            actor: principal("worker"),
            new_question_id: msg("q-1"),
        };
        assert!(reduce_question(&Some(opened()), &same_id, None).is_err());
    }

    #[test]
    fn deadline_requires_known_clock_and_strictly_past_boundary() {
        let state = opened();
        let event = QuestionEvent::EvaluateDeadline {
            event_id: msg("evt-expire"),
            expected_revision: 1,
            authority_ref: "grant:task-1:r4".into(),
        };
        assert!(reduce_question(&Some(state.clone()), &event, None).is_err());
        assert!(reduce_question(&Some(state.clone()), &event, Some(100)).is_err());
        let expired = reduce_question(&Some(state), &event, Some(101)).unwrap();
        assert_eq!(expired.state.status, QuestionStatus::Expired);
        assert_eq!(expired.escalation.unwrap().task_id, task("t-1"));
    }

    #[test]
    fn duplicate_event_replay_is_idempotent_but_id_collision_is_rejected() {
        let state = opened();
        let event = answer(1, "grant:task-1:r4", "owner");
        let reduced = reduce_question(&Some(state), &event, None).unwrap();
        let replay = reduce_question(&Some(reduced.state.clone()), &event, None).unwrap();
        assert!(replay.replayed);
        assert_eq!(replay.state, reduced.state);
        let collision = QuestionEvent::Answer {
            event_id: msg("evt-answer"),
            expected_revision: 2,
            authority_ref: "grant:task-1:r4".into(),
            actor: principal("owner"),
            answer: "v1".into(),
        };
        assert!(reduce_question(&Some(reduced.state), &collision, None).is_err());
    }

    #[test]
    fn opening_enforces_text_and_option_bounds() {
        let mut state = QuestionEvent::Open {
            event_id: msg("evt-open"),
            question_id: msg("q-1"),
            project_id: project("p-1"),
            task_id: task("t-1"),
            authority_ref: "grant:r1".into(),
            asker: principal("worker"),
            recipient: principal("owner"),
            requirement: "R1".into(),
            question: "q".into(),
            options: vec![],
            deadline_utc: None,
        };
        if let QuestionEvent::Open { options, .. } = &mut state {
            *options = (0..=MAX_OPTIONS).map(|n| n.to_string()).collect();
        }
        assert!(reduce_question(&None, &state, None).is_err());
        if let QuestionEvent::Open {
            options, question, ..
        } = &mut state
        {
            options.clear();
            *question = "x".repeat(MAX_QUESTION_TEXT_BYTES + 1);
        }
        assert!(reduce_question(&None, &state, None).is_err());
    }
}
