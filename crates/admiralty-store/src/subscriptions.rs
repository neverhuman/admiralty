//! Bounded durable subscriptions. Only the Store writer accesses this journal.
//! Every operation, including an exact retry, requires a current startup policy
//! and an opaque process-local proof. Subscription labels never grant access.
use crate::{Result, StoreError, inbox::InboxScope, stored_text};
use admiralty_core::{
    CoordinationMessage, MessageId, MessagePayload, PrincipalId, ProjectId, TaskId, TaskView,
    content_digest,
};
use rusqlite::{Connection, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use std::{any::Any, sync::Arc};

pub const MAX_SUBSCRIPTIONS: i64 = 4096;
pub const MAX_TOPIC_SUBSCRIPTIONS: i64 = 128;
const MAX_SUBSCRIPTION_BYTES: usize = 4096;
pub type SubscriptionProof = Arc<dyn Any + Send + Sync>;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Subscription {
    pub subscription_id: MessageId,
    pub project_id: ProjectId,
    pub task_id: TaskId,
    pub recipient: PrincipalId,
    pub context_epoch: String,
    pub channel: String,
    pub contract_revision: u32,
    pub contract_digest: String,
    /// Recorded UTC seconds. Expiry rejects routing visibly; it never ACKs or
    /// removes an already queued message. The policy authenticates clock input.
    pub expires_at_utc: Option<u64>,
}

impl Subscription {
    pub fn inbox_scope(&self) -> InboxScope {
        InboxScope {
            project_id: self.project_id.clone(),
            recipient: self.recipient.clone(),
            context_epoch: self.context_epoch.clone(),
        }
    }
}

pub enum SubscriptionAction<'a> {
    Subscribe,
    Read,
    Route {
        source: &'a admiralty_core::SourceIdentity,
        message: &'a CoordinationMessage,
    },
}

/// Installed independently at startup. Must authenticate the opaque proof and
/// exact actor/project/task/channel/recipient/source, current grants/revocation
/// and recorded clock. No I/O or reentrant Store calls on the writer thread.
pub trait SubscriptionAuthority: Send + Sync {
    fn authorize(
        &self,
        subscription: &Subscription,
        action: SubscriptionAction<'_>,
        observed_utc: u64,
        current_task: &TaskView,
        proof: &(dyn Any + Send + Sync),
    ) -> bool;
}

fn invalid(message: &str) -> StoreError {
    StoreError::Invalid(message.into())
}

pub(crate) fn validate(subscription: &Subscription) -> Result<()> {
    admiralty_core::limits::bounded_json_size(subscription, MAX_SUBSCRIPTION_BYTES)
        .map_err(|_| invalid("subscription exceeds its serialized bound"))?;
    let digest = &subscription.contract_digest;
    if subscription.contract_revision == 0
        || !digest.strip_prefix("sha256:").is_some_and(|hex| {
            hex.len() == 64
                && hex
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
        || [&subscription.channel, &subscription.context_epoch]
            .iter()
            .any(|s| {
                s.is_empty()
                    || s.len() > 256
                    || s.trim() != s.as_str()
                    || s.chars().any(char::is_control)
            })
        || subscription.expires_at_utc == Some(0)
    {
        return Err(invalid("subscription scope, contract or expiry is invalid"));
    }
    Ok(())
}

pub(crate) fn initialize(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS coordination_subscriptions (
             subscription_id TEXT PRIMARY KEY,
             project_id TEXT NOT NULL,
             task_id TEXT NOT NULL,
             channel TEXT NOT NULL,
             definition_json TEXT NOT NULL,
             definition_digest TEXT NOT NULL
         );
         CREATE INDEX IF NOT EXISTS coordination_subscription_topic
         ON coordination_subscriptions(project_id,task_id,channel);",
    )?;
    Ok(())
}

pub(crate) fn authorize(
    subscription: &Subscription,
    action: SubscriptionAction<'_>,
    observed_utc: u64,
    task: Option<&TaskView>,
    authority: Option<&dyn SubscriptionAuthority>,
    proof: &(dyn Any + Send + Sync),
) -> Result<()> {
    validate(subscription)?;
    let task = task.ok_or_else(|| invalid("subscription task is unavailable"))?;
    if task.contract.project_id != subscription.project_id
        || task.contract.revision != subscription.contract_revision
        || content_digest(&task.contract).map_err(invalid_digest)? != subscription.contract_digest
    {
        return Err(invalid("subscription contract is stale or cross-project"));
    }
    if observed_utc == 0
        || subscription
            .expires_at_utc
            .is_some_and(|expiry| observed_utc >= expiry)
        || !authority
            .is_some_and(|policy| policy.authorize(subscription, action, observed_utc, task, proof))
    {
        return Err(invalid("subscription operation is expired or unauthorized"));
    }
    Ok(())
}

fn invalid_digest(error: impl std::fmt::Display) -> StoreError {
    invalid(&error.to_string())
}

fn checked_definition(row: &rusqlite::Row<'_>) -> Result<Subscription> {
    let json = stored_text(row, 0, MAX_SUBSCRIPTION_BYTES)?;
    let digest = stored_text(row, 1, 71)?;
    let original: Subscription = serde_json::from_str(&json)?;
    validate(&original)?;
    if content_digest(&original).map_err(invalid_digest)? != digest
        || stored_text(row, 2, 128)? != original.subscription_id.as_str()
        || stored_text(row, 3, 128)? != original.project_id.as_str()
        || stored_text(row, 4, 128)? != original.task_id.as_str()
        || stored_text(row, 5, 256)? != original.channel
    {
        return Err(invalid(
            "subscription journal digest or indexed scope is corrupt",
        ));
    }
    Ok(original)
}

pub(crate) fn read(connection: &Connection, subscription: &Subscription) -> Result<bool> {
    let mut statement = connection.prepare(
        "SELECT definition_json,definition_digest,subscription_id,project_id,task_id,channel
         FROM coordination_subscriptions WHERE subscription_id=?1",
    )?;
    let mut rows = statement.query([subscription.subscription_id.as_str()])?;
    let Some(row) = rows.next()? else {
        return Ok(false);
    };
    if &checked_definition(row)? != subscription {
        return Err(invalid("subscription identity collision"));
    }
    Ok(true)
}

pub(crate) fn validate_all(connection: &Connection) -> Result<()> {
    let mut statement = connection.prepare(
        "SELECT definition_json,definition_digest,subscription_id,project_id,task_id,channel
         FROM coordination_subscriptions ORDER BY subscription_id LIMIT 4097",
    )?;
    let mut rows = statement.query([])?;
    let mut count = 0;
    let mut topics = std::collections::BTreeMap::new();
    while let Some(row) = rows.next()? {
        count += 1;
        let definition = checked_definition(row)?;
        let members = topics
            .entry((
                definition.project_id,
                definition.task_id,
                definition.channel,
            ))
            .or_insert(0);
        *members += 1;
        if count > MAX_SUBSCRIPTIONS || *members > MAX_TOPIC_SUBSCRIPTIONS {
            return Err(invalid("subscription journal exceeds membership bounds"));
        }
    }
    Ok(())
}

pub(crate) fn subscribe(connection: &mut Connection, subscription: &Subscription) -> Result<bool> {
    let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if read(&tx, subscription)? {
        return Ok(false);
    }
    let total: i64 = tx.query_row(
        "SELECT count(*) FROM coordination_subscriptions",
        [],
        |row| row.get(0),
    )?;
    let topic: i64 = tx.query_row(
        "SELECT count(*) FROM coordination_subscriptions WHERE project_id=?1 AND task_id=?2 AND channel=?3",
        params![subscription.project_id.as_str(), subscription.task_id.as_str(), subscription.channel],
        |row| row.get(0),
    )?;
    if total >= MAX_SUBSCRIPTIONS || topic >= MAX_TOPIC_SUBSCRIPTIONS {
        return Err(invalid(
            "subscription capacity reached; no membership was evicted",
        ));
    }
    tx.execute(
        "INSERT INTO coordination_subscriptions VALUES(?1,?2,?3,?4,?5,?6)",
        params![
            subscription.subscription_id.as_str(),
            subscription.project_id.as_str(),
            subscription.task_id.as_str(),
            subscription.channel,
            serde_json::to_string(subscription)?,
            content_digest(subscription).map_err(invalid_digest)?
        ],
    )?;
    tx.commit()?;
    Ok(true)
}

pub(crate) fn validate_route(
    subscription: &Subscription,
    message: &CoordinationMessage,
    projection: &admiralty_core::Projection,
) -> Result<()> {
    message.validate().map_err(|error| invalid(&error))?;
    let task_id = match &message.payload {
        MessagePayload::TaskAssigned { task_id, .. }
        | MessagePayload::Question { task_id, .. }
        | MessagePayload::ArtifactReady { task_id, .. }
        | MessagePayload::Handoff { task_id, .. } => Some(task_id),
        MessagePayload::Answer { question_id, .. } => {
            projection.messages.get(question_id).and_then(|question| {
                if let MessagePayload::Question { task_id, .. } = &question.payload {
                    Some(task_id)
                } else {
                    None
                }
            })
        }
    };
    if message.project_id != subscription.project_id
        || message.recipient != subscription.recipient
        || message.context_epoch != subscription.context_epoch
        || task_id != Some(&subscription.task_id)
        || !projection
            .tasks
            .get(&subscription.task_id)
            .is_some_and(|task| message.authority_ref == task.contract.authority_ref)
    {
        return Err(invalid(
            "message does not match the exact subscription scope",
        ));
    }
    Ok(())
}
