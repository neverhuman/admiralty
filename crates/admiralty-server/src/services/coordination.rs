//! Store append wrappers and bounded, authorized queued-message observation replay.
//!
//! This prototype does not authenticate callers, run a delivery loop, or confer
//! authority. A trusted service caller must supply its actual read authorizer.
use admiralty_core::{
    CoordinationMessage, DomainEvent, MessageId, PrincipalId, ProjectId, SourceIdentity,
    coordination::mailbox::{BoundedMailbox, DeliveryOutcome},
};
use admiralty_store::{AppendReceipt, Cursor, Store, StoreError};

/// Bound both underlying durable events examined and returned message allocation.
pub const MAX_COORDINATION_SCAN_EVENTS: usize = 16;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoordinationReadScope {
    pub project_id: ProjectId,
    pub recipient: PrincipalId,
    pub context_epoch: String,
}

/// Policy is evaluated on every page before the Store is queried. Implementors
/// must validate actual actor grants and epoch/scope, not merely compare labels.
pub trait CoordinationReadAuthorizer {
    fn authorize(&self, actor: &PrincipalId, scope: &CoordinationReadScope) -> bool;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueuedMessageBatch {
    /// Advances over the bounded scanned durable page, even if none match.
    pub cursor: Cursor,
    pub messages: Vec<CoordinationMessage>,
    pub scanned_events: usize,
}

pub struct CoordinationService<'a> {
    store: &'a Store,
}

impl<'a> CoordinationService<'a> {
    pub fn new(store: &'a Store) -> Self {
        Self { store }
    }

    pub fn enqueue_durable(
        &self,
        source: SourceIdentity,
        message: CoordinationMessage,
        observed_utc: String,
    ) -> Result<AppendReceipt, StoreError> {
        self.store
            .append(source, DomainEvent::MessageQueued { message }, observed_utc)
    }

    pub fn acknowledge_durable(
        &self,
        source: SourceIdentity,
        message_id: MessageId,
        recipient: PrincipalId,
        context_epoch: String,
        observed_utc: String,
    ) -> Result<AppendReceipt, StoreError> {
        self.store.append(
            source,
            DomainEvent::MessageAcknowledged {
                message_id,
                recipient,
                context_epoch,
            },
            observed_utc,
        )
    }

    pub fn route_to_mailbox(
        mailbox: &mut BoundedMailbox,
        seq: u64,
        message: CoordinationMessage,
    ) -> DeliveryOutcome {
        mailbox.route_sequenced(seq, message)
    }

    /// Replay matching queue observations without cloning the global projection.
    /// An ACKed message can still appear here: this is not a pending-delivery
    /// list, application ACK, delivery attempt or subscription activation.
    /// Recovery cursor validation remains atomic inside the Store writer.
    pub fn queued_messages_since(
        &self,
        actor: &PrincipalId,
        authorizer: &impl CoordinationReadAuthorizer,
        scope: &CoordinationReadScope,
        cursor: &Cursor,
        scan_limit: usize,
    ) -> Result<QueuedMessageBatch, StoreError> {
        if scan_limit == 0
            || scan_limit > MAX_COORDINATION_SCAN_EVENTS
            || scope.context_epoch.len() > 16_384
            || scope.context_epoch.trim().is_empty()
            || cursor.recovery_generation.is_empty()
            || cursor.recovery_generation.len() > 64
            || cursor.event_seq.len() > 19
            || !cursor
                .event_seq
                .parse::<i64>()
                .is_ok_and(|seq| seq >= 0 && seq.to_string() == cursor.event_seq)
        {
            return Err(StoreError::Invalid(
                "coordination page requires bounded scope, cursor and scan limit".into(),
            ));
        }
        if !authorizer.authorize(actor, scope) {
            return Err(StoreError::Invalid(
                "coordination read is not authorized for this actor and scope".into(),
            ));
        }
        let batch = self.store.events_since(cursor, scan_limit)?;
        let scanned_events = batch.events.len();
        let messages = batch
            .events
            .into_iter()
            .filter_map(|record| match record.event {
                DomainEvent::MessageQueued { message }
                    if message.project_id == scope.project_id
                        && message.recipient == scope.recipient
                        && message.context_epoch == scope.context_epoch =>
                {
                    Some(message)
                }
                _ => None,
            })
            .collect();
        Ok(QueuedMessageBatch {
            cursor: batch.cursor,
            messages,
            scanned_events,
        })
    }
}
