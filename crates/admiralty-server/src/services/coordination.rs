//! Durable coordination service over Store and bounded project-scoped mailboxes.
use admiralty_core::{
    CoordinationMessage, DomainEvent, MessageId, PrincipalId, ProjectId, SourceIdentity,
    coordination::mailbox::{BoundedMailbox, DeliveryOutcome},
};
use admiralty_store::{AppendReceipt, Store, StoreError};

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

    pub fn project_messages(
        &self,
        project_id: &ProjectId,
    ) -> Result<Vec<CoordinationMessage>, StoreError> {
        let snap = self.store.snapshot()?;
        Ok(snap
            .projection
            .messages
            .values()
            .filter(|m| &m.project_id == project_id)
            .cloned()
            .collect())
    }
}
