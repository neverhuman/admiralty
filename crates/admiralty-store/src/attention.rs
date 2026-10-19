//! Durable, bounded attention-policy replay on Store's sole writer connection.
//!
//! Both observations AND delivery requests are immutable journal commands: the
//! latter consume the pure governor's clock/budget too. Wake proposals and new
//! outbox intents commit in one transaction. Recording a wake is neither an
//! authenticated grant nor transport, provider, or application delivery.
//! Source authority/evidence references are consistency data. The trusted caller
//! supplies the independent project/root/recipient scope; service auth is external.
//! No history/configuration/epoch is replaced or evicted to reset a budget.
//! Unkeyed integrity digests do not authenticate a database owner who rewrites all
//! evidence. Scope/authentication, transport reconciliation and safe retirement
//! remain external. The JSON byte cap is not a SQLite/WAL/disk-growth guarantee.
use crate::outbox::{self, EnqueueResult, OutboxIntent, PreparedIntent};
use admiralty_core::{
    ProjectId, RootId, content_digest,
    governor::{
        AttentionGovernor, AttentionUpdate, DeliveryBatch, DeliveryDecision, GovernorConfig,
        GovernorError, Observation, Recipient,
    },
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

pub const MAX_JOURNALS: usize = 64;
pub const MAX_COMMANDS: usize = 4096;
/// Aggregate accepted command + immutable receipt JSON bytes across all journals.
pub const MAX_HISTORY_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_DEFINITION_BYTES: usize = 8192;
pub const MAX_COMMAND_BYTES: usize = 32768;
pub const MAX_RECEIPT_BYTES: usize = 32768;
pub const MAX_CALLER_PAYLOAD_BYTES: usize = 4096;
type Result<T> = std::result::Result<T, AttentionError>;

#[derive(Debug, thiserror::Error)]
pub enum AttentionError {
    #[error("attention SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("attention JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Governor(#[from] GovernorError),
    #[error(transparent)]
    Outbox(#[from] outbox::OutboxError),
    #[error("invalid or oversized attention request")]
    Invalid,
    #[error("attention identity has conflicting immutable content")]
    Collision,
    #[error("attention prior-state compare-and-swap is stale")]
    StaleCursor,
    #[error("attention journal is not registered in the exact supplied scope")]
    MissingJournal,
    #[error("attention history capacity is full; nothing was evicted or reset")]
    Capacity,
    #[error("stored attention history is inconsistent")]
    Integrity,
}

/// One project/root and an explicitly declared set of independently scoped
/// recipients. Core updates carry no root: their root binding comes ONLY from
/// this caller-owned namespace, never from a source's authority label.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AttentionScope {
    pub journal_id: String,
    pub project_id: ProjectId,
    pub root_id: RootId,
    pub epoch: String,
    /// Sorted unique (role, principal) pairs, at most 16.
    pub recipients: Vec<Recipient>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AttentionDefinition {
    pub scope: AttentionScope,
    pub config: GovernorConfig,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AttentionRegistration {
    pub definition_digest: String,
    pub config_digest: String,
    pub replayed: bool,
    pub authority_granted: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AttentionCursor {
    pub revision: String,
    /// Digest of the full deterministic command/result prefix, not an auth MAC.
    pub state_digest: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AttentionOperation {
    Observe {
        update: Box<AttentionUpdate>,
    },
    /// `intent.payload` is bounded caller metadata. On a Wake it is wrapped with
    /// the actual scope, command ID and governor batch. The resulting complete
    /// envelope must still fit the existing 16 KiB outbox limit, without truncation.
    TakeDelivery {
        recipient: Recipient,
        intent: OutboxIntent,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AttentionCommand {
    pub scope: AttentionScope,
    pub config_digest: String,
    pub command_id: String,
    pub expected_prior: Option<AttentionCursor>,
    /// Trusted recorded logical policy tick; None is rejected, never zero-filled.
    pub logical_tick: Option<u64>,
    pub operation: AttentionOperation,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AttentionOutcome {
    Observation { observation: Observation },
    Delivery { decision: DeliveryDecision },
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AttentionEffect {
    pub outbox_id: String,
    pub idempotency_key: String,
    pub payload_digest: String,
    pub intent_digest: String,
    pub not_before: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AttentionReceipt {
    pub cursor: AttentionCursor,
    pub logical_tick: String,
    pub outcome: AttentionOutcome,
    pub effect: Option<AttentionEffect>,
    pub recorded_wake: bool,
    pub delivered: bool,
    pub application_acknowledged: bool,
    pub authority_granted: bool,
}
/// Replay changes this outer flag only. Original receipt bytes, digest, logical
/// tick, batch, cursor and outbox identity remain immutable historical evidence.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AttentionResult {
    pub receipt: AttentionReceipt,
    pub receipt_digest: String,
    pub replayed: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AttentionStatus {
    pub scope: AttentionScope,
    pub config_digest: String,
    pub cursor: Option<AttentionCursor>,
    pub pending_refs: usize,
    pub topics: usize,
    pub authority_granted: bool,
}

fn id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b))
}
fn digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|v| {
        v.len() == 64
            && v.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}
fn json<T: Serialize>(value: &T, max: usize) -> Result<String> {
    admiralty_core::limits::bounded_json_size(value, max).map_err(|_| AttentionError::Invalid)?;
    Ok(serde_json::to_string(value)?)
}
fn hash<T: Serialize>(value: &T) -> Result<String> {
    content_digest(value).map_err(|_| AttentionError::Invalid)
}
pub(crate) fn validate_scope(scope: &AttentionScope) -> Result<()> {
    if !id(&scope.journal_id)
        || !id(&scope.epoch)
        || scope.recipients.is_empty()
        || scope.recipients.len() > 16
        || scope.recipients.windows(2).any(|w| w[0] >= w[1])
    {
        return Err(AttentionError::Invalid);
    }
    json(scope, MAX_DEFINITION_BYTES)?;
    Ok(())
}
pub(crate) fn validate_definition(definition: &AttentionDefinition) -> Result<()> {
    validate_scope(&definition.scope)?;
    json(definition, MAX_DEFINITION_BYTES)?;
    AttentionGovernor::new(definition.config.clone())?;
    Ok(())
}
pub(crate) fn validate_command(command: &AttentionCommand) -> Result<()> {
    validate_scope(&command.scope)?;
    if !id(&command.command_id)
        || !digest(&command.config_digest)
        || command.expected_prior.as_ref().is_some_and(|c| {
            !digest(&c.state_digest)
                || c.revision
                    .parse::<u64>()
                    .ok()
                    .is_none_or(|n| n == 0 || n.to_string() != c.revision)
        })
    {
        return Err(AttentionError::Invalid);
    }
    match &command.operation {
        AttentionOperation::Observe { update } => {
            update.validate()?;
            if update.scope.project_id != command.scope.project_id
                || !command.scope.recipients.contains(&update.recipient)
            {
                return Err(AttentionError::Invalid);
            }
        }
        AttentionOperation::TakeDelivery { recipient, intent } => {
            if !command.scope.recipients.contains(recipient) {
                return Err(AttentionError::Invalid);
            }
            // Structural depth/width validation must precede recursive JSON serialization.
            outbox::prepare_intent(intent)?;
            json(&intent.payload, MAX_CALLER_PAYLOAD_BYTES)?;
        }
    }
    json(command, MAX_COMMAND_BYTES)?;
    Ok(())
}

fn bounds(connection: &Connection) -> Result<(i64, i64, i64)> {
    let (journals, definitions): (i64, i64) = connection.query_row(
        "SELECT count(*),COALESCE(sum(length(CAST(definition_json AS BLOB))),0) FROM main.attention_journals", [],
        |r| Ok((r.get(0)?,r.get(1)?)))?;
    let (commands, bytes): (i64, i64) = connection.query_row(
        "SELECT count(*),COALESCE(sum(length(CAST(command_json AS BLOB))+length(CAST(receipt_json AS BLOB))),0) FROM main.attention_commands", [],
        |r| Ok((r.get(0)?,r.get(1)?)))?;
    if journals > MAX_JOURNALS as i64
        || definitions > (MAX_JOURNALS * MAX_DEFINITION_BYTES) as i64
        || commands > MAX_COMMANDS as i64
        || bytes > MAX_HISTORY_BYTES as i64
    {
        return Err(AttentionError::Capacity);
    }
    Ok((journals, commands, bytes))
}
pub(crate) fn initialize(connection: &Connection) -> Result<()> {
    let tx = Transaction::new_unchecked(connection, TransactionBehavior::Immediate)?;
    tx.execute_batch(include_str!("../migrations/0005_attention_journal.sql"))?;
    tx.commit()?;
    validate_all(connection)
}
struct DefinitionRow {
    definition: AttentionDefinition,
    definition_digest: String,
    head: Option<AttentionCursor>,
}
fn definition(connection: &Connection, journal_id: &str) -> Result<Option<DefinitionRow>> {
    let raw = connection.query_row(
        "SELECT journal_id,definition_json,definition_digest,head_sequence,head_digest FROM main.attention_journals WHERE journal_id=?1", [journal_id], |r| {
            Ok((crate::stored_text(r,0,128)?, crate::stored_text(r,1,MAX_DEFINITION_BYTES)?,
                crate::stored_text(r,2,71)?, r.get::<_,i64>(3)?,
                if matches!(r.get_ref(4)?, rusqlite::types::ValueRef::Null) { None } else { Some(crate::stored_text(r,4,71)?) }))
        }).optional()?;
    raw.map(|(id, bytes, expected, sequence, state_digest)| {
        let definition: AttentionDefinition = serde_json::from_str(&bytes)?;
        validate_definition(&definition).map_err(|_| AttentionError::Integrity)?;
        if definition.scope.journal_id != id
            || id != journal_id
            || hash(&definition)? != expected
            || json(&definition, MAX_DEFINITION_BYTES)? != bytes
            || sequence < 0
            || (sequence == 0) != state_digest.is_none()
            || state_digest.as_ref().is_some_and(|d| !digest(d))
        {
            return Err(AttentionError::Integrity);
        }
        Ok(DefinitionRow {
            definition,
            definition_digest: expected,
            head: state_digest.map(|state_digest| AttentionCursor {
                revision: sequence.to_string(),
                state_digest,
            }),
        })
    })
    .transpose()
}
pub(crate) fn register(
    connection: &mut Connection,
    requested: AttentionDefinition,
) -> Result<AttentionRegistration> {
    validate_definition(&requested)?;
    let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let (count, _, _) = bounds(&tx)?;
    let existing = definition(&tx, &requested.scope.journal_id)?;
    let replayed = existing.is_some();
    if let Some(original) = existing {
        if original.definition != requested {
            return Err(AttentionError::Collision);
        }
        replay(&tx, &original, None)?;
    } else {
        if count >= MAX_JOURNALS as i64 {
            return Err(AttentionError::Capacity);
        }
        tx.execute("INSERT INTO main.attention_journals(journal_id,definition_json,definition_digest) VALUES(?1,?2,?3)",
            params![requested.scope.journal_id,json(&requested,MAX_DEFINITION_BYTES)?,hash(&requested)?])?;
    }
    tx.commit()?;
    Ok(AttentionRegistration {
        definition_digest: hash(&requested)?,
        config_digest: hash(&requested.config)?,
        replayed,
        authority_granted: false,
    })
}

fn execute(
    governor: &mut AttentionGovernor,
    command: &AttentionCommand,
) -> Result<(AttentionOutcome, Option<PreparedIntent>)> {
    match &command.operation {
        AttentionOperation::Observe { update } => Ok((
            AttentionOutcome::Observation {
                observation: governor.observe((**update).clone(), command.logical_tick)?,
            },
            None,
        )),
        AttentionOperation::TakeDelivery { recipient, intent } => {
            let decision = governor.take_delivery(recipient, command.logical_tick)?;
            let prepared = if let DeliveryDecision::Wake { delivery } = &decision {
                // All source values were bounded before cloning/constructing this
                // envelope. prepare_intent bounds the WHOLE envelope again.
                #[derive(Serialize)]
                struct WakePayload<'a> {
                    schema_version: u8,
                    kind: &'static str,
                    scope: &'a AttentionScope,
                    config_digest: &'a str,
                    command_id: &'a str,
                    delivery: &'a DeliveryBatch,
                    payload: &'a serde_json::Value,
                }
                let envelope = WakePayload {
                    schema_version: 1,
                    kind: "attention_wake",
                    scope: &command.scope,
                    config_digest: &command.config_digest,
                    command_id: &command.command_id,
                    delivery,
                    payload: &intent.payload,
                };
                admiralty_core::limits::bounded_json_size(&envelope, outbox::MAX_PAYLOAD_BYTES)
                    .map_err(|_| AttentionError::Invalid)?;
                let payload = serde_json::to_value(&envelope)?;
                Some(outbox::prepare_intent(&OutboxIntent {
                    idempotency_key: intent.idempotency_key.clone(),
                    payload,
                    not_before: intent.not_before,
                })?)
            } else {
                None
            };
            Ok((AttentionOutcome::Delivery { decision }, prepared))
        }
    }
}
fn effect(id: i64, prepared: &PreparedIntent) -> AttentionEffect {
    AttentionEffect {
        outbox_id: id.to_string(),
        idempotency_key: prepared.idempotency_key.clone(),
        payload_digest: prepared.payload_digest.clone(),
        intent_digest: prepared.intent_digest.clone(),
        not_before: prepared.not_before.to_string(),
    }
}
fn receipt(
    def: &DefinitionRow,
    command: &AttentionCommand,
    sequence: i64,
    outcome: AttentionOutcome,
    effect: Option<AttentionEffect>,
) -> Result<AttentionReceipt> {
    let state_digest = hash(&(
        "attention-prefix-v1",
        &def.definition_digest,
        sequence.to_string(),
        &command.expected_prior,
        hash(command)?,
        &outcome,
        &effect,
    ))?;
    Ok(AttentionReceipt {
        cursor: AttentionCursor {
            revision: sequence.to_string(),
            state_digest,
        },
        logical_tick: command
            .logical_tick
            .ok_or(GovernorError::UnknownClock)?
            .to_string(),
        recorded_wake: effect.is_some(),
        outcome,
        effect,
        delivered: false,
        application_acknowledged: false,
        authority_granted: false,
    })
}
fn validate_effect(connection: &Connection, id: i64, prepared: &PreparedIntent) -> Result<()> {
    let raw = connection.query_row("SELECT idempotency_key,payload,payload_digest,not_before FROM main.outbox_intents WHERE id=?1",[id],|r| {
        Ok((crate::stored_text(r,0,256)?,crate::stored_text(r,1,outbox::MAX_PAYLOAD_BYTES)?,
            crate::stored_text(r,2,71)?,r.get::<_,i64>(3)?))
    }).optional()?.ok_or(AttentionError::Integrity)?;
    if id <= 0
        || raw.0 != prepared.idempotency_key
        || raw.1 != prepared.payload
        || raw.2 != prepared.payload_digest
        || raw.3 != prepared.not_before
    {
        return Err(AttentionError::Integrity);
    }
    // Mutable pending/claimed/unknown/completed status is deliberately neither
    // rewritten nor interpreted as delivered. No re-enqueue occurs on replay.
    Ok(())
}
struct Replayed {
    governor: AttentionGovernor,
    cursor: Option<AttentionCursor>,
    matching: Option<(AttentionCommand, AttentionResult)>,
    count: i64,
}
fn replay(connection: &Connection, def: &DefinitionRow, find_id: Option<&str>) -> Result<Replayed> {
    bounds(connection)?;
    let mut governor = AttentionGovernor::new(def.definition.config.clone())?;
    let mut statement = connection.prepare("SELECT command_id,sequence,command_json,command_digest,receipt_json,receipt_digest,outbox_id FROM main.attention_commands WHERE journal_id=?1 ORDER BY sequence LIMIT 4097")?;
    let mut rows = statement.query([&def.definition.scope.journal_id])?;
    let mut cursor = None;
    let mut matching = None;
    let mut count = 0i64;
    let mut bytes = 0usize;
    while let Some(row) = rows.next()? {
        count += 1;
        if count > MAX_COMMANDS as i64 {
            return Err(AttentionError::Capacity);
        }
        let id = crate::stored_text(row, 0, 128)?;
        let sequence: i64 = row.get(1)?;
        let command_json = crate::stored_text(row, 2, MAX_COMMAND_BYTES)?;
        let command_digest = crate::stored_text(row, 3, 71)?;
        let receipt_json = crate::stored_text(row, 4, MAX_RECEIPT_BYTES)?;
        let receipt_digest = crate::stored_text(row, 5, 71)?;
        let outbox_id: Option<i64> = row.get(6)?;
        bytes = bytes
            .checked_add(command_json.len() + receipt_json.len())
            .ok_or(AttentionError::Capacity)?;
        if bytes > MAX_HISTORY_BYTES {
            return Err(AttentionError::Capacity);
        }
        let command: AttentionCommand = serde_json::from_str(&command_json)?;
        let stored: AttentionReceipt = serde_json::from_str(&receipt_json)?;
        validate_command(&command).map_err(|_| AttentionError::Integrity)?;
        if command.scope != def.definition.scope
            || command.config_digest != hash(&def.definition.config)?
            || command.command_id != id
            || command.expected_prior != cursor
            || sequence != count
            || hash(&command)? != command_digest
            || json(&command, MAX_COMMAND_BYTES)? != command_json
            || hash(&stored)? != receipt_digest
            || json(&stored, MAX_RECEIPT_BYTES)? != receipt_json
        {
            return Err(AttentionError::Integrity);
        }
        let (outcome, prepared) =
            execute(&mut governor, &command).map_err(|_| AttentionError::Integrity)?;
        let association = match (outbox_id, prepared) {
            (Some(id), Some(prepared)) => {
                validate_effect(connection, id, &prepared)?;
                Some(effect(id, &prepared))
            }
            (None, None) => None,
            _ => return Err(AttentionError::Integrity),
        };
        let expected = receipt(def, &command, sequence, outcome, association)?;
        if stored != expected {
            return Err(AttentionError::Integrity);
        }
        cursor = Some(expected.cursor.clone());
        if find_id == Some(id.as_str()) {
            matching = Some((
                command,
                AttentionResult {
                    receipt: stored,
                    receipt_digest,
                    replayed: true,
                },
            ));
        }
    }
    if cursor != def.head {
        return Err(AttentionError::Integrity);
    }
    Ok(Replayed {
        governor,
        cursor,
        matching,
        count,
    })
}
pub(crate) fn apply(
    connection: &mut Connection,
    command: AttentionCommand,
) -> Result<AttentionResult> {
    validate_command(&command)?;
    let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let def = definition(&tx, &command.scope.journal_id)?.ok_or(AttentionError::MissingJournal)?;
    if def.definition.scope != command.scope
        || hash(&def.definition.config)? != command.config_digest
    {
        return Err(AttentionError::Collision);
    }
    let mut state = replay(&tx, &def, Some(&command.command_id))?;
    if let Some((original, result)) = state.matching {
        if original != command {
            return Err(AttentionError::Collision);
        }
        tx.commit()?;
        return Ok(result);
    }
    if command.expected_prior != state.cursor {
        return Err(AttentionError::StaleCursor);
    }
    let (_, count, bytes) = bounds(&tx)?;
    if count >= MAX_COMMANDS as i64 {
        return Err(AttentionError::Capacity);
    }
    let (outcome, prepared) = execute(&mut state.governor, &command)?;
    let association = if let Some(prepared) = prepared {
        match outbox::enqueue_prepared_in_transaction(&tx, &prepared)? {
            EnqueueResult::Inserted { id } => Some(effect(id, &prepared)),
            // Even an existing exact *foreign* row cannot be associated with a
            // new decision/budget consumption. RAII rolls back the whole tx.
            EnqueueResult::ExistingExact { .. } | EnqueueResult::Collision => {
                return Err(AttentionError::Collision);
            }
        }
    } else {
        None
    };
    let receipt = receipt(&def, &command, state.count + 1, outcome, association)?;
    let command_json = json(&command, MAX_COMMAND_BYTES)?;
    let receipt_json = json(&receipt, MAX_RECEIPT_BYTES)?;
    if bytes
        .checked_add((command_json.len() + receipt_json.len()) as i64)
        .is_none_or(|n| n > MAX_HISTORY_BYTES as i64)
    {
        return Err(AttentionError::Capacity);
    }
    let receipt_digest = hash(&receipt)?;
    let outbox_id = receipt
        .effect
        .as_ref()
        .map(|e| e.outbox_id.parse::<i64>())
        .transpose()
        .map_err(|_| AttentionError::Integrity)?;
    tx.execute("INSERT INTO main.attention_commands(journal_id,command_id,sequence,command_json,command_digest,receipt_json,receipt_digest,outbox_id) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
        params![command.scope.journal_id,command.command_id,state.count+1,command_json,hash(&command)?,receipt_json,receipt_digest,outbox_id])?;
    tx.execute(
        "UPDATE main.attention_journals SET head_sequence=?2,head_digest=?3 WHERE journal_id=?1",
        params![
            command.scope.journal_id,
            state.count + 1,
            receipt.cursor.state_digest
        ],
    )?;
    tx.commit()?;
    Ok(AttentionResult {
        receipt,
        receipt_digest,
        replayed: false,
    })
}
pub(crate) fn status(
    connection: &Connection,
    scope: &AttentionScope,
) -> Result<Option<AttentionStatus>> {
    validate_scope(scope)?;
    let Some(def) = definition(connection, &scope.journal_id)? else {
        return Ok(None);
    };
    if &def.definition.scope != scope {
        return Err(AttentionError::MissingJournal);
    }
    let state = replay(connection, &def, None)?;
    Ok(Some(AttentionStatus {
        scope: scope.clone(),
        config_digest: hash(&def.definition.config)?,
        cursor: state.cursor,
        pending_refs: state.governor.pending_refs(),
        topics: state.governor.topic_count(),
        authority_granted: false,
    }))
}
pub(crate) fn validate_all(connection: &Connection) -> Result<()> {
    bounds(connection)?;
    let orphan: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM main.attention_commands c LEFT JOIN main.attention_journals j USING(journal_id) WHERE j.journal_id IS NULL)",[],|r|r.get(0))?;
    if orphan {
        return Err(AttentionError::Integrity);
    }
    let mut statement = connection
        .prepare("SELECT journal_id FROM main.attention_journals ORDER BY journal_id LIMIT 65")?;
    let mut rows = statement.query([])?;
    let mut count = 0;
    while let Some(row) = rows.next()? {
        count += 1;
        if count > MAX_JOURNALS {
            return Err(AttentionError::Capacity);
        }
        let id = crate::stored_text(row, 0, 128)?;
        let def = definition(connection, &id)?.ok_or(AttentionError::Integrity)?;
        replay(connection, &def, None)?;
    }
    Ok(())
}
