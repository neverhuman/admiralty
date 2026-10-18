//! Bounded in-memory recipient delivery observations, not a durable mailbox.
//!
//! Retained immutable messages include local ACK state and collision evidence.
//! ACK never frees that evidence. History exhaustion requires a separately
//! authorized resnapshot; this component does not invent a reset, persistence,
//! actor authorization, delivery journal, expiry or task acceptance transition.
use super::message::{MessageValidationError, validate_scoped_envelope};
use crate::{CoordinationMessage, MessageId, PrincipalId, ProjectId};
use std::collections::BTreeMap;

pub const MAX_MAILBOX_ENTRIES: usize = 2000;
pub const MAX_DELIVERY_BATCH: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum MailboxError {
    #[error("mailbox capacity {capacity} is outside 1..={maximum}")]
    InvalidCapacity { capacity: usize, maximum: usize },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeliveryOutcome {
    Enqueued {
        seq: u64,
    },
    Duplicate {
        seq: u64,
    },
    BufferedOutOfOrder {
        seq: u64,
        contiguous_seq: u64,
    },
    /// Incoming message was not admitted; retained history is never discarded.
    RequiresResnapshot {
        capacity: usize,
        dropped_seq: u64,
    },
    CrossProjectRejected,
    InvalidMessage {
        error: MessageValidationError,
    },
    InvalidSequence {
        seq: u64,
    },
    MessageIdCollision {
        message_id: MessageId,
        existing_seq: u64,
        received_seq: u64,
    },
    SequenceCollision {
        seq: u64,
    },
    SequenceExhausted,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RetainedMessage {
    message: CoordinationMessage,
    acked: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoundedMailbox {
    project_id: ProjectId,
    owner: PrincipalId,
    capacity: usize,
    contiguous_seq: u64,
    resnapshot_required: bool,
    by_id: BTreeMap<MessageId, u64>,
    entries: BTreeMap<u64, RetainedMessage>,
}

impl BoundedMailbox {
    pub fn new(
        project_id: ProjectId,
        owner: PrincipalId,
        capacity: usize,
    ) -> Result<Self, MailboxError> {
        if !(1..=MAX_MAILBOX_ENTRIES).contains(&capacity) {
            return Err(MailboxError::InvalidCapacity {
                capacity,
                maximum: MAX_MAILBOX_ENTRIES,
            });
        }
        Ok(Self {
            project_id,
            owner,
            capacity,
            contiguous_seq: 0,
            resnapshot_required: false,
            by_id: BTreeMap::new(),
            entries: BTreeMap::new(),
        })
    }

    pub fn project_id(&self) -> &ProjectId {
        &self.project_id
    }
    pub fn owner(&self) -> &PrincipalId {
        &self.owner
    }
    pub fn capacity(&self) -> usize {
        self.capacity
    }
    pub fn contiguous_seq(&self) -> u64 {
        self.contiguous_seq
    }
    pub fn resnapshot_required(&self) -> bool {
        self.resnapshot_required
    }
    pub fn retained_len(&self) -> usize {
        self.entries.len()
    }

    pub fn route_sequenced(&mut self, seq: u64, message: CoordinationMessage) -> DeliveryOutcome {
        if seq == 0 {
            return DeliveryOutcome::InvalidSequence { seq };
        }
        if let Err(error) = validate_scoped_envelope(&message, &self.project_id, Some(&self.owner))
        {
            return match error {
                MessageValidationError::CrossProject { .. }
                | MessageValidationError::WrongRecipient { .. } => {
                    DeliveryOutcome::CrossProjectRejected
                }
                error => DeliveryOutcome::InvalidMessage { error },
            };
        }
        if let Some(existing_seq) = self.by_id.get(&message.message_id) {
            if *existing_seq == seq
                && self
                    .entries
                    .get(existing_seq)
                    .is_some_and(|entry| entry.message == message)
            {
                return DeliveryOutcome::Duplicate { seq: *existing_seq };
            }
            return DeliveryOutcome::MessageIdCollision {
                message_id: message.message_id.clone(),
                existing_seq: *existing_seq,
                received_seq: seq,
            };
        }
        if self.entries.contains_key(&seq) {
            return DeliveryOutcome::SequenceCollision { seq };
        }
        if self.contiguous_seq == u64::MAX {
            return DeliveryOutcome::SequenceExhausted;
        }
        if seq <= self.contiguous_seq {
            return DeliveryOutcome::InvalidSequence { seq };
        }
        if self.resnapshot_required || self.entries.len() >= self.capacity {
            self.resnapshot_required = true;
            return DeliveryOutcome::RequiresResnapshot {
                capacity: self.capacity,
                dropped_seq: seq,
            };
        }
        // Calculate the resulting prefix before mutation. MAX is a valid final
        // sequence: checked_add(None) closes the prefix without wrapping to zero.
        let mut contiguous = self.contiguous_seq;
        while let Some(next) = contiguous.checked_add(1) {
            if next == seq || self.entries.contains_key(&next) {
                contiguous = next;
            } else {
                break;
            }
        }
        self.by_id.insert(message.message_id.clone(), seq);
        self.entries.insert(
            seq,
            RetainedMessage {
                message,
                acked: false,
            },
        );
        self.contiguous_seq = contiguous;
        if seq <= contiguous {
            DeliveryOutcome::Enqueued { seq }
        } else {
            DeliveryOutcome::BufferedOutOfOrder {
                seq,
                contiguous_seq: contiguous,
            }
        }
    }

    /// Bounded local delivery view. This is not a durable transport/provider ACK
    /// and does not assert that a recipient understood or accepted the contents.
    pub fn deliver_contiguous_batch(&self, limit: usize) -> Vec<(u64, CoordinationMessage)> {
        self.entries
            .iter()
            .filter(|(seq, entry)| **seq <= self.contiguous_seq && !entry.acked)
            .take(limit.min(MAX_DELIVERY_BATCH))
            .map(|(seq, entry)| (*seq, entry.message.clone()))
            .collect()
    }

    /// Idempotent in-memory acknowledgment. Exact replay remains recognizable;
    /// no collision tombstone or retained message is evicted to make room.
    pub fn ack_delivery(&mut self, seq: u64) -> bool {
        if seq == 0 || seq > self.contiguous_seq {
            return false;
        }
        if let Some(entry) = self.entries.get_mut(&seq)
            && !entry.acked
        {
            entry.acked = true;
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MessagePayload, TaskId};

    fn message(id: &str) -> CoordinationMessage {
        CoordinationMessage {
            schema_version: 1,
            message_id: MessageId::new(id).expect("fixture message identity"),
            project_id: ProjectId::new("project-1").expect("fixture project"),
            sender: PrincipalId::new("captain").expect("fixture sender"),
            recipient: PrincipalId::new("worker").expect("fixture recipient"),
            causation_id: None,
            context_epoch: "epoch-1".into(),
            authority_ref: "grant:task-1".into(),
            payload: MessagePayload::Handoff {
                task_id: TaskId::new("task-1").expect("fixture task"),
                candidate_ref: "candidate:one".into(),
                failed_check: None,
                next_action: "Do NOT publish; verify timeout <= 5 ms".into(),
            },
        }
    }
    fn mailbox(capacity: usize) -> BoundedMailbox {
        let message = message("first");
        BoundedMailbox::new(message.project_id, message.recipient, capacity)
            .expect("bounded fixture mailbox")
    }

    #[test]
    fn constructor_bounds_and_read_only_scope_are_explicit() {
        for capacity in [0, MAX_MAILBOX_ENTRIES + 1, usize::MAX] {
            let message = message("first");
            assert!(matches!(
                BoundedMailbox::new(message.project_id, message.recipient, capacity),
                Err(MailboxError::InvalidCapacity { .. })
            ));
        }
        let instance = mailbox(MAX_MAILBOX_ENTRIES);
        assert_eq!(instance.capacity(), MAX_MAILBOX_ENTRIES);
        assert_eq!(instance.project_id().as_str(), "project-1");
        assert_eq!(instance.owner().as_str(), "worker");
        assert_eq!(instance.contiguous_seq(), 0);
        assert_eq!(instance.retained_len(), 0);
        assert!(!instance.resnapshot_required());
    }

    #[test]
    fn same_id_payload_epoch_authority_and_causation_changes_are_collisions_without_mutation() {
        let mut instance = mailbox(10);
        let original = message("one");
        assert_eq!(
            instance.route_sequenced(1, original.clone()),
            DeliveryOutcome::Enqueued { seq: 1 }
        );
        let before = instance.clone();
        let mut variants = Vec::new();
        let mut changed = original.clone();
        if let MessagePayload::Handoff { next_action, .. } = &mut changed.payload {
            *next_action = "Publish; timeout <= 5 seconds".into();
        }
        variants.push(changed);
        changed = original.clone();
        changed.context_epoch = "epoch-2".into();
        variants.push(changed);
        changed = original.clone();
        changed.authority_ref = "grant:other".into();
        variants.push(changed);
        changed = original.clone();
        changed.causation_id = Some(MessageId::new("cause-other").expect("fixture causation"));
        variants.push(changed);
        changed = original;
        changed.sender = PrincipalId::new("foreign-sender").expect("fixture sender");
        variants.push(changed);
        for changed in variants {
            assert!(matches!(
                instance.route_sequenced(1, changed),
                DeliveryOutcome::MessageIdCollision {
                    existing_seq: 1,
                    received_seq: 1,
                    ..
                }
            ));
            assert_eq!(instance, before);
        }
    }

    #[test]
    fn message_id_sequence_and_sequence_message_id_bindings_are_immutable() {
        let mut instance = mailbox(3);
        let original = message("one");
        instance.route_sequenced(1, original.clone());
        let before = instance.clone();
        assert!(matches!(
            instance.route_sequenced(2, original.clone()),
            DeliveryOutcome::MessageIdCollision {
                existing_seq: 1,
                received_seq: 2,
                ..
            }
        ));
        assert_eq!(instance, before);
        assert_eq!(
            instance.route_sequenced(1, message("other")),
            DeliveryOutcome::SequenceCollision { seq: 1 }
        );
        assert_eq!(instance, before);
        assert_eq!(
            instance.route_sequenced(1, original),
            DeliveryOutcome::Duplicate { seq: 1 }
        );
        assert_eq!(instance, before);
    }

    #[test]
    fn exact_replay_after_ack_and_resnapshot_preserves_collision_evidence() {
        let mut instance = mailbox(1);
        let original = message("one");
        instance.route_sequenced(1, original.clone());
        assert!(instance.ack_delivery(1));
        assert!(!instance.ack_delivery(1));
        assert!(instance.deliver_contiguous_batch(10).is_empty());
        assert_eq!(
            instance.route_sequenced(2, message("two")),
            DeliveryOutcome::RequiresResnapshot {
                capacity: 1,
                dropped_seq: 2
            }
        );
        let before = instance.clone();
        assert_eq!(
            instance.route_sequenced(1, original.clone()),
            DeliveryOutcome::Duplicate { seq: 1 }
        );
        assert_eq!(instance, before);
        assert!(matches!(
            instance.route_sequenced(2, original),
            DeliveryOutcome::MessageIdCollision { .. }
        ));
        assert_eq!(instance, before);
    }

    #[test]
    fn long_capacity_one_ack_cycles_cannot_grow_retained_history_or_clear_backpressure() {
        let mut instance = mailbox(1);
        instance.route_sequenced(1, message("first"));
        assert!(instance.ack_delivery(1));
        for seq in 2..=10_000 {
            assert_eq!(
                instance.route_sequenced(seq, message(&format!("message-{seq}"))),
                DeliveryOutcome::RequiresResnapshot {
                    capacity: 1,
                    dropped_seq: seq
                }
            );
            assert!(!instance.ack_delivery(seq));
            assert!(instance.resnapshot_required());
            assert_eq!(instance.retained_len(), 1);
            assert_eq!(instance.by_id.len(), 1);
            assert_eq!(instance.contiguous_seq(), 1);
        }
    }

    #[test]
    fn reorder_prefix_local_ack_and_delivery_page_limits_are_deterministic() {
        let mut instance = mailbox(MAX_DELIVERY_BATCH + 2);
        assert_eq!(
            instance.route_sequenced(2, message("two")),
            DeliveryOutcome::BufferedOutOfOrder {
                seq: 2,
                contiguous_seq: 0
            }
        );
        assert!(instance.deliver_contiguous_batch(usize::MAX).is_empty());
        assert!(!instance.ack_delivery(2));
        assert_eq!(
            instance.route_sequenced(1, message("one")),
            DeliveryOutcome::Enqueued { seq: 1 }
        );
        for seq in 3..=MAX_DELIVERY_BATCH as u64 + 2 {
            instance.route_sequenced(seq, message(&format!("message-{seq}")));
        }
        assert!(instance.deliver_contiguous_batch(0).is_empty());
        assert_eq!(
            instance.deliver_contiguous_batch(usize::MAX).len(),
            MAX_DELIVERY_BATCH
        );
        assert!(instance.ack_delivery(1));
        assert_eq!(instance.deliver_contiguous_batch(1)[0].0, 2);
        assert_eq!(instance.retained_len(), MAX_DELIVERY_BATCH + 2);
    }

    #[test]
    fn zero_max_and_exhausted_watermarks_never_wrap_or_mutate_on_error() {
        let mut instance = mailbox(3);
        let before = instance.clone();
        assert_eq!(
            instance.route_sequenced(0, message("zero")),
            DeliveryOutcome::InvalidSequence { seq: 0 }
        );
        assert_eq!(instance, before);
        assert!(!instance.ack_delivery(0));
        assert_eq!(
            instance.route_sequenced(u64::MAX, message("far-future")),
            DeliveryOutcome::BufferedOutOfOrder {
                seq: u64::MAX,
                contiguous_seq: 0
            }
        );
        // Exercise the arithmetic boundary without allocating a historical u64
        // number of messages. Production watermarks have no mutation API.
        let mut terminal = mailbox(3);
        terminal.contiguous_seq = u64::MAX - 1;
        let last = message("last");
        assert_eq!(
            terminal.route_sequenced(u64::MAX, last.clone()),
            DeliveryOutcome::Enqueued { seq: u64::MAX }
        );
        assert_eq!(terminal.contiguous_seq(), u64::MAX);
        assert!(terminal.ack_delivery(u64::MAX));
        let before = terminal.clone();
        assert_eq!(
            terminal.route_sequenced(u64::MAX, last),
            DeliveryOutcome::Duplicate { seq: u64::MAX }
        );
        assert_eq!(terminal, before);
        assert_eq!(
            terminal.route_sequenced(1, message("wrapped")),
            DeliveryOutcome::SequenceExhausted
        );
        assert_eq!(terminal, before);
        let mut old = mailbox(3);
        old.contiguous_seq = 10;
        let before = old.clone();
        assert_eq!(
            old.route_sequenced(9, message("unknown-old")),
            DeliveryOutcome::InvalidSequence { seq: 9 }
        );
        assert_eq!(old, before);
    }

    #[test]
    fn invalid_schema_epoch_authority_payload_and_size_leave_no_retained_trace() {
        let mut variants = Vec::new();
        let mut invalid = message("invalid");
        invalid.schema_version = 2;
        variants.push(invalid);
        invalid = message("invalid");
        invalid.context_epoch = " \n".into();
        variants.push(invalid);
        invalid = message("invalid");
        invalid.authority_ref = " \n".into();
        variants.push(invalid);
        invalid = message("invalid");
        invalid.authority_ref = "a".repeat(16_384);
        variants.push(invalid);
        invalid = message("invalid");
        invalid.context_epoch = "e".repeat(16_384);
        variants.push(invalid);
        invalid = message("invalid");
        if let MessagePayload::Handoff { next_action, .. } = &mut invalid.payload {
            *next_action = "\"".repeat(8192);
        }
        variants.push(invalid);
        invalid = message("invalid");
        if let MessagePayload::Handoff { candidate_ref, .. } = &mut invalid.payload {
            candidate_ref.clear();
        }
        variants.push(invalid);
        for invalid in variants {
            let mut instance = mailbox(1);
            let before = instance.clone();
            assert!(matches!(
                instance.route_sequenced(1, invalid),
                DeliveryOutcome::InvalidMessage { .. }
            ));
            assert_eq!(instance, before);
        }
        for field in ["project", "recipient"] {
            let mut invalid = message("foreign");
            if field == "project" {
                invalid.project_id = ProjectId::new("foreign").expect("fixture project");
            } else {
                invalid.recipient = PrincipalId::new("foreign").expect("fixture recipient");
            }
            let mut instance = mailbox(1);
            let before = instance.clone();
            assert_eq!(
                instance.route_sequenced(1, invalid),
                DeliveryOutcome::CrossProjectRejected
            );
            assert_eq!(instance, before);
        }
    }
}
