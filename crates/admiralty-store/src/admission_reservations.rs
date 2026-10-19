//! Bounded durable reservation command journal. Policy and proofs remain process-local.
use admiralty_core::SchedulingState;
use admiralty_core::{SourceIdentity, TaskView, content_digest, limits::bounded_json_size};
use admiralty_runtime::reservations::{
    ReservationEvent, ReservationLedger, ReservationScope, reduce_reservation,
};
use rusqlite::{Connection, Transaction, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{any::Any, sync::Arc};

pub const MAX_COMMAND_BYTES: usize = 64 * 1024;
const MAX_LEDGER_BYTES: usize = 1024 * 1024;
const MAX_RECEIPT_BYTES: usize = 8192;
const MAX_COMMANDS: i64 = 8192;
const MAX_TOTAL_BYTES: i64 = 64 * 1024 * 1024;
pub type Result<T> = std::result::Result<T, ReservationStoreError>;
pub type ReservationProof = Arc<dyn Any + Send + Sync>;

#[derive(Debug, thiserror::Error)]
pub enum ReservationStoreError {
    #[error("reservation SQLite failure: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("reservation JSON failure: {0}")]
    Json(#[from] serde_json::Error),
    #[error("reservation reducer failure: {0}")]
    Reducer(#[from] admiralty_runtime::reservations::ReservationError),
    #[error("reservation command is invalid or exceeds bounds")]
    Invalid,
    #[error("reservation authority is not currently authorized")]
    Denied,
    #[error("reservation command or source identity was reused with different content")]
    Collision,
    #[error("reservation ledger compare-and-swap failed")]
    Stale,
    #[error("reservation journal capacity is full; nothing was evicted")]
    Capacity,
    #[error("reservation journal integrity failure")]
    Integrity,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReservationCommand {
    pub schema_version: u32,
    pub command_id: String,
    pub source: SourceIdentity,
    #[serde(with = "decimal_u64")]
    pub expected_ledger_revision: u64,
    pub expected_ledger_digest: String,
    /// Set only by the trusted atomic claim path to bind this reserve to its effect.
    pub launch_intent_digest: Option<String>,
    pub event: ReservationEvent,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReservationReceipt {
    pub command_id: String,
    pub source: SourceIdentity,
    pub command_digest: String,
    #[serde(with = "decimal_u64")]
    pub ledger_revision: u64,
    pub ledger_digest: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReservationResult {
    pub receipt: ReservationReceipt,
    pub receipt_digest: String,
    pub replayed: bool,
}

pub struct ReservationBinding<'a> {
    pub task: Option<&'a TaskView>,
    pub command: &'a ReservationCommand,
    pub command_digest: &'a str,
    pub ledger: &'a ReservationLedger,
    /// True when only recorded lifecycle history binds this old attempt to the task.
    pub historical_attempt: bool,
}

/// Installed at Store startup; request JSON cannot supply an authority.
/// Callbacks must be bounded, nonblocking, non-reentrant, and perform no I/O.
pub trait ReservationAuthority: Send + Sync {
    fn authorize_write(
        &self,
        _binding: &ReservationBinding<'_>,
        _proof: &(dyn Any + Send + Sync),
    ) -> bool {
        false
    }
    fn authorize_read(
        &self,
        _ledger: &ReservationLedger,
        _proof: &(dyn Any + Send + Sync),
    ) -> bool {
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
    value.strip_prefix("sha256:").is_some_and(|v| {
        v.len() == 64
            && v.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}
fn hash<T: Serialize + ?Sized>(domain: &str, value: &T, limit: usize) -> Result<String> {
    bounded_json_size(value, limit).map_err(|_| ReservationStoreError::Invalid)?;
    let mut h = Sha256::new();
    h.update(domain.as_bytes());
    h.update([0]);
    h.update(serde_json::to_vec(value)?);
    Ok(format!("sha256:{:x}", h.finalize()))
}
pub fn ledger_digest(ledger: &ReservationLedger) -> Result<String> {
    hash("admiralty:reservation-ledger:v1", ledger, MAX_LEDGER_BYTES)
}
fn source_key(source: &SourceIdentity) -> Result<String> {
    Ok(serde_json::to_string(source)?)
}
fn event_id(event: &ReservationEvent) -> &str {
    match event {
        ReservationEvent::Reserve { event_id, .. }
        | ReservationEvent::RequestStop { event_id, .. }
        | ReservationEvent::Settle { event_id, .. }
        | ReservationEvent::CorrectSettlement { event_id, .. }
        | ReservationEvent::SetLimits { event_id, .. } => event_id,
    }
}
fn scope(command: &ReservationCommand) -> Option<&ReservationScope> {
    match &command.event {
        ReservationEvent::Reserve { scope, .. } => Some(scope),
        ReservationEvent::RequestStop { binding, .. } => Some(&binding.scope),
        _ => None,
    }
}
fn task_matches(
    task: &TaskView,
    command: &ReservationCommand,
    scope: &ReservationScope,
    allow_historical: bool,
) -> Option<bool> {
    let contract = &task.contract;
    if contract.project_id != scope.project_id
        || contract.root_id != scope.root_id
        || contract.task_id != scope.task_id
        || contract.revision != scope.contract_revision
        || contract.authority_ref != scope.authority_ref
        || !content_digest(contract).is_ok_and(|d| d == scope.contract_digest)
        || command.source.incarnation != scope.incarnation
    {
        return None;
    }
    let current_attempt = task.attempt_id.as_ref() == Some(&scope.attempt_id)
        && task.owner.as_ref() == Some(&scope.owner);
    if current_attempt {
        return Some(false);
    }
    let historical_attempt = task
        .lifecycle
        .as_ref()
        .is_some_and(|lifecycle| lifecycle.attempts.get(&scope.attempt_id) == Some(&scope.owner));
    if allow_historical && historical_attempt {
        return Some(true);
    }
    let atomic_ready_reserve = matches!(command.event, ReservationEvent::Reserve { .. })
        && command.launch_intent_digest.is_some()
        && task.state == SchedulingState::Ready
        && task.attempt_id.is_none()
        && task.owner.is_none();
    if atomic_ready_reserve {
        Some(false)
    } else {
        None
    }
}

pub fn validate_command(command: &ReservationCommand) -> Result<String> {
    if command.schema_version != 1
        || !id(&command.command_id)
        || command.source.validate().is_err()
        || !id(&command.source.producer_id)
        || !id(&command.source.incarnation)
        || !digest_valid(&command.expected_ledger_digest)
        || command
            .launch_intent_digest
            .as_deref()
            .is_some_and(|d| !digest_valid(d))
        || (command.launch_intent_digest.is_some()
            && !matches!(command.event, ReservationEvent::Reserve { .. }))
        || event_id(&command.event) != command.command_id
    {
        return Err(ReservationStoreError::Invalid);
    }
    match &command.event {
        ReservationEvent::Reserve { .. }
        | ReservationEvent::RequestStop { .. }
        | ReservationEvent::SetLimits { .. } => {}
        ReservationEvent::Settle { .. } | ReservationEvent::CorrectSettlement { .. } => {
            return Err(ReservationStoreError::Invalid);
        }
    }
    hash(
        "admiralty:reservation-command:v1",
        command,
        MAX_COMMAND_BYTES,
    )
}

pub(crate) fn initialize(c: &Connection) -> Result<()> {
    c.execute_batch(include_str!(
        "../migrations/0009_admission_reservations.sql"
    ))?;
    let empty = ReservationLedger::default();
    let json = serde_json::to_string(&empty)?;
    let digest = ledger_digest(&empty)?;
    c.execute("INSERT OR IGNORE INTO admission_reservation_ledger(singleton,revision,ledger_json,ledger_digest) VALUES(1,'0',?1,?2)", params![json,digest])?;
    validate_all(c)
}

fn decode<T: serde::de::DeserializeOwned + Serialize>(s: &str, max: usize) -> Result<T> {
    if s.len() > max {
        return Err(ReservationStoreError::Integrity);
    }
    let v: T = serde_json::from_str(s).map_err(|_| ReservationStoreError::Integrity)?;
    if serde_json::to_string(&v)? != s {
        return Err(ReservationStoreError::Integrity);
    }
    Ok(v)
}
fn capacity(c: &Connection) -> Result<()> {
    let (rows, bytes): (i64,i64) = c.query_row("SELECT count(*),coalesce(sum(length(CAST(command_id AS BLOB))+length(CAST(source_key AS BLOB))+length(CAST(command_json AS BLOB))+length(CAST(command_digest AS BLOB))+length(CAST(receipt_json AS BLOB))+length(CAST(receipt_digest AS BLOB))),0) FROM admission_reservation_commands", [], |r| Ok((r.get(0)?,r.get(1)?)))?;
    if rows > MAX_COMMANDS || bytes > MAX_TOTAL_BYTES {
        return Err(ReservationStoreError::Capacity);
    }
    Ok(())
}
fn load_ledger(c: &Connection) -> Result<ReservationLedger> {
    let (rev,json,digest):(String,String,String) = c.query_row(
        "SELECT revision,ledger_json,ledger_digest FROM admission_reservation_ledger WHERE singleton=1", [],
        |r| Ok((crate::stored_text(r, 0, 20)?, crate::stored_text(r, 1, MAX_LEDGER_BYTES)?, crate::stored_text(r, 2, 71)?)),
    )?;
    let ledger: ReservationLedger = decode(&json, MAX_LEDGER_BYTES)?;
    if rev.parse::<u64>().ok().filter(|n| n.to_string() == rev) != Some(ledger.revision)
        || ledger_digest(&ledger)? != digest
    {
        return Err(ReservationStoreError::Integrity);
    }
    Ok(ledger)
}
fn replay(c: &Connection) -> Result<(ReservationLedger, usize)> {
    capacity(c)?;
    let mut ledger = ReservationLedger::default();
    let mut count = 0usize;
    let mut stmt = c.prepare("SELECT seq,command_id,source_key,command_json,command_digest,receipt_json,receipt_digest FROM admission_reservation_commands ORDER BY seq")?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let seq: i64 = row.get(0)?;
        let cid = crate::stored_text(row, 1, 128)?;
        let skey = crate::stored_text(row, 2, 512)?;
        let command_json = crate::stored_text(row, 3, MAX_COMMAND_BYTES)?;
        let command_digest_stored = crate::stored_text(row, 4, 71)?;
        let receipt_json = crate::stored_text(row, 5, MAX_RECEIPT_BYTES)?;
        let receipt_digest_stored = crate::stored_text(row, 6, 71)?;
        if seq != count as i64 + 1 {
            return Err(ReservationStoreError::Integrity);
        }
        let command: ReservationCommand = decode(&command_json, MAX_COMMAND_BYTES)?;
        let cd = validate_command(&command)?;
        if command.command_id != cid
            || source_key(&command.source)? != skey
            || cd != command_digest_stored
            || command.expected_ledger_revision != ledger.revision
            || command.expected_ledger_digest != ledger_digest(&ledger)?
        {
            return Err(ReservationStoreError::Integrity);
        }
        let after = reduce_reservation(&ledger, command.event.clone())?;
        let after_digest = ledger_digest(&after)?;
        let receipt: ReservationReceipt = decode(&receipt_json, MAX_RECEIPT_BYTES)?;
        let expected = ReservationReceipt {
            command_id: command.command_id.clone(),
            source: command.source.clone(),
            command_digest: cd,
            ledger_revision: after.revision,
            ledger_digest: after_digest,
        };
        let rd = hash(
            "admiralty:reservation-receipt:v1",
            &receipt,
            MAX_RECEIPT_BYTES,
        )?;
        if receipt != expected || rd != receipt_digest_stored {
            return Err(ReservationStoreError::Integrity);
        }
        ledger = after;
        count += 1;
    }
    Ok((ledger, count))
}
pub(crate) fn validate_all(c: &Connection) -> Result<()> {
    let (computed, _) = replay(c)?;
    if computed != load_ledger(c)? {
        return Err(ReservationStoreError::Integrity);
    }
    Ok(())
}
pub(crate) fn read_authorized(
    c: &Connection,
    authority: &dyn ReservationAuthority,
    proof: &(dyn Any + Send + Sync),
) -> Result<ReservationLedger> {
    let ledger = load_ledger(c)?;
    if !authority.authorize_read(&ledger, proof) {
        return Err(ReservationStoreError::Denied);
    }
    // Recheck integrity after the callback, before returning data.
    let (computed, _) = replay(c)?;
    if computed != ledger || !authority.authorize_read(&ledger, proof) {
        return Err(ReservationStoreError::Denied);
    }
    Ok(ledger)
}

pub(crate) fn apply_in_transaction(
    tx: &Transaction<'_>,
    command: &ReservationCommand,
    authority: Option<&dyn ReservationAuthority>,
    proof: &(dyn Any + Send + Sync),
    task: Option<&TaskView>,
) -> Result<ReservationResult> {
    let cd = validate_command(command)?;
    let current = load_ledger(tx)?;
    let authorize = |ledger: &ReservationLedger, allow_historical: bool| -> Result<()> {
        let historical_attempt = match (scope(command), task) {
            (Some(s), Some(t)) => task_matches(t, command, s, allow_historical)
                .ok_or(ReservationStoreError::Denied)?,
            (None, None) if matches!(command.event, ReservationEvent::SetLimits { .. }) => false,
            _ => return Err(ReservationStoreError::Denied),
        };
        let auth = authority.ok_or(ReservationStoreError::Denied)?;
        if !auth.authorize_write(
            &ReservationBinding {
                task,
                command,
                command_digest: &cd,
                ledger,
                historical_attempt,
            },
            proof,
        ) {
            return Err(ReservationStoreError::Denied);
        }
        Ok(())
    };
    let skey = source_key(&command.source)?;
    let command_json = serde_json::to_string(command)?;
    if command_json.len() > MAX_COMMAND_BYTES {
        return Err(ReservationStoreError::Invalid);
    }
    let by_command: Option<(String, String, String, String)> = tx.query_row(
        "SELECT command_id,command_json,command_digest,receipt_json FROM admission_reservation_commands WHERE command_id=?1",
        params![command.command_id], |r| Ok((
            crate::stored_text(r, 0, 128)?, crate::stored_text(r, 1, MAX_COMMAND_BYTES)?,
            crate::stored_text(r, 2, 71)?, crate::stored_text(r, 3, MAX_RECEIPT_BYTES)?,
        )),
    ).optional()?;
    let by_source: Option<(String, String, String, String)> = tx.query_row(
        "SELECT command_id,command_json,command_digest,receipt_json FROM admission_reservation_commands WHERE source_key=?1",
        params![skey], |r| Ok((
            crate::stored_text(r, 0, 128)?, crate::stored_text(r, 1, MAX_COMMAND_BYTES)?,
            crate::stored_text(r, 2, 71)?, crate::stored_text(r, 3, MAX_RECEIPT_BYTES)?,
        )),
    ).optional()?;
    let split_identity = by_command
        .as_ref()
        .is_some_and(|a| by_source.as_ref().is_some_and(|b| a.0 != b.0));
    let prior = by_command.or(by_source);
    let exact_replay = prior.as_ref().is_some_and(|(cid, json, stored_cd, _)| {
        cid == &command.command_id
            && json == &serde_json::to_string(command).unwrap_or_default()
            && stored_cd == &cd
    });
    authorize(&current, exact_replay)?;
    if split_identity || (prior.is_some() && !exact_replay) {
        return Err(ReservationStoreError::Collision);
    }
    if let Some((_cid, _json, _stored_cd, receipt_json)) = prior {
        let receipt: ReservationReceipt = decode(&receipt_json, MAX_RECEIPT_BYTES)?;
        let rd = hash(
            "admiralty:reservation-receipt:v1",
            &receipt,
            MAX_RECEIPT_BYTES,
        )?;
        let (computed, _) = replay(tx)?;
        if computed != current {
            return Err(ReservationStoreError::Integrity);
        }
        authorize(&current, true)?;
        return Ok(ReservationResult {
            receipt,
            receipt_digest: rd,
            replayed: true,
        });
    }
    if command.expected_ledger_revision != current.revision
        || command.expected_ledger_digest != ledger_digest(&current)?
    {
        return Err(ReservationStoreError::Stale);
    }
    let next = reduce_reservation(&current, command.event.clone())?;
    let next_digest = ledger_digest(&next)?;
    let receipt = ReservationReceipt {
        command_id: command.command_id.clone(),
        source: command.source.clone(),
        command_digest: cd.clone(),
        ledger_revision: next.revision,
        ledger_digest: next_digest.clone(),
    };
    let receipt_json = serde_json::to_string(&receipt)?;
    if receipt_json.len() > MAX_RECEIPT_BYTES {
        return Err(ReservationStoreError::Invalid);
    }
    let rd = hash(
        "admiralty:reservation-receipt:v1",
        &receipt,
        MAX_RECEIPT_BYTES,
    )?;
    authorize(&current, false)?;
    authorize(&next, false)?;
    let (_, rows) = replay(tx)?;
    if rows as i64 >= MAX_COMMANDS {
        return Err(ReservationStoreError::Capacity);
    }
    let used: i64 = tx.query_row("SELECT coalesce(sum(length(CAST(command_id AS BLOB))+length(CAST(source_key AS BLOB))+length(CAST(command_json AS BLOB))+length(CAST(command_digest AS BLOB))+length(CAST(receipt_json AS BLOB))+length(CAST(receipt_digest AS BLOB))),0) FROM admission_reservation_commands",[],|r|r.get(0))?;
    let added = (command.command_id.len()
        + skey.len()
        + command_json.len()
        + cd.len()
        + receipt_json.len()
        + rd.len()) as i64;
    if used.checked_add(added).is_none_or(|n| n > MAX_TOTAL_BYTES) {
        return Err(ReservationStoreError::Capacity);
    }
    tx.execute("INSERT INTO admission_reservation_commands(command_id,source_key,command_json,command_digest,receipt_json,receipt_digest) VALUES(?1,?2,?3,?4,?5,?6)",params![command.command_id,skey,command_json,cd,receipt_json,rd])?;
    tx.execute("UPDATE admission_reservation_ledger SET revision=?1,ledger_json=?2,ledger_digest=?3 WHERE singleton=1",params![next.revision.to_string(),serde_json::to_string(&next)?,next_digest])?;
    let (computed, _) = replay(tx)?;
    if computed != next {
        return Err(ReservationStoreError::Integrity);
    }
    authorize(&next, false)?;
    Ok(ReservationResult {
        receipt,
        receipt_digest: rd,
        replayed: false,
    })
}

// OptionalExtension kept local to this journal's query surface.
use rusqlite::OptionalExtension;

pub(crate) fn reauthorize_in_transaction(
    tx: &Transaction<'_>,
    command: &ReservationCommand,
    authority: Option<&dyn ReservationAuthority>,
    proof: &(dyn Any + Send + Sync),
    task: Option<&TaskView>,
) -> Result<()> {
    let cd = validate_command(command)?;
    let ledger = load_ledger(tx)?;
    let historical_attempt = match (scope(command), task) {
        (Some(s), Some(t)) => {
            task_matches(t, command, s, true).ok_or(ReservationStoreError::Denied)?
        }
        (None, None) if matches!(command.event, ReservationEvent::SetLimits { .. }) => false,
        _ => return Err(ReservationStoreError::Denied),
    };
    let auth = authority.ok_or(ReservationStoreError::Denied)?;
    let binding = ReservationBinding {
        task,
        command,
        command_digest: &cd,
        ledger: &ledger,
        historical_attempt,
    };
    if !auth.authorize_write(&binding, proof) {
        return Err(ReservationStoreError::Denied);
    }
    let (computed, _) = replay(tx)?;
    if computed != ledger {
        return Err(ReservationStoreError::Integrity);
    }
    if !auth.authorize_write(
        &ReservationBinding {
            task,
            command,
            command_digest: &cd,
            ledger: &ledger,
            historical_attempt,
        },
        proof,
    ) {
        return Err(ReservationStoreError::Denied);
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use admiralty_core::{
        AttemptId, PrincipalId, ProjectId, RootId, SchedulingState, TaskContract, TaskId,
        TaskLifecycleView,
    };

    #[test]
    fn exact_reservation_replay_can_use_historical_attempt_lineage() {
        let contract: TaskContract = serde_json::from_value(serde_json::json!({
            "schema_version": 1, "project_id": "project", "root_id": "root", "task_id": "task", "revision": 1,
            "title": "Task", "objective": "Reserve budget", "non_goals": [], "owned_paths": ["src/lib.rs"],
            "forbidden_paths": [], "read_first": [], "dependencies": [],
            "criteria": [{"id": "criterion", "text": "Bound reservation", "check_ids": ["check"]}],
            "checks": [{"id": "check", "program": "true", "args": [], "cwd": ".", "timeout_secs": 1}],
            "authority_ref": "grant:budget", "max_attempts": 2, "simulated": true
        })).unwrap();
        let attempt = AttemptId::new("attempt-a").unwrap();
        let owner = PrincipalId::new("worker-a").unwrap();
        let scope = ReservationScope {
            project_id: ProjectId::new("project").unwrap(),
            root_id: RootId::new("root").unwrap(),
            task_id: TaskId::new("task").unwrap(),
            attempt_id: attempt.clone(),
            owner: owner.clone(),
            incarnation: "boot-1".into(),
            contract_revision: 1,
            contract_digest: content_digest(&contract).unwrap(),
            authority_ref: "grant:budget".into(),
            resource_claims_digest: format!("sha256:{}", "a".repeat(64)),
            account_id: admiralty_runtime::reservations::AccountId::new("account").unwrap(),
            unit: admiralty_runtime::reservations::CurrencyUnit::new("USD", 2).unwrap(),
        };
        let task = TaskView {
            contract,
            state: SchedulingState::Accepted,
            attempt_id: None,
            owner: None,
            candidate_ref: None,
            lifecycle: Some(TaskLifecycleView {
                attempts: [(attempt, owner)].into_iter().collect(),
                ..Default::default()
            }),
        };
        let command = ReservationCommand {
            schema_version: 1,
            command_id: "reserve-a".into(),
            source: SourceIdentity {
                producer_id: "budget-agent".into(),
                incarnation: "boot-1".into(),
                sequence: "1".into(),
            },
            expected_ledger_revision: 0,
            expected_ledger_digest: format!("sha256:{}", "b".repeat(64)),
            launch_intent_digest: Some(format!("sha256:{}", "c".repeat(64))),
            event: ReservationEvent::Reserve {
                event_id: "reserve-a".into(),
                reservation_id: "reservation-a".into(),
                scope: scope.clone(),
                correlation_id: "correlation-a".into(),
                amount: admiralty_runtime::reservations::MinorUnits::new(1),
            },
        };
        assert_eq!(task_matches(&task, &command, &scope, false), None);
        assert_eq!(task_matches(&task, &command, &scope, true), Some(true));
    }
}
