//! Pure, bounded lifecycle reducer. The caller authenticates actors, validates
//! grants, and durably records inputs/proposals. Clock observations that affect
//! reduction are part of immutable events, never read from a replay machine.

use crate::{MessageId, PrincipalId, ProjectId, TaskId, content_digest};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_QUESTION_TEXT_BYTES: usize = 16 * 1024;
pub const MAX_QUESTION_EVENT_BYTES: usize = 16 * 1024;
pub const MAX_AUTHORITY_REF_BYTES: usize = 512;
pub const MAX_OPTIONS: usize = 32;
/// Includes the initial recipient. Routes may never revisit a recipient.
pub const MAX_QUESTION_RECIPIENTS: usize = 32;
/// Opening + at most 31 routes + one terminal transition; no receipt eviction.
pub const MAX_QUESTION_EVENTS: usize = MAX_QUESTION_RECIPIENTS + 1;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct QuestionScope {
    pub question_id: MessageId,
    pub project_id: ProjectId,
    pub task_id: TaskId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QuestionStatus {
    Open,
    Routed,
    Answered { answer: String },
    Superseded { by: MessageId },
    Expired { observed_utc: u64 },
    Cancelled,
}
impl QuestionStatus {
    fn terminal(&self) -> bool {
        matches!(
            self,
            Self::Answered { .. }
                | Self::Superseded { .. }
                | Self::Expired { .. }
                | Self::Cancelled
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuestionState {
    pub question_id: MessageId,
    pub project_id: ProjectId,
    pub task_id: TaskId,
    /// Immutable bounded reference, not proof that the caller holds a grant.
    pub authority_ref: String,
    pub asker: PrincipalId,
    pub recipient: PrincipalId,
    pub requirement: String,
    pub question: String,
    pub options: Vec<String>,
    pub deadline_utc: Option<u64>,
    pub revision: u64,
    pub status: QuestionStatus,
    allowed_recipients: BTreeSet<PrincipalId>,
    visited_recipients: BTreeSet<PrincipalId>,
    /// Bounded SHA-256 receipts preserve collisions without historical text copies.
    applied_events: BTreeMap<MessageId, String>,
}
impl QuestionState {
    pub fn scope(&self) -> QuestionScope {
        QuestionScope {
            question_id: self.question_id.clone(),
            project_id: self.project_id.clone(),
            task_id: self.task_id.clone(),
        }
    }
    pub fn retained_event_count(&self) -> usize {
        self.applied_events.len()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum QuestionEvent {
    Open {
        event_id: MessageId,
        scope: QuestionScope,
        authority_ref: String,
        asker: PrincipalId,
        recipient: PrincipalId,
        /// Caller-validated recipients allowed under this exact scope/grant.
        /// Includes the initial recipient; excludes the asker and duplicates.
        allowed_recipients: Vec<PrincipalId>,
        requirement: String,
        question: String,
        options: Vec<String>,
        deadline_utc: Option<u64>,
    },
    Route {
        event_id: MessageId,
        scope: QuestionScope,
        expected_revision: u64,
        authority_ref: String,
        actor: PrincipalId,
        recipient: PrincipalId,
    },
    Answer {
        event_id: MessageId,
        scope: QuestionScope,
        expected_revision: u64,
        authority_ref: String,
        actor: PrincipalId,
        answer: String,
    },
    Supersede {
        event_id: MessageId,
        scope: QuestionScope,
        expected_revision: u64,
        authority_ref: String,
        actor: PrincipalId,
        new_scope: QuestionScope,
    },
    Cancel {
        event_id: MessageId,
        scope: QuestionScope,
        expected_revision: u64,
        authority_ref: String,
        actor: PrincipalId,
    },
    EvaluateDeadline {
        event_id: MessageId,
        scope: QuestionScope,
        expected_revision: u64,
        authority_ref: String,
        actor: PrincipalId,
        /// Recorded UTC Unix second; None cannot establish expiry. Included
        /// in event identity, never replaced by the replay machine's clock.
        observed_utc: Option<u64>,
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
    /// Time of the recorded expiry observation, not an inferred deadline.
    pub expired_at_utc: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuestionReduction {
    pub state: QuestionState,
    /// Caller persists proposals atomically; duplicate input emits none again.
    pub escalation: Option<EscalationProposal>,
    pub replayed: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct QuestionError(pub &'static str);

/// Apply a recorded event without I/O. State is a trusted reducer projection;
/// reconstruct it from the journal, not arbitrary JSON. Identity collisions,
/// scope/authority changes, stale revisions and unknown clocks fail unchanged.
pub fn reduce_question(
    current: &Option<QuestionState>,
    event: &QuestionEvent,
) -> Result<QuestionReduction, QuestionError> {
    validate_event(event)?;
    let event_digest = content_digest(event)
        .map_err(|_| QuestionError("cannot fingerprint bounded question event"))?;
    let event_id = event_id(event);
    if let Some(state) = current
        && let Some(previous) = state.applied_events.get(event_id)
    {
        if previous == &event_digest {
            return Ok(QuestionReduction {
                state: state.clone(),
                escalation: None,
                replayed: true,
            });
        }
        return Err(QuestionError("event ID reused with different content"));
    }
    if let QuestionEvent::Open {
        scope,
        authority_ref,
        asker,
        recipient,
        allowed_recipients,
        requirement,
        question,
        options,
        deadline_utc,
        ..
    } = event
    {
        if current.is_some() {
            return Err(QuestionError("question already exists"));
        }
        return Ok(QuestionReduction {
            state: QuestionState {
                question_id: scope.question_id.clone(),
                project_id: scope.project_id.clone(),
                task_id: scope.task_id.clone(),
                authority_ref: authority_ref.clone(),
                asker: asker.clone(),
                recipient: recipient.clone(),
                requirement: requirement.clone(),
                question: question.clone(),
                options: options.clone(),
                deadline_utc: *deadline_utc,
                revision: 1,
                status: QuestionStatus::Open,
                allowed_recipients: allowed_recipients.iter().cloned().collect(),
                visited_recipients: BTreeSet::from([recipient.clone()]),
                applied_events: BTreeMap::from([(event_id.clone(), event_digest)]),
            },
            escalation: None,
            replayed: false,
        });
    }
    let state = current
        .as_ref()
        .ok_or(QuestionError("question does not exist"))?;
    if event_scope(event) != &state.scope() {
        return Err(QuestionError("question, project or task scope mismatch"));
    }
    let (expected_revision, authority_ref) = expected_and_authority(event);
    if authority_ref != &state.authority_ref {
        return Err(QuestionError("stale or mismatched authority"));
    }
    if expected_revision != state.revision {
        return Err(QuestionError("stale question revision"));
    }
    if state.status.terminal() {
        return Err(QuestionError("terminal question cannot be replaced"));
    }
    if state.applied_events.len() >= MAX_QUESTION_EVENTS {
        return Err(QuestionError("retained question event bound reached"));
    }
    let next_revision = state
        .revision
        .checked_add(1)
        .ok_or(QuestionError("revision overflow"))?;
    let mut escalation = None;
    let status = match event {
        QuestionEvent::Route {
            actor, recipient, ..
        } => {
            require_participant(state, actor)?;
            if !state.allowed_recipients.contains(recipient) || recipient == &state.asker {
                return Err(QuestionError(
                    "recipient is outside the scoped route allowlist",
                ));
            }
            if state.visited_recipients.contains(recipient) {
                return Err(QuestionError("route cycle or repeated recipient"));
            }
            // Leave one receipt for an answer/cancel/supersede/expiry.
            if state.visited_recipients.len() >= MAX_QUESTION_RECIPIENTS
                || state.applied_events.len() >= MAX_QUESTION_EVENTS - 1
            {
                return Err(QuestionError(
                    "route bound reached; terminal receipt reserved",
                ));
            }
            QuestionStatus::Routed
        }
        QuestionEvent::Answer { actor, answer, .. } => {
            if actor != &state.recipient {
                return Err(QuestionError("only current recipient may answer"));
            }
            QuestionStatus::Answered {
                answer: answer.clone(),
            }
        }
        QuestionEvent::Supersede {
            actor, new_scope, ..
        } => {
            if actor != &state.asker {
                return Err(QuestionError("only asker may supersede"));
            }
            if new_scope.question_id == state.question_id
                || new_scope.project_id != state.project_id
                || new_scope.task_id != state.task_id
            {
                return Err(QuestionError(
                    "supersession requires a new ID in the same task scope",
                ));
            }
            QuestionStatus::Superseded {
                by: new_scope.question_id.clone(),
            }
        }
        QuestionEvent::Cancel { actor, .. } => {
            if actor != &state.asker {
                return Err(QuestionError("only asker may cancel"));
            }
            QuestionStatus::Cancelled
        }
        QuestionEvent::EvaluateDeadline {
            actor,
            observed_utc,
            ..
        } => {
            require_participant(state, actor)?;
            let now =
                observed_utc.ok_or(QuestionError("clock unknown; deadline cannot be evaluated"))?;
            let deadline = state
                .deadline_utc
                .ok_or(QuestionError("question has no deadline"))?;
            if now <= deadline {
                return Err(QuestionError("deadline has not passed"));
            }
            escalation = Some(EscalationProposal {
                project_id: state.project_id.clone(),
                task_id: state.task_id.clone(),
                question_id: state.question_id.clone(),
                authority_ref: state.authority_ref.clone(),
                requested_by: state.asker.clone(),
                current_recipient: state.recipient.clone(),
                expired_at_utc: now,
            });
            QuestionStatus::Expired { observed_utc: now }
        }
        QuestionEvent::Open { .. } => unreachable!("opening handled above"),
    };
    // Validate before cloning bounded state and adding the next immutable receipt.
    let mut next = state.clone();
    if let QuestionEvent::Route { recipient, .. } = event {
        next.recipient = recipient.clone();
        next.visited_recipients.insert(recipient.clone());
    }
    next.status = status;
    next.revision = next_revision;
    next.applied_events.insert(event_id.clone(), event_digest);
    Ok(QuestionReduction {
        state: next,
        escalation,
        replayed: false,
    })
}

fn require_participant(state: &QuestionState, actor: &PrincipalId) -> Result<(), QuestionError> {
    if actor == &state.asker || actor == &state.recipient {
        Ok(())
    } else {
        Err(QuestionError(
            "actor is outside current question participants",
        ))
    }
}
fn validate_text(value: &str) -> Result<(), QuestionError> {
    if value.trim().is_empty() || value.len() > MAX_QUESTION_TEXT_BYTES {
        Err(QuestionError("text is empty or exceeds 16 KiB"))
    } else {
        Ok(())
    }
}
fn validate_event(event: &QuestionEvent) -> Result<(), QuestionError> {
    let authority = match event {
        QuestionEvent::Open { authority_ref, .. } => authority_ref,
        _ => expected_and_authority(event).1,
    };
    if authority.is_empty()
        || authority.len() > MAX_AUTHORITY_REF_BYTES
        || authority
            .chars()
            .any(|ch| ch.is_control() || ch.is_whitespace())
    {
        return Err(QuestionError(
            "authority requires a bounded opaque reference",
        ));
    }
    match event {
        QuestionEvent::Open {
            asker,
            recipient,
            allowed_recipients,
            requirement,
            question,
            options,
            ..
        } => {
            validate_text(requirement)?;
            validate_text(question)?;
            if options.len() > MAX_OPTIONS {
                return Err(QuestionError("too many options"));
            }
            for option in options {
                validate_text(option)?;
            }
            if allowed_recipients.is_empty() || allowed_recipients.len() > MAX_QUESTION_RECIPIENTS {
                return Err(QuestionError("invalid bounded recipient allowlist"));
            }
            let unique: BTreeSet<_> = allowed_recipients.iter().collect();
            if asker == recipient
                || unique.len() != allowed_recipients.len()
                || unique.contains(asker)
                || !unique.contains(recipient)
            {
                return Err(QuestionError(
                    "distinct allowlisted recipients are required",
                ));
            }
        }
        QuestionEvent::Answer { answer, .. } => validate_text(answer)?,
        _ => {}
    }
    // Count escaping and the complete envelope before copying any supplied text.
    serde_json::to_writer(EventSize(MAX_QUESTION_EVENT_BYTES), event)
        .map_err(|_| QuestionError("question event exceeds 16 KiB serialized bound"))?;
    Ok(())
}
struct EventSize(usize);
impl std::io::Write for EventSize {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 = self
            .0
            .checked_sub(bytes.len())
            .ok_or_else(|| std::io::Error::other("question event byte limit"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
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
fn event_scope(event: &QuestionEvent) -> &QuestionScope {
    match event {
        QuestionEvent::Open { scope, .. }
        | QuestionEvent::Route { scope, .. }
        | QuestionEvent::Answer { scope, .. }
        | QuestionEvent::Supersede { scope, .. }
        | QuestionEvent::Cancel { scope, .. }
        | QuestionEvent::EvaluateDeadline { scope, .. } => scope,
    }
}
fn expected_and_authority(event: &QuestionEvent) -> (u64, &String) {
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
        } => (*expected_revision, authority_ref),
        QuestionEvent::Open { .. } => unreachable!("opening has no expected revision"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn msg(value: &str) -> MessageId {
        MessageId::new(value).expect("valid fixture message ID")
    }
    fn principal(value: &str) -> PrincipalId {
        PrincipalId::new(value).expect("valid fixture principal")
    }
    fn scope() -> QuestionScope {
        QuestionScope {
            question_id: msg("q-1"),
            project_id: ProjectId::new("p-1").expect("valid fixture project"),
            task_id: TaskId::new("t-1").expect("valid fixture task"),
        }
    }
    fn opening() -> QuestionEvent {
        QuestionEvent::Open {
            event_id: msg("evt-open"),
            scope: scope(),
            authority_ref: "grant:task-1:r4".into(),
            asker: principal("worker"),
            recipient: principal("owner"),
            allowed_recipients: vec![principal("owner"), principal("reviewer")],
            requirement: "R1: do not publish; timeout <= 5 ms".into(),
            question: "Which interface revision?".into(),
            options: vec!["v1".into(), "v2".into()],
            deadline_utc: Some(100),
        }
    }
    fn opened() -> QuestionState {
        reduce_question(&None, &opening())
            .expect("valid bounded opening")
            .state
    }
    fn answer(revision: u64, authority: &str, actor: &str) -> QuestionEvent {
        QuestionEvent::Answer {
            event_id: msg("evt-answer"),
            scope: scope(),
            expected_revision: revision,
            authority_ref: authority.into(),
            actor: principal(actor),
            answer: "v2; do not publish; timeout <= 5 ms".into(),
        }
    }
    fn route(state: &QuestionState, id: &str, actor: &str, recipient: &str) -> QuestionEvent {
        QuestionEvent::Route {
            event_id: msg(id),
            scope: state.scope(),
            expected_revision: state.revision,
            authority_ref: state.authority_ref.clone(),
            actor: principal(actor),
            recipient: principal(recipient),
        }
    }
    fn deadline(observed_utc: Option<u64>) -> QuestionEvent {
        QuestionEvent::EvaluateDeadline {
            event_id: msg("evt-expire"),
            scope: scope(),
            expected_revision: 1,
            authority_ref: "grant:task-1:r4".into(),
            actor: principal("worker"),
            observed_utc,
        }
    }

    #[test]
    fn only_current_recipient_with_exact_authority_and_revision_can_answer() {
        for bad in [
            answer(1, "grant:revoked", "owner"),
            answer(0, "grant:task-1:r4", "owner"),
            answer(1, "grant:task-1:r4", "intruder"),
        ] {
            assert!(reduce_question(&Some(opened()), &bad).is_err());
        }
        let accepted = reduce_question(&Some(opened()), &answer(1, "grant:task-1:r4", "owner"))
            .expect("current scoped recipient answers");
        assert_eq!(accepted.state.revision, 2);
        assert_eq!(
            accepted.state.status,
            QuestionStatus::Answered {
                answer: "v2; do not publish; timeout <= 5 ms".into(),
            }
        );
        assert_eq!(
            accepted.state.requirement,
            "R1: do not publish; timeout <= 5 ms"
        );
    }

    #[test]
    fn exact_question_project_and_task_are_required_even_if_grant_and_revision_match() {
        for field in ["question", "project", "task"] {
            let mut event = answer(1, "grant:task-1:r4", "owner");
            if let QuestionEvent::Answer { scope, .. } = &mut event {
                match field {
                    "question" => scope.question_id = msg("q-other"),
                    "project" => {
                        scope.project_id = ProjectId::new("other").expect("fixture project")
                    }
                    _ => scope.task_id = TaskId::new("other").expect("fixture task"),
                }
            }
            let current = Some(opened());
            let before = current.clone();
            assert_eq!(
                reduce_question(&current, &event),
                Err(QuestionError("question, project or task scope mismatch"))
            );
            assert_eq!(current, before);
        }
    }

    #[test]
    fn routes_require_current_actor_allowlist_and_cannot_revisit_recipients_or_asker() {
        let state = opened();
        for event in [
            route(&state, "bad-actor", "intruder", "reviewer"),
            route(&state, "bad-recipient", "owner", "outside"),
            route(&state, "same-recipient", "worker", "owner"),
            route(&state, "asker-recipient", "owner", "worker"),
        ] {
            assert!(reduce_question(&Some(state.clone()), &event).is_err());
        }
        let routed = reduce_question(
            &Some(state.clone()),
            &route(&state, "route-once", "owner", "reviewer"),
        )
        .expect("first scoped route")
        .state;
        assert_eq!(routed.recipient, principal("reviewer"));
        assert_eq!(routed.authority_ref, state.authority_ref);
        assert!(
            reduce_question(
                &Some(routed.clone()),
                &route(&routed, "cycle", "reviewer", "owner")
            )
            .is_err()
        );
        assert!(
            reduce_question(
                &Some(routed.clone()),
                &answer(2, "grant:task-1:r4", "owner")
            )
            .is_err(),
            "former recipient cannot answer"
        );
        assert!(reduce_question(&Some(routed), &answer(2, "grant:task-1:r4", "reviewer")).is_ok());
    }

    #[test]
    fn terminal_truth_cannot_be_replaced_and_supersession_stays_inside_task_scope() {
        let answered = reduce_question(&Some(opened()), &answer(1, "grant:task-1:r4", "owner"))
            .expect("answer terminal")
            .state;
        let cancel = QuestionEvent::Cancel {
            event_id: msg("evt-cancel"),
            scope: scope(),
            expected_revision: 2,
            authority_ref: "grant:task-1:r4".into(),
            actor: principal("worker"),
        };
        assert!(reduce_question(&Some(answered), &cancel).is_err());
        for field in ["same-id", "foreign-project", "foreign-task"] {
            let mut new_scope = scope();
            if field != "same-id" {
                new_scope.question_id = msg("q-next");
            }
            if field == "foreign-project" {
                new_scope.project_id = ProjectId::new("other").expect("fixture project");
            }
            if field == "foreign-task" {
                new_scope.task_id = TaskId::new("other").expect("fixture task");
            }
            let invalid = QuestionEvent::Supersede {
                event_id: msg("evt-super"),
                scope: scope(),
                expected_revision: 1,
                authority_ref: "grant:task-1:r4".into(),
                actor: principal("worker"),
                new_scope,
            };
            assert!(reduce_question(&Some(opened()), &invalid).is_err());
        }
        let new_scope = QuestionScope {
            question_id: msg("q-next"),
            ..scope()
        };
        let valid = QuestionEvent::Supersede {
            event_id: msg("evt-super"),
            scope: scope(),
            expected_revision: 1,
            authority_ref: "grant:task-1:r4".into(),
            actor: principal("worker"),
            new_scope,
        };
        assert_eq!(
            reduce_question(&Some(opened()), &valid)
                .expect("same task supersession")
                .state
                .status,
            QuestionStatus::Superseded { by: msg("q-next") }
        );
    }

    #[test]
    fn deadline_requires_recorded_known_time_strict_boundary_and_current_actor() {
        for observation in [None, Some(99), Some(100)] {
            assert!(reduce_question(&Some(opened()), &deadline(observation)).is_err());
        }
        let mut unauthorized = deadline(Some(101));
        if let QuestionEvent::EvaluateDeadline { actor, .. } = &mut unauthorized {
            *actor = principal("intruder");
        }
        assert!(reduce_question(&Some(opened()), &unauthorized).is_err());
        let expired = reduce_question(&Some(opened()), &deadline(Some(101)))
            .expect("strictly past recorded deadline");
        assert_eq!(
            expired.state.status,
            QuestionStatus::Expired { observed_utc: 101 }
        );
        let proposal = expired.escalation.expect("scoped expiry proposal");
        assert_eq!(proposal.task_id, scope().task_id);
        assert_eq!(proposal.question_id, scope().question_id);
        assert_eq!(proposal.expired_at_utc, 101);
        assert!(
            reduce_question(&Some(expired.state), &answer(2, "grant:task-1:r4", "owner")).is_err(),
            "late answer cannot erase terminal expiry"
        );
    }

    #[test]
    fn changed_clock_is_identity_collision_and_fresh_replay_uses_the_same_observation() {
        let event = deadline(Some(101));
        let first = reduce_question(&Some(opened()), &event).expect("initial expiry");
        let reconstructed = reduce_question(&Some(opened()), &event).expect("recorded replay");
        assert_eq!(first, reconstructed);
        let replay = reduce_question(&Some(first.state.clone()), &event).expect("exact replay");
        assert!(replay.replayed);
        assert_eq!(replay.state, first.state);
        assert!(replay.escalation.is_none());
        for observation in [None, Some(99), Some(999)] {
            assert_eq!(
                reduce_question(&Some(first.state.clone()), &deadline(observation)),
                Err(QuestionError("event ID reused with different content"))
            );
        }
        let mut max_open = opening();
        if let QuestionEvent::Open { deadline_utc, .. } = &mut max_open {
            *deadline_utc = Some(u64::MAX);
        }
        let max_state = reduce_question(&None, &max_open)
            .expect("maximum deadline")
            .state;
        assert!(reduce_question(&Some(max_state), &deadline(Some(u64::MAX))).is_err());
        let mut no_deadline = opening();
        if let QuestionEvent::Open { deadline_utc, .. } = &mut no_deadline {
            *deadline_utc = None;
        }
        let state = reduce_question(&None, &no_deadline)
            .expect("no deadline permitted")
            .state;
        assert!(reduce_question(&Some(state), &deadline(Some(101))).is_err());
    }

    #[test]
    fn duplicate_identity_is_idempotent_and_changed_protected_text_is_rejected() {
        let event = answer(1, "grant:task-1:r4", "owner");
        let reduced = reduce_question(&Some(opened()), &event).expect("answer accepted");
        let replay = reduce_question(&Some(reduced.state.clone()), &event).expect("exact replay");
        assert!(replay.replayed);
        assert_eq!(replay.state, reduced.state);
        let mut collision = event;
        if let QuestionEvent::Answer { answer, .. } = &mut collision {
            *answer = "publish immediately; timeout <= 5 seconds".into();
        }
        assert!(reduce_question(&Some(reduced.state), &collision).is_err());
    }

    #[test]
    fn aggregate_text_escaping_authority_and_allowlist_are_bounded_before_admission() {
        let mut variants = vec![];
        let mut too_many = opening();
        if let QuestionEvent::Open { options, .. } = &mut too_many {
            *options = (0..=MAX_OPTIONS).map(|n| n.to_string()).collect();
        }
        variants.push(too_many);
        let mut aggregate = opening();
        if let QuestionEvent::Open { options, .. } = &mut aggregate {
            *options = vec!["x".repeat(1024); MAX_OPTIONS];
        }
        variants.push(aggregate);
        let mut escaped = opening();
        if let QuestionEvent::Open { question, .. } = &mut escaped {
            *question = "\n".repeat(MAX_QUESTION_EVENT_BYTES / 2);
        }
        variants.push(escaped);
        for authority in [
            "a".repeat(MAX_AUTHORITY_REF_BYTES + 1),
            "grant:\nforged".into(),
            "grant: with-space".into(),
        ] {
            let mut invalid = opening();
            if let QuestionEvent::Open { authority_ref, .. } = &mut invalid {
                *authority_ref = authority;
            }
            variants.push(invalid);
        }
        for recipients in [
            vec![],
            vec![principal("owner"), principal("owner")],
            vec![principal("worker"), principal("owner")],
            vec![principal("reviewer")],
            (0..=MAX_QUESTION_RECIPIENTS)
                .map(|n| principal(&format!("owner-{n}")))
                .collect(),
        ] {
            let mut invalid = opening();
            if let QuestionEvent::Open {
                allowed_recipients, ..
            } = &mut invalid
            {
                *allowed_recipients = recipients;
            }
            variants.push(invalid);
        }
        for invalid in variants {
            assert!(reduce_question(&None, &invalid).is_err());
        }
        let mut oversized = answer(1, "grant:task-1:r4", "owner");
        if let QuestionEvent::Answer { answer, .. } = &mut oversized {
            *answer = "x".repeat(MAX_QUESTION_TEXT_BYTES + 1);
        }
        assert!(reduce_question(&Some(opened()), &oversized).is_err());
    }

    #[test]
    fn maximal_routes_preserve_old_collision_receipts_and_reserved_terminal_capacity() {
        let recipients: Vec<_> = (0..MAX_QUESTION_RECIPIENTS)
            .map(|n| principal(&format!("recipient-{n}")))
            .collect();
        let mut open = opening();
        if let QuestionEvent::Open {
            recipient,
            allowed_recipients,
            ..
        } = &mut open
        {
            *recipient = recipients[0].clone();
            *allowed_recipients = recipients.clone();
        }
        let mut state = reduce_question(&None, &open)
            .expect("bounded routes opening")
            .state;
        let mut events = vec![open.clone()];
        for (index, recipient) in recipients.iter().enumerate().skip(1) {
            let event = route(
                &state,
                &format!("route-{index}"),
                state.recipient.as_str(),
                recipient.as_str(),
            );
            state = reduce_question(&Some(state), &event)
                .expect("unique bounded route")
                .state;
            events.push(event);
        }
        assert_eq!(state.retained_event_count(), MAX_QUESTION_EVENTS - 1);
        assert_eq!(state.visited_recipients.len(), MAX_QUESTION_RECIPIENTS);
        let terminal = answer(state.revision, "grant:task-1:r4", state.recipient.as_str());
        state = reduce_question(&Some(state), &terminal)
            .expect("reserved terminal receipt")
            .state;
        assert_eq!(state.retained_event_count(), MAX_QUESTION_EVENTS);
        assert_eq!(state.applied_events[&msg("evt-open")].len(), 71);
        let replay = reduce_question(&Some(state.clone()), &open).expect("old receipt retained");
        assert!(replay.replayed);
        assert_eq!(replay.state, state);
        let mut collision = events[1].clone();
        if let QuestionEvent::Route { recipient, .. } = &mut collision {
            *recipient = principal("forged");
        }
        assert!(reduce_question(&Some(state), &collision).is_err());
    }

    #[test]
    fn overflow_and_unknown_fields_fail_without_mutating_trusted_projection() {
        let mut state = opened();
        state.revision = u64::MAX;
        let current = Some(state.clone());
        assert_eq!(
            reduce_question(&current, &answer(u64::MAX, "grant:task-1:r4", "owner")),
            Err(QuestionError("revision overflow"))
        );
        assert_eq!(current, Some(state));
        let mut event = serde_json::to_value(opening()).expect("fixture serialized event");
        event["accept_as_decision"] = serde_json::json!(true);
        assert!(serde_json::from_value::<QuestionEvent>(event).is_err());
    }
}
