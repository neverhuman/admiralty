//! Bounded pending-message observations from the writer-owned projection.
//!
//! Durability belongs to the existing Store message/ACK events, not this helper.
//! Reads record no delivery, application ACK, processing progress or authority.
//! A continuation is a lexical live-scan position, not a stable snapshot or a
//! consumer watermark. Start a new pass without a cursor after reconnecting.

use admiralty_core::{
    CoordinationMessage, DomainEvent, MessageId, PrincipalId, ProjectId, Projection, content_digest,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt,
    ops::Bound,
};

pub const MAX_PENDING_PAGE: usize = 16;
pub const MAX_PENDING_SCAN: usize = 256;
pub const MAX_CONTEXT_EPOCH_BYTES: usize = 16_384;
pub const MAX_RECOVERY_GENERATION_BYTES: usize = 64;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub struct InboxScope {
    pub project_id: ProjectId,
    pub recipient: PrincipalId,
    pub context_epoch: String,
}

impl InboxScope {
    pub fn validate(&self) -> Result<(), InboxError> {
        if !bounded_label(&self.context_epoch, MAX_CONTEXT_EPOCH_BYTES) {
            return Err(InboxError::InvalidInput);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InboxCursor {
    pub recovery_generation: String,
    /// Canonical core content digest of the complete, exact requested scope.
    pub scope_digest: String,
    pub after_message_id: MessageId,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PendingInboxPage {
    pub recovery_generation: String,
    pub messages: Vec<CoordinationMessage>,
    /// Continue this live scan after its last examined ID within this scope.
    /// A new pass/reconnect must begin without a continuation.
    pub next_cursor: Option<InboxCursor>,
    pub scanned_messages: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InboxError {
    InvalidInput,
    SnapshotRequired,
    CorruptProjection,
}

impl fmt::Display for InboxError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidInput => "pending inbox request is invalid or outside admission bounds",
            Self::SnapshotRequired => {
                "snapshot_required: pending inbox recovery generation changed"
            }
            Self::CorruptProjection => {
                "pending inbox projection contradicts a validated message or acknowledgment"
            }
        })
    }
}

impl Error for InboxError {}

fn bounded_label(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && !value.trim().is_empty()
        && !value.chars().any(char::is_control)
}

fn valid_scope_digest(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("sha256:")
        && value.as_bytes()[7..]
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
}

fn admitted_scope_digest(
    scope: &InboxScope,
    cursor: Option<&InboxCursor>,
    limit: usize,
) -> Result<String, InboxError> {
    scope.validate()?;
    if !(1..=MAX_PENDING_PAGE).contains(&limit) {
        return Err(InboxError::InvalidInput);
    }
    if let Some(cursor) = cursor
        && (!bounded_label(&cursor.recovery_generation, MAX_RECOVERY_GENERATION_BYTES)
            || !valid_scope_digest(&cursor.scope_digest))
    {
        return Err(InboxError::InvalidInput);
    }
    // Scope strings are bounded before hashing. content_digest streams into its
    // hasher and allocates only the fixed-size digest, not a serialized scope.
    let digest = content_digest(scope).map_err(|_| InboxError::InvalidInput)?;
    if cursor.is_some_and(|cursor| cursor.scope_digest != digest) {
        return Err(InboxError::InvalidInput);
    }
    Ok(digest)
}

/// Reuse before cloning a request into the Store queue and inside the writer.
/// This validates bounds and exact scope binding, not authentication or the
/// current recovery generation. The latter must be compared in the writer turn.
pub fn validate_request(
    scope: &InboxScope,
    cursor: Option<&InboxCursor>,
    limit: usize,
) -> Result<(), InboxError> {
    admitted_scope_digest(scope, cursor, limit).map(|_| ())
}

/// Derived state owned exclusively by the serialized Store writer. Reconstructed
/// from durable message/ACK facts; never a separate source of delivery authority.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct PendingInboxIndex {
    scopes: BTreeMap<InboxScope, BTreeSet<MessageId>>,
}

/// Prepared before committing SQL; application afterward is infallible. Fields
/// are private so the writer cannot invent an unvalidated index mutation.
pub(crate) struct PendingInboxUpdate(Option<(InboxScope, MessageId, bool)>);

impl PendingInboxIndex {
    pub(crate) fn from_projection(projection: &Projection) -> Result<Self, InboxError> {
        let mut index = Self::default();
        for (id, message) in &projection.messages {
            let (scope, pending) = indexed_message(projection, id, message)?;
            if pending {
                index.scopes.entry(scope).or_default().insert(id.clone());
            }
        }
        if projection
            .acknowledgments
            .keys()
            .any(|id| !projection.messages.contains_key(id))
        {
            return Err(InboxError::CorruptProjection);
        }
        Ok(index)
    }

    /// Validate against the post-reduce projection while the SQL transaction
    /// can still roll back. Business-level queue replay after an ACK must retain
    /// that ACK, rather than putting the historical message back in pending.
    pub(crate) fn prepare_update(
        projection: &Projection,
        event: &DomainEvent,
    ) -> Result<PendingInboxUpdate, InboxError> {
        let id = match event {
            DomainEvent::MessageQueued { message } => &message.message_id,
            DomainEvent::MessageAcknowledged { message_id, .. } => message_id,
            _ => return Ok(PendingInboxUpdate(None)),
        };
        let message = projection
            .messages
            .get(id)
            .ok_or(InboxError::CorruptProjection)?;
        let (scope, pending) = indexed_message(projection, id, message)?;
        Ok(PendingInboxUpdate(Some((scope, id.clone(), pending))))
    }

    pub(crate) fn apply_prepared(&mut self, update: PendingInboxUpdate) {
        let Some((scope, id, pending)) = update.0 else {
            return;
        };
        if pending {
            self.scopes.entry(scope).or_default().insert(id);
        } else if let Some(ids) = self.scopes.get_mut(&scope) {
            ids.remove(&id);
            if ids.is_empty() {
                self.scopes.remove(&scope);
            }
        }
    }
}

fn indexed_message(
    projection: &Projection,
    id: &MessageId,
    message: &CoordinationMessage,
) -> Result<(InboxScope, bool), InboxError> {
    if id != &message.message_id || message.validate().is_err() {
        return Err(InboxError::CorruptProjection);
    }
    let pending = match projection.acknowledgments.get(id) {
        None => true,
        Some(epoch) if epoch == &message.context_epoch => false,
        Some(_) => return Err(InboxError::CorruptProjection),
    };
    Ok((
        InboxScope {
            project_id: message.project_id.clone(),
            recipient: message.recipient.clone(),
            context_epoch: message.context_epoch.clone(),
        },
        pending,
    ))
}

/// Observe pending messages in one serialized Store-writer turn.
///
/// Only the exact scope's pending index is consulted. At most 16 validated
/// envelopes are cloned; one borrowed own-scope key determines continuation.
/// Foreign or ACKed history cannot affect cursor IDs, counts or page boundaries.
/// Neither a read nor a cursor acknowledges anything. Actor authorization still
/// belongs to the trusted service before invoking this storage seam.
pub(crate) fn pending_page(
    projection: &Projection,
    index: &PendingInboxIndex,
    generation: &str,
    scope: &InboxScope,
    cursor: Option<&InboxCursor>,
    limit: usize,
) -> Result<PendingInboxPage, InboxError> {
    let scope_digest = admitted_scope_digest(scope, cursor, limit)?;
    if !bounded_label(generation, MAX_RECOVERY_GENERATION_BYTES) {
        return Err(InboxError::InvalidInput);
    }
    if cursor.is_some_and(|cursor| cursor.recovery_generation != generation) {
        return Err(InboxError::SnapshotRequired);
    }

    let Some(ids) = index.scopes.get(scope) else {
        return Ok(PendingInboxPage {
            recovery_generation: generation.to_owned(),
            messages: Vec::new(),
            next_cursor: None,
            scanned_messages: 0,
        });
    };
    let lower = cursor.map_or(Bound::Unbounded, |cursor| {
        Bound::Excluded(&cursor.after_message_id)
    });
    let mut records = ids
        .range::<MessageId, _>((lower, Bound::Unbounded))
        .peekable();
    let mut messages = Vec::new();
    let mut scanned_messages = 0_u32;
    let mut last_examined = None;
    while scanned_messages < MAX_PENDING_SCAN as u32 && messages.len() < limit {
        let Some(id) = records.next() else {
            break;
        };
        scanned_messages += 1;
        last_examined = Some(id);
        let message = projection
            .messages
            .get(id)
            .ok_or(InboxError::CorruptProjection)?;
        let (actual_scope, pending) = indexed_message(projection, id, message)?;
        if &actual_scope != scope || !pending {
            return Err(InboxError::CorruptProjection);
        }
        messages.push(message.clone());
    }
    // peek borrows one key/value pair; it does not inspect or clone that next
    // envelope. No additional record contributes to scanned_messages.
    let has_more = records.peek().is_some();
    let next_cursor = if has_more {
        last_examined.map(|id| InboxCursor {
            recovery_generation: generation.to_owned(),
            scope_digest,
            after_message_id: id.clone(),
        })
    } else {
        None
    };
    Ok(PendingInboxPage {
        recovery_generation: generation.to_owned(),
        messages,
        next_cursor,
        scanned_messages,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use admiralty_core::{MessagePayload, TaskId};

    const GENERATION: &str = "recovery-one";

    // Projection-only fixtures rebuild a fresh derived index. Store tests below
    // exercise incremental updates through the actual serialized SQL writer.
    fn pending_page(
        projection: &Projection,
        generation: &str,
        scope: &InboxScope,
        cursor: Option<&InboxCursor>,
        limit: usize,
    ) -> Result<PendingInboxPage, InboxError> {
        validate_request(scope, cursor, limit)?;
        let index = PendingInboxIndex::from_projection(projection)?;
        super::pending_page(projection, &index, generation, scope, cursor, limit)
    }

    fn scope() -> InboxScope {
        InboxScope {
            project_id: ProjectId::new("project-one").expect("fixture project"),
            recipient: PrincipalId::new("worker-one").expect("fixture recipient"),
            context_epoch: "epoch-one".into(),
        }
    }

    fn message(id: &str) -> CoordinationMessage {
        CoordinationMessage {
            schema_version: 1,
            message_id: MessageId::new(id).expect("fixture message ID"),
            project_id: scope().project_id,
            sender: PrincipalId::new("captain-one").expect("fixture sender"),
            recipient: scope().recipient,
            causation_id: Some(MessageId::new("source-one").expect("fixture causation")),
            context_epoch: scope().context_epoch,
            authority_ref: "grant:task-one-exact".into(),
            payload: MessagePayload::Question {
                task_id: TaskId::new("TASK-ONE").expect("fixture task"),
                requirement: "Do not exceed 5 ms in project-one".into(),
                question: "Keep --strict and --units=ms exactly?\nDo not broaden scope.".into(),
                options: vec![
                    "No; keep 5 ms".into(),
                    "Yes, only after a scoped decision".into(),
                ],
            },
        }
    }

    fn insert(projection: &mut Projection, message: CoordinationMessage) {
        assert!(message.validate().is_ok(), "valid bounded fixture envelope");
        assert!(
            projection
                .messages
                .insert(message.message_id.clone(), message)
                .is_none(),
            "unique fixture identity"
        );
    }

    fn cursor(after: &str) -> InboxCursor {
        InboxCursor {
            recovery_generation: GENERATION.into(),
            scope_digest: content_digest(&scope()).expect("bounded scope digest"),
            after_message_id: MessageId::new(after).expect("fixture cursor ID"),
        }
    }

    fn task_event() -> DomainEvent {
        let contract = serde_json::from_value(serde_json::json!({
            "schema_version":1,"project_id":"project-one","root_id":"root-one",
            "task_id":"TASK-ONE","revision":1,"title":"Indexed inbox fixture",
            "objective":"Preserve scoped pending delivery and durable history","non_goals":[],
            "owned_paths":["src/inbox.rs"],"forbidden_paths":[],"read_first":[],
            "dependencies":[],"criteria":[{"id":"C1","text":"Scoped replay","check_ids":["unit"]}],
            "checks":[{"id":"unit","program":"cargo","args":["test"],"cwd":".","timeout_secs":30}],
            "authority_ref":"grant:task-one-exact","max_attempts":1,"simulated":true
        }))
        .expect("valid fixture task");
        DomainEvent::TaskSubmitted { contract }
    }

    #[test]
    fn exact_project_recipient_and_epoch_preserve_original_protected_facts() {
        let original = message("matching");
        let mut projection = Projection::default();
        insert(&mut projection, original.clone());
        let mut foreign = message("foreign-project");
        foreign.project_id = ProjectId::new("other-project").expect("foreign project");
        insert(&mut projection, foreign);
        let mut other_recipient = message("foreign-recipient");
        other_recipient.recipient = PrincipalId::new("other-worker").expect("foreign recipient");
        insert(&mut projection, other_recipient);
        let mut other_epoch = message("foreign-epoch");
        other_epoch.context_epoch = "epoch-two".into();
        insert(&mut projection, other_epoch);
        let before = projection.clone();
        let page = pending_page(&projection, GENERATION, &scope(), None, 16).expect("pending page");
        assert_eq!(page.messages, vec![original.clone()]);
        assert_eq!(
            serde_json::to_vec(&page.messages[0]).expect("returned envelope"),
            serde_json::to_vec(&original).expect("original envelope")
        );
        assert_eq!(page.scanned_messages, 1);
        assert!(page.next_cursor.is_none());
        assert_eq!(projection, before);
    }

    #[test]
    fn pages_are_lexical_and_retries_never_implicitly_acknowledge() {
        let mut projection = Projection::default();
        for id in ["message-c", "message-a", "message-b"] {
            insert(&mut projection, message(id));
        }
        let first = pending_page(&projection, GENERATION, &scope(), None, 2).expect("first page");
        assert_eq!(
            first
                .messages
                .iter()
                .map(|m| m.message_id.as_str())
                .collect::<Vec<_>>(),
            vec!["message-a", "message-b"]
        );
        assert_eq!(first.scanned_messages, 2);
        assert_eq!(first.next_cursor, Some(cursor("message-b")));
        let second = pending_page(
            &projection,
            GENERATION,
            &scope(),
            first.next_cursor.as_ref(),
            2,
        )
        .expect("second page");
        assert_eq!(second.messages, vec![message("message-c")]);
        assert!(second.next_cursor.is_none());
        assert_eq!(
            pending_page(&projection, GENERATION, &scope(), None, 2).expect("lost-response retry"),
            first
        );
        assert!(projection.acknowledgments.is_empty());
    }

    #[test]
    fn foreign_and_acknowledged_history_does_not_create_empty_scan_pages() {
        let mut projection = Projection::default();
        for index in 0..MAX_PENDING_SCAN * 2 {
            let id = format!("a-{index:04}");
            let mut row = message(&id);
            if index % 2 == 0 {
                row.project_id = ProjectId::new("other-project").expect("foreign project");
            } else {
                projection
                    .acknowledgments
                    .insert(row.message_id.clone(), row.context_epoch.clone());
            }
            insert(&mut projection, row);
        }
        insert(&mut projection, message("z-pending"));
        let before = projection.clone();
        let first = pending_page(&projection, GENERATION, &scope(), None, 16).expect("scan one");
        assert_eq!(first.messages, vec![message("z-pending")]);
        assert_eq!(first.scanned_messages, 1);
        assert!(first.next_cursor.is_none());
        assert_eq!(projection, before);
    }

    #[test]
    fn exact_page_and_scan_endpoints_do_not_invent_another_page() {
        let mut projection = Projection::default();
        for index in 0..MAX_PENDING_PAGE {
            insert(&mut projection, message(&format!("m-{index:02}")));
        }
        let page = pending_page(&projection, GENERATION, &scope(), None, MAX_PENDING_PAGE)
            .expect("exact full page");
        assert_eq!(page.messages.len(), MAX_PENDING_PAGE);
        assert_eq!(page.scanned_messages, MAX_PENDING_PAGE as u32);
        assert!(page.next_cursor.is_none());
        insert(&mut projection, message("z-next"));
        let with_more = pending_page(&projection, GENERATION, &scope(), None, MAX_PENDING_PAGE)
            .expect("one borrowed lookahead");
        assert_eq!(with_more.messages.len(), MAX_PENDING_PAGE);
        assert_eq!(with_more.scanned_messages, MAX_PENDING_PAGE as u32);
        assert_eq!(with_more.next_cursor, Some(cursor("m-15")));

        let mut only_foreign = Projection::default();
        for index in 0..MAX_PENDING_SCAN {
            let mut row = message(&format!("f-{index:03}"));
            row.context_epoch = "foreign-epoch".into();
            insert(&mut only_foreign, row);
        }
        let exhausted =
            pending_page(&only_foreign, GENERATION, &scope(), None, 16).expect("exact scan limit");
        assert!(exhausted.messages.is_empty());
        assert_eq!(exhausted.scanned_messages, 0);
        assert!(exhausted.next_cursor.is_none());
    }

    #[test]
    fn recovery_generation_and_every_scope_field_bind_a_continuation() {
        let projection = Projection::default();
        let original = cursor("after");
        assert_eq!(validate_request(&scope(), Some(&original), 1), Ok(()));
        assert_eq!(
            pending_page(&projection, "recovery-two", &scope(), Some(&original), 1),
            Err(InboxError::SnapshotRequired)
        );
        for changed in [
            InboxScope {
                project_id: ProjectId::new("different-project").expect("foreign project"),
                ..scope()
            },
            InboxScope {
                recipient: PrincipalId::new("different-worker").expect("foreign worker"),
                ..scope()
            },
            InboxScope {
                context_epoch: "different-epoch".into(),
                ..scope()
            },
        ] {
            assert_eq!(
                validate_request(&changed, Some(&original), 1),
                Err(InboxError::InvalidInput)
            );
            assert_eq!(
                pending_page(&projection, GENERATION, &changed, Some(&original), 1),
                Err(InboxError::InvalidInput)
            );
        }
        let mut changed_digest = original;
        changed_digest.scope_digest = format!("sha256:{}", "a".repeat(64));
        assert_eq!(
            validate_request(&scope(), Some(&changed_digest), 1),
            Err(InboxError::InvalidInput)
        );
    }

    #[test]
    fn bounds_use_utf8_bytes_and_reject_blank_control_or_invalid_digest_inputs() {
        let exact = InboxScope {
            context_epoch: "π".repeat(MAX_CONTEXT_EPOCH_BYTES / 2),
            ..scope()
        };
        assert_eq!(exact.validate(), Ok(()));
        for epoch in [
            "".into(),
            " \u{2003} ".into(),
            "epoch\nline".into(),
            "epoch\0".into(),
            "π".repeat(MAX_CONTEXT_EPOCH_BYTES / 2 + 1),
            "e".repeat(MAX_CONTEXT_EPOCH_BYTES + 1),
        ] {
            let bad = InboxScope {
                context_epoch: epoch,
                ..scope()
            };
            assert_eq!(bad.validate(), Err(InboxError::InvalidInput));
            assert_eq!(
                validate_request(&bad, None, 1),
                Err(InboxError::InvalidInput)
            );
        }
        for limit in [0, MAX_PENDING_PAGE + 1, usize::MAX] {
            assert_eq!(
                validate_request(&scope(), None, limit),
                Err(InboxError::InvalidInput)
            );
        }
        let projection = Projection::default();
        for generation in ["".into(), " ".into(), "g\n".into(), "g".repeat(65)] {
            let mut bad = cursor("after");
            bad.recovery_generation = generation.clone();
            assert_eq!(
                validate_request(&scope(), Some(&bad), 1),
                Err(InboxError::InvalidInput)
            );
            assert_eq!(
                pending_page(&projection, &generation, &scope(), None, 1),
                Err(InboxError::InvalidInput)
            );
        }
        assert!(pending_page(&projection, &"π".repeat(32), &scope(), None, 1).is_ok());
        for digest in [
            "".into(),
            "a".repeat(64),
            format!("sha256:{}", "A".repeat(64)),
            format!("sha256:{}", "a".repeat(65)),
        ] {
            let mut bad = cursor("after");
            bad.scope_digest = digest;
            assert_eq!(
                validate_request(&scope(), Some(&bad), 1),
                Err(InboxError::InvalidInput)
            );
        }
    }

    #[test]
    fn exact_ack_epoch_suppresses_only_its_message_and_mismatch_fails_closed() {
        let mut projection = Projection::default();
        let acknowledged = message("acknowledged");
        insert(&mut projection, acknowledged.clone());
        insert(&mut projection, message("still-pending"));
        projection.acknowledgments.insert(
            acknowledged.message_id.clone(),
            acknowledged.context_epoch.clone(),
        );
        let page =
            pending_page(&projection, GENERATION, &scope(), None, 16).expect("valid ACK binding");
        assert_eq!(page.messages, vec![message("still-pending")]);
        projection
            .acknowledgments
            .insert(acknowledged.message_id, "wrong-epoch".into());
        let before = projection.clone();
        assert_eq!(
            pending_page(&projection, GENERATION, &scope(), None, 16),
            Err(InboxError::CorruptProjection)
        );
        assert_eq!(projection, before);
    }

    #[test]
    fn malformed_selected_envelopes_and_mismatched_map_ids_never_return_partial_data() {
        for mutation in 0..5 {
            let mut projection = Projection::default();
            insert(&mut projection, message("a-valid-first"));
            let mut broken = message("z-broken");
            let key = broken.message_id.clone();
            match mutation {
                0 => broken.schema_version = 2,
                1 => broken.authority_ref = " ".into(),
                2 => broken.authority_ref = "a".repeat(16_384),
                3 => broken.message_id = MessageId::new("mismatched-id").expect("test ID"),
                _ => {
                    broken.payload = MessagePayload::Answer {
                        question_id: MessageId::new("question").expect("test question"),
                        answer: " ".into(),
                        decision_ref: None,
                    }
                }
            }
            projection.messages.insert(key, broken);
            let before = projection.clone();
            assert_eq!(
                pending_page(&projection, GENERATION, &scope(), None, 16),
                Err(InboxError::CorruptProjection)
            );
            assert_eq!(projection, before);
        }
    }

    #[test]
    fn insertion_before_continuation_is_observed_on_the_next_fresh_pass() {
        let mut projection = Projection::default();
        insert(&mut projection, message("b-existing"));
        insert(&mut projection, message("d-existing"));
        let first = pending_page(&projection, GENERATION, &scope(), None, 1).expect("first page");
        insert(&mut projection, message("a-late-insert"));
        let continued = pending_page(
            &projection,
            GENERATION,
            &scope(),
            first.next_cursor.as_ref(),
            1,
        )
        .expect("live continuation");
        assert_eq!(continued.messages, vec![message("d-existing")]);
        let restarted =
            pending_page(&projection, GENERATION, &scope(), None, 16).expect("fresh pass");
        assert_eq!(restarted.messages[0], message("a-late-insert"));
        assert_eq!(restarted.messages.len(), 3);
        assert!(projection.acknowledgments.is_empty());
    }

    #[test]
    fn nonexistent_cursor_keys_seek_lexically_without_claiming_processing_progress() {
        let mut projection = Projection::default();
        insert(&mut projection, message("b"));
        insert(&mut projection, message("d"));
        let between = pending_page(&projection, GENERATION, &scope(), Some(&cursor("c")), 16)
            .expect("lexical seek");
        assert_eq!(between.messages, vec![message("d")]);
        let past = pending_page(&projection, GENERATION, &scope(), Some(&cursor("z")), 16)
            .expect("past end");
        assert_eq!(past.scanned_messages, 0);
        assert!(past.messages.is_empty() && past.next_cursor.is_none());
        assert_eq!(projection.messages.len(), 2);
        assert!(projection.acknowledgments.is_empty());
    }

    #[test]
    fn serde_rejects_unknown_fields_duplicate_fields_and_invalid_typed_ids() {
        let mut encoded = serde_json::to_value(scope()).expect("scope JSON");
        encoded["execution_granted"] = true.into();
        assert!(serde_json::from_value::<InboxScope>(encoded).is_err());
        assert!(serde_json::from_str::<InboxScope>(r#"{"project_id":"project-one","recipient":"worker-one","context_epoch":"epoch-one","context_epoch":"epoch-two"}"#).is_err());
        let mut encoded_cursor = serde_json::to_value(cursor("after")).expect("cursor JSON");
        encoded_cursor["after_message_id"] = "../escape".into();
        assert!(serde_json::from_value::<InboxCursor>(encoded_cursor).is_err());
        let mut encoded_page = serde_json::to_value(
            pending_page(&Projection::default(), GENERATION, &scope(), None, 16)
                .expect("empty page"),
        )
        .expect("page JSON");
        encoded_page["application_ack_recorded"] = true.into();
        assert!(serde_json::from_value::<PendingInboxPage>(encoded_page).is_err());
    }

    #[test]
    fn foreign_history_cannot_change_own_cursor_counts_or_page_boundaries() {
        let mut projection = Projection::default();
        insert(&mut projection, message("own-a"));
        insert(&mut projection, message("own-z"));
        let original =
            pending_page(&projection, GENERATION, &scope(), None, 1).expect("original scoped page");
        for number in 0..1024 {
            let mut foreign = message(&format!("foreign-{number:04}"));
            match number % 3 {
                0 => foreign.project_id = ProjectId::new("other-project").expect("project"),
                1 => foreign.recipient = PrincipalId::new("other-recipient").expect("recipient"),
                _ => foreign.context_epoch = "other-epoch".into(),
            }
            insert(&mut projection, foreign);
        }
        assert_eq!(
            pending_page(&projection, GENERATION, &scope(), None, 1),
            Ok(original)
        );
    }

    #[test]
    fn prepared_deltas_match_rebuild_and_queue_replay_never_resurrects_acked_message() {
        let mut projection = admiralty_core::reduce(&Projection::default(), &task_event())
            .expect("task for actual reducer validation");
        let mut index = PendingInboxIndex::default();
        let queued = DomainEvent::MessageQueued {
            message: message("one"),
        };
        let acknowledged = DomainEvent::MessageAcknowledged {
            message_id: message("one").message_id,
            recipient: scope().recipient,
            context_epoch: scope().context_epoch,
        };
        for event in [queued.clone(), acknowledged.clone(), acknowledged] {
            let next = admiralty_core::reduce(&projection, &event).expect("valid event");
            let before = index.clone();
            let delta = PendingInboxIndex::prepare_update(&next, &event).expect("precommit delta");
            assert_eq!(index, before, "preparation must not mutate committed index");
            index.apply_prepared(delta);
            projection = next;
            assert_eq!(
                index,
                PendingInboxIndex::from_projection(&projection).expect("replay index")
            );
        }
        assert!(
            admiralty_core::reduce(&projection, &queued).is_err(),
            "current reducer rejects a new-source queue replay"
        );
        // Even if a future reducer admits an exact business replay, derive the
        // update from the final ACK state rather than resurrecting the message.
        index.apply_prepared(
            PendingInboxIndex::prepare_update(&projection, &queued).expect("retained ACK delta"),
        );
        assert!(index.scopes.is_empty(), "ACKed history is not pending");
        assert_eq!(projection.messages.len(), 1, "history is retained");
    }

    #[test]
    fn corrupted_index_references_and_unknown_ack_fail_without_partial_page() {
        let mut projection = Projection::default();
        insert(&mut projection, message("a-valid"));
        let mut foreign = message("z-foreign");
        foreign.project_id = ProjectId::new("other-project").expect("project");
        insert(&mut projection, foreign);
        let original = PendingInboxIndex::from_projection(&projection).expect("index");
        for id in ["z-foreign", "z-missing", "z-acked"] {
            let mut broken = original.clone();
            if id == "z-acked" {
                let row = message(id);
                projection
                    .acknowledgments
                    .insert(row.message_id.clone(), row.context_epoch.clone());
                insert(&mut projection, row);
            }
            broken
                .scopes
                .entry(scope())
                .or_default()
                .insert(MessageId::new(id).expect("ID"));
            assert_eq!(
                super::pending_page(&projection, &broken, GENERATION, &scope(), None, 16),
                Err(InboxError::CorruptProjection)
            );
        }
        projection
            .acknowledgments
            .insert(MessageId::new("unknown").expect("ID"), "epoch-one".into());
        assert_eq!(
            PendingInboxIndex::from_projection(&projection),
            Err(InboxError::CorruptProjection)
        );
    }

    #[test]
    fn real_writer_keeps_index_atomic_across_outbox_rejection_ack_replay_and_reopen() {
        use crate::{Store, outbox::OutboxIntent};
        use admiralty_core::SourceIdentity;
        let directory = tempfile::tempdir().expect("private fixture");
        let database = directory.path().join("indexed.sqlite");
        let source = |sequence: u64| SourceIdentity {
            producer_id: "indexed-test".into(),
            incarnation: "boot-one".into(),
            sequence: sequence.to_string(),
        };
        let event = |id: &str| DomainEvent::MessageQueued {
            message: message(id),
        };
        let observed = || "2026-10-18T00:00:00Z".to_owned();
        let store = Store::open(&database).expect("writer");
        store
            .append(
                SourceIdentity {
                    producer_id: "task-registration".into(),
                    incarnation: "boot-one".into(),
                    sequence: "1".into(),
                },
                task_event(),
                observed(),
            )
            .expect("register real immutable task");
        let intent = OutboxIntent {
            idempotency_key: "one-effect".into(),
            payload: serde_json::json!({"fixture":true}),
            not_before: 0,
        };
        store
            .append_with_intent(source(1), event("one"), observed(), intent.clone())
            .expect("first commit");
        assert!(
            store
                .append_with_intent(source(2), event("rejected"), observed(), intent)
                .is_err()
        );
        let page = store
            .pending_messages(scope(), None, 16)
            .expect("committed pending");
        assert_eq!(
            page.messages,
            vec![message("one")],
            "rejected event never enters index"
        );
        store
            .acknowledge_scoped_message(source(3), scope(), message("one").message_id, observed())
            .expect("ACK");
        assert!(
            store
                .append_with_intent(
                    source(1),
                    event("one"),
                    observed(),
                    OutboxIntent {
                        idempotency_key: "one-effect".into(),
                        payload: serde_json::json!({"fixture":true}),
                        not_before: 0
                    }
                )
                .expect("lost source reply")
                .duplicate
        );
        assert!(
            store.append(source(4), event("one"), observed()).is_err(),
            "new-source queue replay remains rejected"
        );
        assert_eq!(
            store
                .pending_messages(scope(), None, 16)
                .expect("ACK retained")
                .scanned_messages,
            0
        );
        for (sequence, id) in [(5, "pending-a"), (6, "pending-b")] {
            store
                .append(source(sequence), event(id), observed())
                .expect("queue");
        }
        let first = store
            .pending_messages(scope(), None, 1)
            .expect("first page");
        let snapshot = store.rebuild().expect("rebuild and index swap");
        assert!(matches!(
            store.pending_messages(scope(), first.next_cursor.clone(), 1),
            Err(crate::StoreError::Inbox(InboxError::SnapshotRequired))
        ));
        let expected = pending_page(
            &snapshot.projection,
            &snapshot.cursor.recovery_generation,
            &scope(),
            None,
            16,
        )
        .expect("rebuilt reference");
        assert_eq!(
            store
                .pending_messages(scope(), None, 16)
                .expect("indexed query"),
            expected
        );
        drop(store);
        let reopened = Store::open(&database).expect("reopen index");
        assert_eq!(
            reopened
                .pending_messages(scope(), None, 16)
                .expect("replayed query"),
            expected
        );
    }
}
