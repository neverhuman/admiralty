//! Bounded recipient mailbox with duplicate suppression, reorder buffering, storm coalescing, and explicit resnapshot signaling.
use crate::{CoordinationMessage, MessageId, PrincipalId, ProjectId};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeliveryOutcome {
    Enqueued { seq: u64 },
    Duplicate { seq: u64 },
    BufferedOutOfOrder { seq: u64, contiguous_seq: u64 },
    RequiresResnapshot { capacity: usize, dropped_seq: u64 },
    CrossProjectRejected,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoundedMailbox {
    pub project_id: ProjectId,
    pub owner: PrincipalId,
    pub capacity: usize,
    pub contiguous_seq: u64,
    pub resnapshot_required: bool,
    seen_ids: BTreeSet<MessageId>,
    entries: BTreeMap<u64, CoordinationMessage>,
    acked: BTreeSet<u64>,
}

impl BoundedMailbox {
    pub fn new(project_id: ProjectId, owner: PrincipalId, capacity: usize) -> Self {
        Self {
            project_id,
            owner,
            capacity: capacity.max(1),
            contiguous_seq: 0,
            resnapshot_required: false,
            seen_ids: BTreeSet::new(),
            entries: BTreeMap::new(),
            acked: BTreeSet::new(),
        }
    }

    pub fn route_sequenced(&mut self, seq: u64, message: CoordinationMessage) -> DeliveryOutcome {
        if message.project_id != self.project_id || message.recipient != self.owner {
            return DeliveryOutcome::CrossProjectRejected;
        }
        if self.seen_ids.contains(&message.message_id) || self.entries.contains_key(&seq) {
            return DeliveryOutcome::Duplicate { seq };
        }
        let unacked = self.entries.len().saturating_sub(self.acked.len());
        if unacked >= self.capacity {
            self.resnapshot_required = true;
            return DeliveryOutcome::RequiresResnapshot {
                capacity: self.capacity,
                dropped_seq: seq,
            };
        }
        self.seen_ids.insert(message.message_id.clone());
        self.entries.insert(seq, message);
        while self.entries.contains_key(&(self.contiguous_seq + 1)) {
            self.contiguous_seq += 1;
        }
        if seq == self.contiguous_seq {
            DeliveryOutcome::Enqueued { seq }
        } else {
            DeliveryOutcome::BufferedOutOfOrder {
                seq,
                contiguous_seq: self.contiguous_seq,
            }
        }
    }

    pub fn deliver_contiguous_batch(&self, limit: usize) -> Vec<(u64, CoordinationMessage)> {
        self.entries
            .iter()
            .filter(|(seq, _)| **seq <= self.contiguous_seq && !self.acked.contains(seq))
            .take(limit)
            .map(|(seq, msg)| (*seq, msg.clone()))
            .collect()
    }

    pub fn ack_delivery(&mut self, seq: u64) -> bool {
        if seq <= self.contiguous_seq && self.entries.contains_key(&seq) {
            self.acked.insert(seq)
        } else {
            false
        }
    }
}
