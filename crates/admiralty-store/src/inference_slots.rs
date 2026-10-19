//! Bounded reserve-only inference-slot journal. Proofs and policy stay process-local.
//!
//! A pool's capacity key is an independently authenticated physical identity. This
//! journal preserves that identity and all holds; it cannot attest the physical
//! mapping, reconcile external consumers, or free capacity.
use admiralty_core::limits::bounded_json_size;
use admiralty_core::{SchedulingState, SourceIdentity, TaskView, content_digest};
use admiralty_runtime::reservations::{CurrencyUnit, ReservationScope};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{any::Any, collections::BTreeMap, sync::Arc};

pub const MAX_COMMAND_BYTES: usize = 64 * 1024;
const MAX_LEDGER_BYTES: usize = 1024 * 1024;
const MAX_RECEIPT_BYTES: usize = 8192;
const MAX_POOLS: usize = 128;
const MAX_HOLDS: usize = 1024;
const MAX_COMMANDS: i64 = 8192;
const MAX_TOTAL_BYTES: i64 = 64 * 1024 * 1024;
pub type Result<T> = std::result::Result<T, SlotStoreError>;
pub type SlotProof = Arc<dyn Any + Send + Sync>;

#[derive(Debug, thiserror::Error)]
pub enum SlotStoreError {
    #[error("inference slot SQLite failure: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("inference slot JSON failure: {0}")]
    Json(#[from] serde_json::Error),
    #[error("inference slot command is invalid or exceeds bounds")]
    Invalid,
    #[error("inference slot authority is not currently authorized")]
    Denied,
    #[error("inference slot command, source, or hold identity was reused")]
    Collision,
    #[error("inference slot ledger compare-and-swap failed")]
    Stale,
    #[error("inference slot capacity is full; no hold was freed or evicted")]
    Capacity,
    #[error("inference slot journal integrity failure")]
    Integrity,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SlotCommand {
    pub schema_version: u32,
    pub command_id: String,
    pub source: SourceIdentity,
    #[serde(with = "decimal_u64")]
    pub expected_ledger_revision: u64,
    pub expected_ledger_digest: String,
    pub event: SlotEvent,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
// Keep the frozen public command API's directly owned claim; command size is bounded.
#[allow(clippy::large_enum_variant)]
pub enum SlotEvent {
    SetPool {
        pool_id: String,
        capacity_key: String,
        generation: String,
        managed_capacity: u16,
    },
    Hold {
        hold_id: String,
        claim: SlotClaim,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SlotClaim {
    pub scope: ReservationScope,
    pub pool_id: String,
    pub generation: String,
    pub model_id: String,
    pub model_config_digest: String,
    pub inference_claims_digest: String,
    pub kernel_claims_digest: String,
    pub budget_reservation_id: String,
    pub budget_command_digest: String,
    pub launch_intent_digest: String,
    pub slots: u16,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SlotPool {
    pub capacity_key: String,
    pub generation: String,
    pub managed_capacity: u16,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SlotHold {
    pub id: String,
    pub claim: SlotClaim,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SlotLedger {
    #[serde(with = "decimal_u64")]
    pub revision: u64,
    pub pools: BTreeMap<String, SlotPool>,
    pub holds: BTreeMap<String, SlotHold>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SlotReceipt {
    pub command_id: String,
    pub source: SourceIdentity,
    pub command_digest: String,
    #[serde(with = "decimal_u64")]
    pub ledger_revision: u64,
    pub ledger_digest: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SlotResult {
    pub receipt: SlotReceipt,
    pub receipt_digest: String,
    pub replayed: bool,
}

pub struct SlotBinding<'a> {
    pub task: Option<&'a TaskView>,
    pub command: &'a SlotCommand,
    pub command_digest: &'a str,
    pub ledger: &'a SlotLedger,
    /// True only when immutable lifecycle history binds an exact replay's attempt.
    pub historical_attempt: bool,
}

/// Installed at Store startup; request JSON cannot supply an authority.
/// Callbacks must be bounded, nonblocking, non-reentrant, and perform no I/O.
/// Policy must independently attest physical capacity identity and allotment, as
/// well as the claim's model, resource, budget, scope, and launch bindings.
pub trait InferenceSlotAuthority: Send + Sync {
    fn authorize_write(
        &self,
        _binding: &SlotBinding<'_>,
        _proof: &(dyn Any + Send + Sync),
    ) -> bool {
        false
    }

    fn authorize_read(&self, _ledger: &SlotLedger, _proof: &(dyn Any + Send + Sync)) -> bool {
        false
    }
}

mod decimal_u64 {
    use serde::{Deserialize, Serialize};

    pub fn serialize<S: serde::Serializer>(value: &u64, serializer: S) -> Result<S::Ok, S::Error> {
        value.to_string().serialize(serializer)
    }

    pub fn deserialize<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
        let value = String::deserialize(deserializer)?;
        if value.len() > 20 {
            return Err(serde::de::Error::custom("noncanonical decimal u64"));
        }
        let parsed = value.parse::<u64>().map_err(serde::de::Error::custom)?;
        if parsed.to_string() != value {
            return Err(serde::de::Error::custom("noncanonical decimal u64"));
        }
        Ok(parsed)
    }
}

fn id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b))
}

fn digest_valid(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

fn hash<T: Serialize + ?Sized>(domain: &str, value: &T, limit: usize) -> Result<String> {
    bounded_json_size(value, limit).map_err(|_| SlotStoreError::Invalid)?;
    let mut hash = Sha256::new();
    hash.update(domain.as_bytes());
    hash.update([0]);
    hash.update(serde_json::to_vec(value)?);
    Ok(format!("sha256:{:x}", hash.finalize()))
}

pub fn ledger_digest(ledger: &SlotLedger) -> Result<String> {
    hash(
        "admiralty:inference-slot-ledger:v1",
        ledger,
        MAX_LEDGER_BYTES,
    )
}

pub fn receipt_digest(receipt: &SlotReceipt) -> Result<String> {
    hash(
        "admiralty:inference-slot-receipt:v1",
        receipt,
        MAX_RECEIPT_BYTES,
    )
}

fn source_key(source: &SourceIdentity) -> Result<String> {
    Ok(serde_json::to_string(source)?)
}

fn claim_valid(claim: &SlotClaim) -> bool {
    let scope = &claim.scope;
    [
        scope.project_id.as_str(),
        scope.root_id.as_str(),
        scope.task_id.as_str(),
        scope.attempt_id.as_str(),
        scope.owner.as_str(),
        &scope.incarnation,
        scope.account_id.as_str(),
        &claim.pool_id,
        &claim.generation,
        &claim.model_id,
        &claim.budget_reservation_id,
    ]
    .into_iter()
    .all(id)
        && scope.contract_revision > 0
        && !scope.authority_ref.is_empty()
        && scope.authority_ref.len() <= 512
        && scope.authority_ref.bytes().all(|b| b.is_ascii_graphic())
        && CurrencyUnit::new(&scope.unit.code, scope.unit.scale).is_ok()
        && [
            &scope.contract_digest,
            &scope.resource_claims_digest,
            &claim.model_config_digest,
            &claim.inference_claims_digest,
            &claim.kernel_claims_digest,
            &claim.budget_command_digest,
            &claim.launch_intent_digest,
        ]
        .into_iter()
        .all(|value| digest_valid(value))
        && claim.kernel_claims_digest == scope.resource_claims_digest
        && claim.slots > 0
}

pub fn validate_command(command: &SlotCommand) -> Result<String> {
    let event_valid = match &command.event {
        SlotEvent::SetPool {
            pool_id,
            capacity_key,
            generation,
            ..
        } => id(pool_id) && digest_valid(capacity_key) && id(generation),
        SlotEvent::Hold { hold_id, claim } => {
            id(hold_id)
                && claim_valid(claim)
                && command.source.incarnation == claim.scope.incarnation
        }
    };
    if command.schema_version != 1
        || !id(&command.command_id)
        || !id(&command.source.producer_id)
        || !id(&command.source.incarnation)
        || command.source.sequence.len() > 20
        || command.source.validate().is_err()
        || !digest_valid(&command.expected_ledger_digest)
        || !event_valid
    {
        return Err(SlotStoreError::Invalid);
    }
    hash(
        "admiralty:inference-slot-command:v1",
        command,
        MAX_COMMAND_BYTES,
    )
}

/// Counts every immutable hold, including holds from previous pool generations.
fn occupancy(ledger: &SlotLedger, pool_id: &str) -> Result<u64> {
    ledger
        .holds
        .values()
        .filter(|hold| hold.claim.pool_id == pool_id)
        .try_fold(0u64, |total, hold| {
            total
                .checked_add(u64::from(hold.claim.slots))
                .ok_or(SlotStoreError::Capacity)
        })
}

fn validate_ledger(ledger: &SlotLedger) -> Result<()> {
    if ledger.pools.len() > MAX_POOLS || ledger.holds.len() > MAX_HOLDS {
        return Err(SlotStoreError::Capacity);
    }
    let mut keys = std::collections::BTreeSet::new();
    for (pool_id, pool) in &ledger.pools {
        if !id(pool_id)
            || !id(&pool.generation)
            || !digest_valid(&pool.capacity_key)
            || !keys.insert(&pool.capacity_key)
            || occupancy(ledger, pool_id)? > u64::from(pool.managed_capacity)
        {
            return Err(SlotStoreError::Invalid);
        }
    }
    for (hold_id, hold) in &ledger.holds {
        if !id(hold_id)
            || hold_id != &hold.id
            || !claim_valid(&hold.claim)
            || !ledger.pools.contains_key(&hold.claim.pool_id)
        {
            return Err(SlotStoreError::Invalid);
        }
    }
    bounded_json_size(ledger, MAX_LEDGER_BYTES).map_err(|_| SlotStoreError::Capacity)?;
    Ok(())
}

fn reduce(ledger: &SlotLedger, event: &SlotEvent) -> Result<SlotLedger> {
    validate_ledger(ledger)?;
    let mut next = ledger.clone();
    match event {
        SlotEvent::SetPool {
            pool_id,
            capacity_key,
            generation,
            managed_capacity,
        } => {
            if ledger
                .pools
                .get(pool_id)
                .is_some_and(|pool| &pool.capacity_key != capacity_key)
                || ledger.pools.iter().any(|(other_id, pool)| {
                    other_id != pool_id && &pool.capacity_key == capacity_key
                })
            {
                return Err(SlotStoreError::Invalid);
            }
            if (!ledger.pools.contains_key(pool_id) && ledger.pools.len() >= MAX_POOLS)
                || occupancy(ledger, pool_id)? > u64::from(*managed_capacity)
            {
                return Err(SlotStoreError::Capacity);
            }
            next.pools.insert(
                pool_id.clone(),
                SlotPool {
                    capacity_key: capacity_key.clone(),
                    generation: generation.clone(),
                    managed_capacity: *managed_capacity,
                },
            );
        }
        SlotEvent::Hold { hold_id, claim } => {
            if ledger.holds.contains_key(hold_id) {
                return Err(SlotStoreError::Collision);
            }
            // Unknown capacity and stale generations cannot confer an allotment.
            let pool = ledger
                .pools
                .get(&claim.pool_id)
                .ok_or(SlotStoreError::Denied)?;
            if pool.generation != claim.generation {
                return Err(SlotStoreError::Denied);
            }
            let total = occupancy(ledger, &claim.pool_id)?
                .checked_add(u64::from(claim.slots))
                .ok_or(SlotStoreError::Capacity)?;
            if ledger.holds.len() >= MAX_HOLDS || total > u64::from(pool.managed_capacity) {
                return Err(SlotStoreError::Capacity);
            }
            next.holds.insert(
                hold_id.clone(),
                SlotHold {
                    id: hold_id.clone(),
                    claim: claim.clone(),
                },
            );
        }
    }
    next.revision = ledger
        .revision
        .checked_add(1)
        .ok_or(SlotStoreError::Capacity)?;
    validate_ledger(&next)?;
    Ok(next)
}

fn task_matches(
    task: &TaskView,
    command: &SlotCommand,
    claim: &SlotClaim,
    exact_replay: bool,
) -> Option<bool> {
    let scope = &claim.scope;
    let contract = &task.contract;
    if contract.project_id != scope.project_id
        || contract.root_id != scope.root_id
        || contract.task_id != scope.task_id
        || contract.revision != scope.contract_revision
        || contract.authority_ref != scope.authority_ref
        || !content_digest(contract).is_ok_and(|digest| digest == scope.contract_digest)
        || command.source.incarnation != scope.incarnation
    {
        return None;
    }
    if task.attempt_id.as_ref() == Some(&scope.attempt_id)
        && task.owner.as_ref() == Some(&scope.owner)
    {
        return Some(false);
    }
    if exact_replay
        && task.lifecycle.as_ref().is_some_and(|lifecycle| {
            lifecycle.attempts.get(&scope.attempt_id) == Some(&scope.owner)
        })
    {
        return Some(true);
    }
    if !exact_replay
        && task.state == SchedulingState::Ready
        && task.attempt_id.is_none()
        && task.owner.is_none()
    {
        return Some(false);
    }
    None
}

fn authorize(
    command: &SlotCommand,
    command_digest: &str,
    ledger: &SlotLedger,
    authority: Option<&dyn InferenceSlotAuthority>,
    proof: &(dyn Any + Send + Sync),
    task: Option<&TaskView>,
    exact_replay: bool,
) -> Result<()> {
    let historical_attempt = match (&command.event, task) {
        (SlotEvent::Hold { claim, .. }, Some(task)) => {
            task_matches(task, command, claim, exact_replay).ok_or(SlotStoreError::Denied)?
        }
        (SlotEvent::SetPool { .. }, None) => false,
        _ => return Err(SlotStoreError::Denied),
    };
    let authority = authority.ok_or(SlotStoreError::Denied)?;
    if !authority.authorize_write(
        &SlotBinding {
            task,
            command,
            command_digest,
            ledger,
            historical_attempt,
        },
        proof,
    ) {
        return Err(SlotStoreError::Denied);
    }
    Ok(())
}

pub(crate) fn initialize(connection: &Connection) -> Result<()> {
    connection.execute_batch(include_str!("../migrations/0010_inference_slots.sql"))?;
    let empty = SlotLedger::default();
    connection.execute(
        "INSERT OR IGNORE INTO inference_slot_ledger(singleton,revision,ledger_json,ledger_digest) VALUES(1,'0',?1,?2)",
        params![serde_json::to_string(&empty)?, ledger_digest(&empty)?],
    )?;
    validate_all(connection)
}

fn decode<T: serde::de::DeserializeOwned + Serialize>(json: &str, max: usize) -> Result<T> {
    if json.len() > max {
        return Err(SlotStoreError::Integrity);
    }
    let decoded: T = serde_json::from_str(json).map_err(|_| SlotStoreError::Integrity)?;
    if serde_json::to_string(&decoded)? != json {
        return Err(SlotStoreError::Integrity);
    }
    Ok(decoded)
}

fn capacity(connection: &Connection) -> Result<(i64, i64)> {
    let (rows, bytes): (i64, i64) = connection.query_row(
        "SELECT count(*),coalesce(sum(length(CAST(command_id AS BLOB))+length(CAST(source_key AS BLOB))+length(CAST(command_json AS BLOB))+length(CAST(command_digest AS BLOB))+length(CAST(receipt_json AS BLOB))+length(CAST(receipt_digest AS BLOB))),0) FROM inference_slot_commands",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if rows > MAX_COMMANDS || bytes > MAX_TOTAL_BYTES || rows < 0 || bytes < 0 {
        return Err(SlotStoreError::Capacity);
    }
    Ok((rows, bytes))
}

fn load_ledger(connection: &Connection) -> Result<SlotLedger> {
    let (revision, json, digest): (String, String, String) = connection.query_row(
        "SELECT revision,ledger_json,ledger_digest FROM inference_slot_ledger WHERE singleton=1",
        [],
        |row| {
            Ok((
                crate::stored_text(row, 0, 20)?,
                crate::stored_text(row, 1, MAX_LEDGER_BYTES)?,
                crate::stored_text(row, 2, 71)?,
            ))
        },
    )?;
    let ledger: SlotLedger = decode(&json, MAX_LEDGER_BYTES)?;
    if revision
        .parse::<u64>()
        .ok()
        .filter(|n| n.to_string() == revision)
        != Some(ledger.revision)
        || validate_ledger(&ledger).is_err()
        || ledger_digest(&ledger)? != digest
    {
        return Err(SlotStoreError::Integrity);
    }
    Ok(ledger)
}

fn replay(connection: &Connection) -> Result<(SlotLedger, usize)> {
    capacity(connection)?;
    let mut ledger = SlotLedger::default();
    let mut count = 0usize;
    let mut statement = connection.prepare(
        "SELECT seq,command_id,source_key,command_json,command_digest,receipt_json,receipt_digest FROM inference_slot_commands ORDER BY seq",
    )?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let seq: i64 = row.get(0)?;
        let command_id = crate::stored_text(row, 1, 128)?;
        let stored_source = crate::stored_text(row, 2, 512)?;
        let command_json = crate::stored_text(row, 3, MAX_COMMAND_BYTES)?;
        let stored_command_digest = crate::stored_text(row, 4, 71)?;
        let receipt_json = crate::stored_text(row, 5, MAX_RECEIPT_BYTES)?;
        let stored_receipt_digest = crate::stored_text(row, 6, 71)?;
        if count >= MAX_COMMANDS as usize || seq != count as i64 + 1 {
            return Err(SlotStoreError::Integrity);
        }
        let command: SlotCommand = decode(&command_json, MAX_COMMAND_BYTES)?;
        let command_digest = validate_command(&command).map_err(|_| SlotStoreError::Integrity)?;
        if command_id != command.command_id
            || stored_source != source_key(&command.source)?
            || stored_command_digest != command_digest
            || command.expected_ledger_revision != ledger.revision
            || command.expected_ledger_digest != ledger_digest(&ledger)?
        {
            return Err(SlotStoreError::Integrity);
        }
        let next = reduce(&ledger, &command.event).map_err(|_| SlotStoreError::Integrity)?;
        let receipt: SlotReceipt = decode(&receipt_json, MAX_RECEIPT_BYTES)?;
        let expected = SlotReceipt {
            command_id: command.command_id,
            source: command.source,
            command_digest,
            ledger_revision: next.revision,
            ledger_digest: ledger_digest(&next)?,
        };
        if receipt != expected || receipt_digest(&receipt)? != stored_receipt_digest {
            return Err(SlotStoreError::Integrity);
        }
        ledger = next;
        count = count.checked_add(1).ok_or(SlotStoreError::Integrity)?;
    }
    Ok((ledger, count))
}

pub(crate) fn validate_all(connection: &Connection) -> Result<()> {
    let (computed, _) = replay(connection)?;
    if computed != load_ledger(connection)? {
        return Err(SlotStoreError::Integrity);
    }
    Ok(())
}

pub(crate) fn read_authorized(
    connection: &Connection,
    authority: &dyn InferenceSlotAuthority,
    proof: &(dyn Any + Send + Sync),
) -> Result<SlotLedger> {
    let ledger = load_ledger(connection)?;
    if !authority.authorize_read(&ledger, proof) {
        return Err(SlotStoreError::Denied);
    }
    let (computed, _) = replay(connection)?;
    if computed != ledger {
        return Err(SlotStoreError::Integrity);
    }
    if !authority.authorize_read(&ledger, proof) {
        return Err(SlotStoreError::Denied);
    }
    Ok(ledger)
}

struct PriorCommand {
    command_id: String,
    source_key: String,
    command_json: String,
    command_digest: String,
    receipt_json: String,
    receipt_digest: String,
}

fn prior_command(
    connection: &Connection,
    command: &SlotCommand,
    command_json: &str,
    command_digest: &str,
) -> Result<(Option<PriorCommand>, bool)> {
    let source = source_key(&command.source)?;
    let query = |column: &str, value: &str| -> Result<Option<PriorCommand>> {
        // Only fixed local column names reach this statement.
        Ok(connection
            .query_row(
                &format!(
                    "SELECT command_id,source_key,command_json,command_digest,receipt_json,receipt_digest FROM inference_slot_commands WHERE {column}=?1"
                ),
                params![value],
                |row| {
                    Ok(PriorCommand {
                        command_id: crate::stored_text(row, 0, 128)?,
                        source_key: crate::stored_text(row, 1, 512)?,
                        command_json: crate::stored_text(row, 2, MAX_COMMAND_BYTES)?,
                        command_digest: crate::stored_text(row, 3, 71)?,
                        receipt_json: crate::stored_text(row, 4, MAX_RECEIPT_BYTES)?,
                        receipt_digest: crate::stored_text(row, 5, 71)?,
                    })
                },
            )
            .optional()?)
    };
    let by_command = query("command_id", &command.command_id)?;
    let by_source = query("source_key", &source)?;
    let split_identity = by_command.as_ref().is_some_and(|a| {
        by_source
            .as_ref()
            .is_some_and(|b| a.command_id != b.command_id)
    });
    let prior = by_command.or(by_source);
    let exact = !split_identity
        && prior.as_ref().is_some_and(|entry| {
            entry.command_id == command.command_id
                && entry.source_key == source
                && entry.command_json == command_json
                && entry.command_digest == command_digest
        });
    Ok((prior, exact))
}

pub(crate) fn apply_in_transaction(
    transaction: &Transaction<'_>,
    command: &SlotCommand,
    authority: Option<&dyn InferenceSlotAuthority>,
    proof: &(dyn Any + Send + Sync),
    task: Option<&TaskView>,
) -> Result<SlotResult> {
    let command_digest = validate_command(command)?;
    let current = load_ledger(transaction)?;
    let command_json = serde_json::to_string(command)?;
    let (prior, exact_replay) =
        prior_command(transaction, command, &command_json, &command_digest)?;
    let authorize = |ledger: &SlotLedger| {
        authorize(
            command,
            &command_digest,
            ledger,
            authority,
            proof,
            task,
            exact_replay,
        )
    };
    authorize(&current)?;
    if prior.is_some() && !exact_replay {
        return Err(SlotStoreError::Collision);
    }
    let (computed, rows) = replay(transaction)?;
    if computed != current {
        return Err(SlotStoreError::Integrity);
    }
    if let Some(prior) = prior {
        let receipt: SlotReceipt = decode(&prior.receipt_json, MAX_RECEIPT_BYTES)?;
        let receipt_digest = receipt_digest(&receipt)?;
        if receipt_digest != prior.receipt_digest {
            return Err(SlotStoreError::Integrity);
        }
        authorize(&current)?;
        return Ok(SlotResult {
            receipt,
            receipt_digest,
            replayed: true,
        });
    }
    if command.expected_ledger_revision != current.revision
        || command.expected_ledger_digest != ledger_digest(&current)?
    {
        return Err(SlotStoreError::Stale);
    }
    let next = reduce(&current, &command.event)?;
    let next_digest = ledger_digest(&next)?;
    let receipt = SlotReceipt {
        command_id: command.command_id.clone(),
        source: command.source.clone(),
        command_digest: command_digest.clone(),
        ledger_revision: next.revision,
        ledger_digest: next_digest.clone(),
    };
    let receipt_json = serde_json::to_string(&receipt)?;
    let receipt_digest = receipt_digest(&receipt)?;
    authorize(&current)?;
    authorize(&next)?;
    let (_, used) = capacity(transaction)?;
    if rows >= MAX_COMMANDS as usize {
        return Err(SlotStoreError::Capacity);
    }
    let source = source_key(&command.source)?;
    let added = [
        command.command_id.len(),
        source.len(),
        command_json.len(),
        command_digest.len(),
        receipt_json.len(),
        receipt_digest.len(),
    ]
    .into_iter()
    .try_fold(0i64, |total, length| {
        let length = i64::try_from(length).map_err(|_| SlotStoreError::Capacity)?;
        total.checked_add(length).ok_or(SlotStoreError::Capacity)
    })?;
    if used
        .checked_add(added)
        .is_none_or(|total| total > MAX_TOTAL_BYTES)
    {
        return Err(SlotStoreError::Capacity);
    }
    transaction.execute(
        "INSERT INTO inference_slot_commands(command_id,source_key,command_json,command_digest,receipt_json,receipt_digest) VALUES(?1,?2,?3,?4,?5,?6)",
        params![command.command_id, source, command_json, command_digest, receipt_json, receipt_digest],
    )?;
    let updated = transaction.execute(
        "UPDATE inference_slot_ledger SET revision=?1,ledger_json=?2,ledger_digest=?3 WHERE singleton=1 AND revision=?4 AND ledger_digest=?5",
        params![next.revision.to_string(), serde_json::to_string(&next)?, next_digest, current.revision.to_string(), ledger_digest(&current)?],
    )?;
    if updated != 1 {
        return Err(SlotStoreError::Stale);
    }
    let (computed, _) = replay(transaction)?;
    if computed != next || load_ledger(transaction)? != next {
        return Err(SlotStoreError::Integrity);
    }
    // This is still the staged new command: its preclaim Ready view is valid here.
    // The Store's final callback supplies the actual postclaim task projection.
    authorize(&next)?;
    Ok(SlotResult {
        receipt,
        receipt_digest,
        replayed: false,
    })
}

pub(crate) fn reauthorize_in_transaction(
    transaction: &Transaction<'_>,
    command: &SlotCommand,
    authority: Option<&dyn InferenceSlotAuthority>,
    proof: &(dyn Any + Send + Sync),
    task: Option<&TaskView>,
) -> Result<()> {
    let command_digest = validate_command(command)?;
    let ledger = load_ledger(transaction)?;
    let command_json = serde_json::to_string(command)?;
    let (prior, exact_replay) =
        prior_command(transaction, command, &command_json, &command_digest)?;
    let authorize = || {
        authorize(
            command,
            &command_digest,
            &ledger,
            authority,
            proof,
            task,
            exact_replay,
        )
    };
    authorize()?;
    if prior.is_some() && !exact_replay {
        return Err(SlotStoreError::Collision);
    }
    let (computed, _) = replay(transaction)?;
    if computed != ledger {
        return Err(SlotStoreError::Integrity);
    }
    authorize()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use admiralty_core::{TaskContract, TaskLifecycleView};

    fn digest(ch: char) -> String {
        format!("sha256:{}", ch.to_string().repeat(64))
    }

    fn contract() -> TaskContract {
        serde_json::from_value(serde_json::json!({
            "schema_version": 1, "project_id": "project", "root_id": "root", "task_id": "task", "revision": 1,
            "title": "Task", "objective": "Reserve inference", "non_goals": [], "owned_paths": ["src/lib.rs"],
            "forbidden_paths": [], "read_first": [], "dependencies": [],
            "criteria": [{"id": "criterion", "text": "Bound slot", "check_ids": ["check"]}],
            "checks": [{"id": "check", "program": "true", "args": [], "cwd": ".", "timeout_secs": 1}],
            "authority_ref": "grant:inference", "max_attempts": 2, "simulated": true
        })).unwrap()
    }

    fn claim(generation: &str, slots: u16) -> SlotClaim {
        let contract = contract();
        SlotClaim {
            scope: ReservationScope {
                project_id: contract.project_id.clone(),
                root_id: contract.root_id.clone(),
                task_id: contract.task_id.clone(),
                attempt_id: admiralty_core::AttemptId::new("attempt").unwrap(),
                owner: admiralty_core::PrincipalId::new("worker").unwrap(),
                incarnation: "boot".into(),
                contract_revision: contract.revision,
                contract_digest: content_digest(&contract).unwrap(),
                authority_ref: contract.authority_ref.clone(),
                resource_claims_digest: digest('a'),
                account_id: admiralty_runtime::reservations::AccountId::new("account").unwrap(),
                unit: CurrencyUnit::new("USD", 2).unwrap(),
            },
            pool_id: "pool".into(),
            generation: generation.into(),
            model_id: "model".into(),
            model_config_digest: digest('b'),
            inference_claims_digest: digest('c'),
            kernel_claims_digest: digest('a'),
            budget_reservation_id: "budget".into(),
            budget_command_digest: digest('d'),
            launch_intent_digest: digest('e'),
            slots,
        }
    }

    fn pool(generation: &str, capacity: u16) -> SlotEvent {
        SlotEvent::SetPool {
            pool_id: "pool".into(),
            capacity_key: digest('f'),
            generation: generation.into(),
            managed_capacity: capacity,
        }
    }

    fn command(event: SlotEvent) -> SlotCommand {
        SlotCommand {
            schema_version: 1,
            command_id: "command".into(),
            source: SourceIdentity {
                producer_id: "producer".into(),
                incarnation: "boot".into(),
                sequence: "1".into(),
            },
            expected_ledger_revision: 0,
            expected_ledger_digest: ledger_digest(&SlotLedger::default()).unwrap(),
            event,
        }
    }

    #[test]
    fn generation_change_preserves_occupancy_and_blocks_reduction() {
        let ledger = reduce(&SlotLedger::default(), &pool("old", 2)).unwrap();
        let ledger = reduce(
            &ledger,
            &SlotEvent::Hold {
                hold_id: "held".into(),
                claim: claim("old", 2),
            },
        )
        .unwrap();
        let next = reduce(&ledger, &pool("new", 2)).unwrap();
        assert_eq!(next.holds, ledger.holds);
        assert_eq!(occupancy(&next, "pool").unwrap(), 2);
        assert!(matches!(
            reduce(&next, &pool("new", 1)),
            Err(SlotStoreError::Capacity)
        ));
        assert!(matches!(
            reduce(
                &next,
                &SlotEvent::Hold {
                    hold_id: "new".into(),
                    claim: claim("new", 1)
                }
            ),
            Err(SlotStoreError::Capacity)
        ));
        assert!(matches!(
            reduce(
                &next,
                &SlotEvent::Hold {
                    hold_id: "old".into(),
                    claim: claim("old", 1)
                }
            ),
            Err(SlotStoreError::Denied)
        ));
    }

    #[test]
    fn capacity_identity_is_unique_and_immutable() {
        let ledger = reduce(&SlotLedger::default(), &pool("old", 1)).unwrap();
        let changed = SlotEvent::SetPool {
            pool_id: "pool".into(),
            capacity_key: digest('a'),
            generation: "new".into(),
            managed_capacity: 1,
        };
        assert!(matches!(
            reduce(&ledger, &changed),
            Err(SlotStoreError::Invalid)
        ));
        let alias = SlotEvent::SetPool {
            pool_id: "alias".into(),
            capacity_key: digest('f'),
            generation: "new".into(),
            managed_capacity: 1,
        };
        assert!(matches!(
            reduce(&ledger, &alias),
            Err(SlotStoreError::Invalid)
        ));
    }

    #[test]
    fn commands_reject_unbound_digests_zero_slots_and_foreign_incarnations() {
        let valid = command(SlotEvent::Hold {
            hold_id: "hold".into(),
            claim: claim("gen", 1),
        });
        assert!(validate_command(&valid).is_ok());
        let mut changed = valid.clone();
        if let SlotEvent::Hold { claim, .. } = &mut changed.event {
            claim.kernel_claims_digest = digest('b');
        }
        assert!(matches!(
            validate_command(&changed),
            Err(SlotStoreError::Invalid)
        ));
        let mut changed = valid.clone();
        if let SlotEvent::Hold { claim, .. } = &mut changed.event {
            claim.slots = 0;
        }
        assert!(validate_command(&changed).is_err());
        let mut changed = valid;
        changed.source.incarnation = "foreign".into();
        assert!(validate_command(&changed).is_err());
    }

    #[test]
    fn revision_and_unknown_fields_are_strict() {
        let value = serde_json::to_value(SlotLedger::default()).unwrap();
        assert_eq!(value["revision"], "0");
        for revision in [
            serde_json::json!(0),
            serde_json::json!("00"),
            serde_json::json!("+1"),
            serde_json::json!("18446744073709551616"),
        ] {
            let mut changed = value.clone();
            changed["revision"] = revision;
            assert!(serde_json::from_value::<SlotLedger>(changed).is_err());
        }
        let mut event = serde_json::to_value(pool("generation", 1)).unwrap();
        event["release"] = serde_json::json!(true);
        assert!(serde_json::from_value::<SlotEvent>(event).is_err());
    }

    #[test]
    fn ready_is_new_only_and_history_is_exact_replay_only() {
        let claim = claim("generation", 1);
        let command = command(SlotEvent::Hold {
            hold_id: "hold".into(),
            claim: claim.clone(),
        });
        let mut task = TaskView {
            contract: contract(),
            state: SchedulingState::Ready,
            attempt_id: None,
            owner: None,
            candidate_ref: None,
            lifecycle: None,
        };
        assert_eq!(task_matches(&task, &command, &claim, false), Some(false));
        assert_eq!(task_matches(&task, &command, &claim, true), None);
        task.state = SchedulingState::Accepted;
        task.lifecycle = Some(TaskLifecycleView {
            attempts: [(claim.scope.attempt_id.clone(), claim.scope.owner.clone())]
                .into_iter()
                .collect(),
            ..Default::default()
        });
        assert_eq!(task_matches(&task, &command, &claim, false), None);
        assert_eq!(task_matches(&task, &command, &claim, true), Some(true));
        task.contract.revision += 1;
        assert_eq!(task_matches(&task, &command, &claim, true), None);
    }

    #[test]
    fn revision_overflow_never_wraps() {
        let ledger = SlotLedger {
            revision: u64::MAX,
            ..Default::default()
        };
        assert!(matches!(
            reduce(&ledger, &pool("generation", 1)),
            Err(SlotStoreError::Capacity)
        ));
    }
}
