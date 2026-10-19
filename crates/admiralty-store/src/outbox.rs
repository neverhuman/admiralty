//! Durable outbox reservation prototype. These helpers never execute effects.
//!
//! Call on the dedicated writer connection, after independent caller grants.
//! Completion means an effect receipt was recorded, never task acceptance. A
//! crashed/expired claim becomes `unknown` and requires an external reconciliation
//! protocol; there is deliberately no API that retries it or makes it pending.
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::io::Write;

pub const MAX_PAYLOAD_BYTES: usize = 16 * 1024;
pub const MAX_LEASE_SECONDS: u64 = 86_400;
const MAX_PAYLOAD_DEPTH: usize = 64;
pub type Result<T> = std::result::Result<T, OutboxError>;

#[derive(Debug, thiserror::Error)]
pub enum OutboxError {
    #[error("outbox SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("outbox JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid outbox input: {0}")]
    Invalid(String),
    #[error("trusted logical clock is required")]
    UnknownClock,
    #[error("logical clock moved backwards")]
    ClockRollback,
    #[error("outbox deadline or fence exceeds supported integer range")]
    Overflow,
    #[error("idempotency identity has conflicting content; incident recorded")]
    IdentityCollision,
    #[error("outbox claim identity is stale or does not match")]
    StaleToken,
    #[error("outbox outcome is unknown and requires reconciliation")]
    ReconciliationRequired,
    #[error("completed outbox claim has a different receipt identity")]
    ReceiptConflict,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct OutboxIntent {
    pub idempotency_key: String,
    pub payload: Value,
    pub not_before: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct PreparedIntent {
    pub idempotency_key: String,
    pub payload: String,
    pub payload_digest: String,
    pub intent_digest: String,
    pub not_before: i64,
}

pub(crate) enum EnqueueResult {
    Inserted { id: i64 },
    ExistingExact { id: i64 },
    Collision,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClaimToken {
    pub intent_id: String,
    pub fence: String,
    pub owner: String,
    pub nonce: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Claim {
    pub id: String,
    pub token: ClaimToken,
    pub idempotency_key: String,
    pub payload_digest: String,
    pub payload: Value,
    pub lease_deadline: String,
}

pub fn initialize(connection: &Connection) -> Result<()> {
    // RAII also rolls back DDL if a partial/incompatible schema fails migration.
    // new_unchecked preserves the public &Connection signature; SQLite rejects
    // an accidental nested transaction before any DDL executes.
    let tx = Transaction::new_unchecked(connection, TransactionBehavior::Immediate)?;
    tx.execute_batch(include_str!("../migrations/0002_outbox.sql"))?;
    tx.commit()?;
    Ok(())
}

fn bounded_text(value: &str, maximum: usize) -> bool {
    !value.is_empty() && value.len() <= maximum && !value.chars().any(char::is_control)
}
fn integer(value: u64) -> Result<i64> {
    i64::try_from(value).map_err(|_| OutboxError::Overflow)
}
fn token_integer(value: &str) -> Result<i64> {
    value
        .parse::<i64>()
        .ok()
        .filter(|number| *number > 0 && number.to_string() == value)
        .ok_or(OutboxError::StaleToken)
}
fn logical_now(now: Option<u64>) -> Result<i64> {
    integer(now.ok_or(OutboxError::UnknownClock)?)
}
fn canonical_value(value: &Value, writer: &mut Vec<u8>) -> Result<()> {
    match value {
        Value::Array(values) => {
            writer
                .write_all(b"[")
                .map_err(|error| OutboxError::Invalid(error.to_string()))?;
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    writer
                        .write_all(b",")
                        .map_err(|error| OutboxError::Invalid(error.to_string()))?;
                }
                canonical_value(value, writer)?;
            }
            writer
                .write_all(b"]")
                .map_err(|error| OutboxError::Invalid(error.to_string()))?;
        }
        Value::Object(values) => {
            writer
                .write_all(b"{")
                .map_err(|error| OutboxError::Invalid(error.to_string()))?;
            let mut entries: Vec<_> = values.iter().collect();
            entries.sort_by(|left, right| left.0.cmp(right.0));
            for (index, (key, value)) in entries.into_iter().enumerate() {
                if index > 0 {
                    writer
                        .write_all(b",")
                        .map_err(|error| OutboxError::Invalid(error.to_string()))?;
                }
                serde_json::to_writer(&mut *writer, key)?;
                writer
                    .write_all(b":")
                    .map_err(|error| OutboxError::Invalid(error.to_string()))?;
                canonical_value(value, writer)?;
            }
            writer
                .write_all(b"}")
                .map_err(|error| OutboxError::Invalid(error.to_string()))?;
        }
        scalar => serde_json::to_writer(&mut *writer, scalar)?,
    }
    Ok(())
}

pub(crate) fn prepare_intent(intent: &OutboxIntent) -> Result<PreparedIntent> {
    if !bounded_text(&intent.idempotency_key, 256) {
        return Err(OutboxError::Invalid(
            "idempotency key requires 1..256 printable bytes".into(),
        ));
    }
    let not_before = integer(intent.not_before)?;
    // Bound work before serialization, including deep and wide in-memory values.
    let mut pending = vec![(&intent.payload, 0usize)];
    let mut nodes = 0usize;
    while let Some((value, depth)) = pending.pop() {
        nodes = nodes.checked_add(1).ok_or(OutboxError::Overflow)?;
        if nodes > MAX_PAYLOAD_BYTES || depth > MAX_PAYLOAD_DEPTH {
            return Err(OutboxError::Invalid(
                "payload exceeds structural bounds".into(),
            ));
        }
        match value {
            Value::Array(items) => {
                if pending
                    .len()
                    .checked_add(items.len())
                    .is_none_or(|size| size > MAX_PAYLOAD_BYTES)
                {
                    return Err(OutboxError::Invalid(
                        "payload exceeds structural bounds".into(),
                    ));
                }
                pending.extend(items.iter().map(|item| (item, depth + 1)));
            }
            Value::Object(items) => {
                if pending
                    .len()
                    .checked_add(items.len())
                    .is_none_or(|size| size > MAX_PAYLOAD_BYTES)
                {
                    return Err(OutboxError::Invalid(
                        "payload exceeds structural bounds".into(),
                    ));
                }
                pending.extend(items.values().map(|item| (item, depth + 1)));
            }
            _ => {}
        }
    }
    let size = admiralty_core::limits::bounded_json_size(&intent.payload, MAX_PAYLOAD_BYTES)
        .map_err(|_| OutboxError::Invalid("payload exceeds 16 KiB".into()))?;
    let mut writer = Vec::with_capacity(size);
    canonical_value(&intent.payload, &mut writer)?;
    let payload =
        String::from_utf8(writer).map_err(|error| OutboxError::Invalid(error.to_string()))?;
    let payload_digest = format!("sha256:{:x}", Sha256::digest(payload.as_bytes()));
    let intent_digest = prepared_intent_digest(&intent.idempotency_key, &payload, not_before);
    Ok(PreparedIntent {
        idempotency_key: intent.idempotency_key.clone(),
        payload,
        payload_digest,
        intent_digest,
        not_before,
    })
}

pub(crate) fn prepared_intent_digest(key: &str, payload: &str, not_before: i64) -> String {
    let mut identity = Sha256::new();
    identity.update(key.as_bytes());
    identity.update([0]);
    identity.update(payload.as_bytes());
    identity.update([0]);
    identity.update(not_before.to_be_bytes());
    format!("sha256:{:x}", identity.finalize())
}

fn observe_clock(tx: &Transaction<'_>, now: i64) -> Result<()> {
    let previous: Option<i64> = tx
        .query_row(
            "SELECT last_observed FROM outbox_clock WHERE singleton=1",
            [],
            |row| row.get(0),
        )
        .optional()?;
    if previous.is_some_and(|previous| now < previous) {
        return Err(OutboxError::ClockRollback);
    }
    tx.execute("INSERT INTO outbox_clock(singleton,last_observed) VALUES(1,?1) ON CONFLICT(singleton) DO UPDATE SET last_observed=excluded.last_observed", [now])?;
    Ok(())
}

/// Duplicate identity/content/schedule returns its original decimal ID.
pub fn enqueue(connection: &mut Connection, intent: OutboxIntent) -> Result<String> {
    let prepared = prepare_intent(&intent)?;
    let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let result = enqueue_prepared_in_transaction(&tx, &prepared)?;
    let id = match result {
        EnqueueResult::Inserted { id } | EnqueueResult::ExistingExact { id } => id,
        EnqueueResult::Collision => {
            tx.commit()?;
            return Err(OutboxError::IdentityCollision);
        }
    };
    tx.commit()?;
    Ok(id.to_string())
}

pub(crate) fn enqueue_prepared_in_transaction(
    tx: &Transaction<'_>,
    intent: &PreparedIntent,
) -> Result<EnqueueResult> {
    let existing: Option<(i64, String, String, i64)> = tx.query_row(
        "SELECT id,payload_digest,payload,not_before FROM outbox_intents WHERE idempotency_key=?1",
        [&intent.idempotency_key],
        |row| Ok((row.get(0)?, super::stored_text(row,1,71)?, super::stored_text(row,2,MAX_PAYLOAD_BYTES)?, row.get(3)?)),
    ).optional()?;
    if let Some((id, previous_digest, previous_payload, previous_due)) = existing {
        if previous_digest == intent.payload_digest
            && previous_payload == intent.payload
            && previous_due == intent.not_before
        {
            return Ok(EnqueueResult::ExistingExact { id });
        }
        tx.execute("INSERT INTO outbox_integrity_incidents(idempotency_key,original_digest,conflicting_digest) VALUES(?1,?2,?3)", params![intent.idempotency_key, previous_digest, intent.payload_digest])?;
        return Ok(EnqueueResult::Collision);
    }
    tx.execute("INSERT INTO outbox_intents(idempotency_key,payload,payload_digest,not_before) VALUES(?1,?2,?3,?4)", params![intent.idempotency_key, intent.payload, intent.payload_digest, intent.not_before])?;
    Ok(EnqueueResult::Inserted {
        id: tx.last_insert_rowid(),
    })
}

/// Observe a trusted clock, fence one due pending row, and never retry uncertainty.
pub fn claim_due(
    connection: &mut Connection,
    now: Option<u64>,
    owner: &str,
    lease_secs: u64,
) -> Result<Option<Claim>> {
    claim_selected(connection, now, owner, lease_secs, None)
}

/// Claim only the exact independently authorized effect key. Expiry observation
/// also applies only to that key; foreign pending/claimed rows remain untouched.
/// The trusted clock watermark is still global. This is not an authority grant.
pub fn claim_key_due(
    connection: &mut Connection,
    now: Option<u64>,
    owner: &str,
    lease_secs: u64,
    idempotency_key: &str,
) -> Result<Option<Claim>> {
    validate_claim_key(idempotency_key)?;
    claim_selected(connection, now, owner, lease_secs, Some(idempotency_key))
}

pub(crate) fn validate_claim_key(key: &str) -> Result<()> {
    if !bounded_text(key, 256) {
        return Err(OutboxError::Invalid(
            "idempotency key requires 1..256 printable bytes".into(),
        ));
    }
    Ok(())
}

type ClaimRow = (i64, String, String, String, i64);

fn claim_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ClaimRow> {
    Ok((
        row.get(0)?,
        super::stored_text(row, 1, 256)?,
        super::stored_text(row, 2, 71)?,
        super::stored_text(row, 3, MAX_PAYLOAD_BYTES)?,
        row.get(4)?,
    ))
}

fn claim_selected(
    connection: &mut Connection,
    now: Option<u64>,
    owner: &str,
    lease_secs: u64,
    key: Option<&str>,
) -> Result<Option<Claim>> {
    let now = logical_now(now)?;
    if !bounded_text(owner, 128) || !(1..=MAX_LEASE_SECONDS).contains(&lease_secs) {
        return Err(OutboxError::Invalid(
            "owner or lease duration is outside bounds".into(),
        ));
    }
    let deadline = now
        .checked_add(integer(lease_secs)?)
        .ok_or(OutboxError::Overflow)?;
    let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    observe_clock(&tx, now)?;
    let next = if let Some(key) = key {
        tx.execute("UPDATE outbox_intents SET state='unknown',unknown_reason='lease_expired' WHERE idempotency_key=?1 AND state='claimed' AND lease_deadline<=?2", params![key, now])?;
        tx.query_row(
            "SELECT id,idempotency_key,payload_digest,payload,fence FROM outbox_intents WHERE idempotency_key=?1 AND state='pending' AND not_before<=?2",
            params![key, now], claim_row,
        ).optional()?
    } else {
        tx.execute("UPDATE outbox_intents SET state='unknown',unknown_reason='lease_expired' WHERE state='claimed' AND lease_deadline<=?1", [now])?;
        tx.query_row(
            "SELECT id,idempotency_key,payload_digest,payload,fence FROM outbox_intents WHERE state='pending' AND not_before<=?1 ORDER BY id LIMIT 1",
            [now], claim_row,
        ).optional()?
    };
    let Some((id, idempotency_key, payload_digest, payload, previous_fence)) = next else {
        tx.commit()?;
        return Ok(None);
    };
    let computed = format!("sha256:{:x}", Sha256::digest(payload.as_bytes()));
    if computed != payload_digest {
        return Err(OutboxError::Invalid(
            "stored payload digest mismatch".into(),
        ));
    }
    let payload = serde_json::from_str(&payload)?;
    let fence = previous_fence.checked_add(1).ok_or(OutboxError::Overflow)?;
    let nonce = uuid::Uuid::new_v4().to_string();
    let changed = tx.execute("UPDATE outbox_intents SET state='claimed',fence=?1,claim_nonce=?2,claim_owner=?3,lease_deadline=?4 WHERE id=?5 AND state='pending' AND fence=?6",
        params![fence, nonce, owner, deadline, id, previous_fence])?;
    if changed != 1 {
        return Err(OutboxError::StaleToken);
    }
    tx.commit()?;
    Ok(Some(Claim {
        id: id.to_string(),
        token: ClaimToken {
            intent_id: id.to_string(),
            fence: fence.to_string(),
            owner: owner.into(),
            nonce,
        },
        idempotency_key,
        payload_digest,
        payload,
        lease_deadline: deadline.to_string(),
    }))
}

fn validate_token(token: &ClaimToken) -> Result<(i64, i64)> {
    if !bounded_text(&token.owner, 128) || uuid::Uuid::parse_str(&token.nonce).is_err() {
        return Err(OutboxError::StaleToken);
    }
    Ok((
        token_integer(&token.intent_id)?,
        token_integer(&token.fence)?,
    ))
}

/// Exact receipt identity is idempotent. Expired claims cannot be acknowledged.
pub fn complete(
    connection: &mut Connection,
    token: &ClaimToken,
    receipt_ref: &str,
    now: Option<u64>,
) -> Result<()> {
    let now = logical_now(now)?;
    let (id, fence) = validate_token(token)?;
    if !bounded_text(receipt_ref, 1024) {
        return Err(OutboxError::Invalid(
            "receipt reference requires 1..1024 printable bytes".into(),
        ));
    }
    let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    observe_clock(&tx, now)?;
    let row: Option<(String, i64, Option<String>)> = tx.query_row(
        "SELECT state,lease_deadline,receipt_ref FROM outbox_intents WHERE id=?1 AND fence=?2 AND claim_owner=?3 AND claim_nonce=?4", params![id, fence, token.owner, token.nonce],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).optional()?;
    let Some((state, deadline, receipt)) = row else {
        return Err(OutboxError::StaleToken);
    };
    if state == "completed" {
        if receipt.as_deref() != Some(receipt_ref) {
            return Err(OutboxError::ReceiptConflict);
        }
        tx.commit()?;
        return Ok(());
    }
    if state == "unknown" {
        return Err(OutboxError::ReconciliationRequired);
    }
    if state != "claimed" {
        return Err(OutboxError::StaleToken);
    }
    if now >= deadline {
        tx.execute("UPDATE outbox_intents SET state='unknown',unknown_reason='lease_expired' WHERE id=?1 AND state='claimed' AND fence=?2 AND claim_nonce=?3", params![id, fence, token.nonce])?;
        tx.commit()?;
        return Err(OutboxError::ReconciliationRequired);
    }
    let changed = tx.execute("UPDATE outbox_intents SET state='completed',receipt_ref=?1 WHERE id=?2 AND state='claimed' AND fence=?3 AND claim_owner=?4 AND claim_nonce=?5 AND lease_deadline>?6",
        params![receipt_ref, id, fence, token.owner, token.nonce, now])?;
    if changed != 1 {
        return Err(OutboxError::StaleToken);
    }
    tx.commit()?;
    Ok(())
}

/// Preserve ambiguity immediately; this never releases the row for redispatch.
pub fn fail_unknown(connection: &mut Connection, token: &ClaimToken, reason: &str) -> Result<()> {
    let (id, fence) = validate_token(token)?;
    if !bounded_text(reason, 1024) {
        return Err(OutboxError::Invalid(
            "uncertainty reason requires 1..1024 printable bytes".into(),
        ));
    }
    let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let changed = tx.execute("UPDATE outbox_intents SET state='unknown',unknown_reason=?1 WHERE id=?2 AND state='claimed' AND fence=?3 AND claim_owner=?4 AND claim_nonce=?5",
        params![reason, id, fence, token.owner, token.nonce])?;
    if changed != 1 {
        return Err(OutboxError::StaleToken);
    }
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::{
        sync::{Arc, Barrier},
        time::Duration,
    };

    fn database() -> Connection {
        let connection = Connection::open_in_memory().expect("memory database");
        initialize(&connection).expect("outbox tables");
        connection
    }
    fn intent(key: &str, due: u64) -> OutboxIntent {
        OutboxIntent {
            idempotency_key: key.into(),
            payload: json!({"kind":"reference_only","artifact":"sha256:example"}),
            not_before: due,
        }
    }
    fn state(connection: &Connection, id: &str) -> String {
        connection
            .query_row(
                "SELECT state FROM outbox_intents WHERE id=?1",
                [id],
                |row| row.get(0),
            )
            .expect("row state")
    }
    fn row_count(connection: &Connection) -> i64 {
        connection
            .query_row("SELECT COUNT(*) FROM outbox_intents", [], |row| row.get(0))
            .expect("row count")
    }

    fn retained_row(connection: &Connection, key: &str) -> String {
        connection.query_row(
            "SELECT json_object('id',id,'key',idempotency_key,'payload',payload,'digest',payload_digest,'due',not_before,'state',state,'fence',fence,'nonce',claim_nonce,'owner',claim_owner,'deadline',lease_deadline,'receipt',receipt_ref,'unknown',unknown_reason) FROM outbox_intents WHERE idempotency_key=?1",
            [key], |row| row.get(0),
        ).expect("retained row")
    }

    #[test]
    fn exact_key_claim_preserves_earlier_foreign_pending_and_expired_claimed_rows() {
        let mut connection = database();
        enqueue(&mut connection, intent("other-project:expired", 0)).expect("expired fixture");
        claim_key_due(
            &mut connection,
            Some(0),
            "other",
            1,
            "other-project:expired",
        )
        .expect("first claim")
        .expect("claimed");
        enqueue(&mut connection, intent("other-project:pending", 0)).expect("earlier due fixture");
        let id = enqueue(&mut connection, intent("our-project:launch", 2)).expect("our effect");
        let foreign_expired = retained_row(&connection, "other-project:expired");
        let foreign_pending = retained_row(&connection, "other-project:pending");
        let claim = claim_key_due(&mut connection, Some(2), "ours", 5, "our-project:launch")
            .expect("exact selection")
            .expect("our claim");
        assert_eq!(claim.id, id);
        assert_eq!(claim.idempotency_key, "our-project:launch");
        assert_eq!(claim.token.fence, "1");
        assert_eq!(claim.lease_deadline, "7");
        assert_eq!(
            retained_row(&connection, "other-project:expired"),
            foreign_expired
        );
        assert_eq!(
            retained_row(&connection, "other-project:pending"),
            foreign_pending
        );
        // A repeated claim never supplies another lease/token, even to the same owner.
        assert!(
            claim_key_due(&mut connection, Some(2), "ours", 5, "our-project:launch")
                .expect("held claim")
                .is_none()
        );
        let mut stale = claim.token.clone();
        stale.fence = "2".into();
        assert!(matches!(
            complete(&mut connection, &stale, "receipt", Some(2)),
            Err(OutboxError::StaleToken)
        ));
        complete(&mut connection, &claim.token, "receipt", Some(3)).expect("real token completes");
        complete(&mut connection, &claim.token, "receipt", Some(3)).expect("same receipt replay");
        assert!(
            claim_key_due(&mut connection, Some(4), "ours", 5, "our-project:launch")
                .expect("completed no redispatch")
                .is_none()
        );
        assert_eq!(
            retained_row(&connection, "other-project:expired"),
            foreign_expired
        );
        assert_eq!(
            retained_row(&connection, "other-project:pending"),
            foreign_pending
        );
    }

    #[test]
    fn exact_key_not_due_missing_and_expired_unknown_never_select_another_effect() {
        let mut connection = database();
        enqueue(&mut connection, intent("foreign-ready", 0)).expect("foreign");
        let future = enqueue(&mut connection, intent("ours-future", 20)).expect("ours");
        let original = retained_row(&connection, "foreign-ready");
        assert!(
            claim_key_due(&mut connection, Some(19), "ours", 2, "ours-future")
                .expect("not due")
                .is_none()
        );
        assert!(
            claim_key_due(&mut connection, Some(19), "ours", 2, "missing")
                .expect("missing")
                .is_none()
        );
        // SQL metacharacters are literal keys, never a selector expression.
        assert!(
            claim_key_due(&mut connection, Some(19), "ours", 2, "x' OR 1=1 --")
                .expect("literal key")
                .is_none()
        );
        let claim = claim_key_due(&mut connection, Some(20), "ours", 2, "ours-future")
            .expect("due")
            .expect("claim");
        assert_eq!(claim.id, future);
        assert!(
            claim_key_due(&mut connection, Some(22), "ours", 2, "ours-future")
                .expect("exact expiry")
                .is_none()
        );
        assert_eq!(state(&connection, &future), "unknown");
        assert!(
            claim_key_due(&mut connection, Some(30), "new-owner", 2, "ours-future")
                .expect("unknown retained")
                .is_none()
        );
        assert!(matches!(
            complete(&mut connection, &claim.token, "late", Some(30)),
            Err(OutboxError::ReconciliationRequired)
        ));
        assert_eq!(retained_row(&connection, "foreign-ready"), original);
    }

    #[test]
    fn invalid_exact_keys_clocks_and_durations_have_no_clock_or_row_effects() {
        let mut connection = database();
        enqueue(&mut connection, intent("ours", 0)).expect("ours");
        let before = retained_row(&connection, "ours");
        for key in [
            String::new(),
            "x".repeat(257),
            "key\nforged".into(),
            "key\0forged".into(),
        ] {
            assert!(matches!(
                claim_key_due(&mut connection, Some(100), "ours", 1, &key),
                Err(OutboxError::Invalid(_))
            ));
        }
        for (now, owner, lease) in [
            (None, "ours", 1),
            (Some(100), "", 1),
            (Some(100), "ours", 0),
            (Some(i64::MAX as u64), "ours", 1),
        ] {
            assert!(claim_key_due(&mut connection, now, owner, lease, "ours").is_err());
        }
        assert_eq!(retained_row(&connection, "ours"), before);
        let clocks: i64 = connection
            .query_row("SELECT count(*) FROM outbox_clock", [], |r| r.get(0))
            .expect("clock rows");
        assert_eq!(clocks, 0);
        assert!(
            claim_key_due(&mut connection, Some(5), "ours", 1, "missing")
                .expect("trusted clock")
                .is_none()
        );
        assert!(matches!(
            claim_key_due(&mut connection, Some(4), "ours", 1, "ours"),
            Err(OutboxError::ClockRollback)
        ));
        assert_eq!(retained_row(&connection, "ours"), before);
    }

    #[test]
    fn exact_claim_and_expiry_sql_failures_rollback_global_clock_and_local_state() {
        let mut connection = database();
        let id = enqueue(&mut connection, intent("ours", 0)).expect("ours");
        let before = retained_row(&connection, "ours");
        connection.execute_batch("CREATE TRIGGER reject_exact_claim BEFORE UPDATE ON outbox_intents WHEN NEW.idempotency_key='ours' AND NEW.state='claimed' BEGIN SELECT RAISE(ABORT,'owned claim fault'); END;").expect("own trigger");
        assert!(matches!(
            claim_key_due(&mut connection, Some(8), "ours", 2, "ours"),
            Err(OutboxError::Sqlite(_))
        ));
        assert_eq!(retained_row(&connection, "ours"), before);
        let clocks: i64 = connection
            .query_row("SELECT count(*) FROM outbox_clock", [], |r| r.get(0))
            .expect("clock rollback");
        assert_eq!(clocks, 0);
        connection
            .execute_batch("DROP TRIGGER reject_exact_claim")
            .expect("remove own fault");
        claim_key_due(&mut connection, Some(0), "ours", 2, "ours")
            .expect("original clock still valid")
            .expect("claim");
        let claimed = retained_row(&connection, "ours");
        connection.execute_batch("CREATE TRIGGER reject_exact_expiry BEFORE UPDATE ON outbox_intents WHEN NEW.idempotency_key='ours' AND NEW.state='unknown' BEGIN SELECT RAISE(ABORT,'owned expiry fault'); END;").expect("own expiry fault");
        assert!(matches!(
            claim_key_due(&mut connection, Some(2), "ours", 2, "ours"),
            Err(OutboxError::Sqlite(_))
        ));
        assert_eq!(retained_row(&connection, "ours"), claimed);
        let clock: i64 = connection
            .query_row(
                "SELECT last_observed FROM outbox_clock WHERE singleton=1",
                [],
                |r| r.get(0),
            )
            .expect("clock after fault");
        assert_eq!(clock, 0);
        connection
            .execute_batch("DROP TRIGGER reject_exact_expiry")
            .expect("remove own expiry fault");
        assert!(
            claim_key_due(&mut connection, Some(2), "ours", 2, "ours")
                .expect("observe expiry")
                .is_none()
        );
        assert_eq!(state(&connection, &id), "unknown");
    }

    #[test]
    fn duplicate_identity_returns_original_and_collision_is_durable() {
        let mut connection = database();
        let id = enqueue(&mut connection, intent("effect-1", 0)).expect("enqueue");
        assert_eq!(
            enqueue(&mut connection, intent("effect-1", 0)).expect("duplicate"),
            id
        );
        let mut conflicting = intent("effect-1", 0);
        conflicting.payload = json!({"different":true});
        assert!(matches!(
            enqueue(&mut connection, conflicting),
            Err(OutboxError::IdentityCollision)
        ));
        assert!(matches!(
            enqueue(&mut connection, intent("effect-1", 1)),
            Err(OutboxError::IdentityCollision)
        ));
        assert_eq!(row_count(&connection), 1);
        let incidents: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM outbox_integrity_incidents",
                [],
                |row| row.get(0),
            )
            .expect("incidents");
        assert_eq!(incidents, 2);
        assert!(
            connection
                .execute(
                    "UPDATE outbox_intents SET payload='null' WHERE id=?1",
                    [&id]
                )
                .is_err()
        );
        assert_eq!(state(&connection, &id), "pending");
    }

    #[test]
    fn failed_insert_and_claim_transactions_rollback_without_ghost_state() {
        let mut connection = database();
        connection.execute_batch("CREATE TRIGGER reject_outbox BEFORE INSERT ON outbox_intents BEGIN SELECT RAISE(ABORT,'injected insert failure'); END;").expect("fault trigger");
        assert!(enqueue(&mut connection, intent("effect-1", 0)).is_err());
        assert_eq!(row_count(&connection), 0);
        connection
            .execute_batch("DROP TRIGGER reject_outbox;")
            .expect("remove fault");
        let id = enqueue(&mut connection, intent("effect-1", 0)).expect("retry unpersisted input");
        connection.execute_batch("CREATE TRIGGER reject_claim BEFORE UPDATE OF state ON outbox_intents WHEN NEW.state='claimed' BEGIN SELECT RAISE(ABORT,'injected claim failure'); END;").expect("claim fault");
        assert!(claim_due(&mut connection, Some(9), "worker", 10).is_err());
        assert_eq!(state(&connection, &id), "pending");
        let clocks: i64 = connection
            .query_row("SELECT COUNT(*) FROM outbox_clock", [], |row| row.get(0))
            .expect("clock rows");
        assert_eq!(clocks, 0);
        connection
            .execute_batch("DROP TRIGGER reject_claim;")
            .expect("remove fault");
        assert!(
            claim_due(&mut connection, Some(0), "worker", 10)
                .expect("clock0 valid after rollback")
                .is_some()
        );
    }

    #[test]
    fn unknown_clock_rollback_and_deadline_overflow_do_not_mutate_intents() {
        let mut connection = database();
        let id = enqueue(&mut connection, intent("effect-1", 5)).expect("enqueue");
        assert!(matches!(
            claim_due(&mut connection, None, "worker", 10),
            Err(OutboxError::UnknownClock)
        ));
        assert!(
            claim_due(&mut connection, Some(4), "worker", 10)
                .expect("not yet due")
                .is_none()
        );
        assert!(matches!(
            claim_due(&mut connection, Some(3), "worker", 10),
            Err(OutboxError::ClockRollback)
        ));
        assert!(matches!(
            claim_due(&mut connection, Some(i64::MAX as u64), "worker", 1),
            Err(OutboxError::Overflow)
        ));
        assert_eq!(state(&connection, &id), "pending");
        let claim = claim_due(&mut connection, Some(5), "worker", 10)
            .expect("due")
            .expect("claim");
        assert!(matches!(
            complete(&mut connection, &claim.token, "receipt:1", None),
            Err(OutboxError::UnknownClock)
        ));
        assert!(matches!(
            complete(&mut connection, &claim.token, "receipt:1", Some(4)),
            Err(OutboxError::ClockRollback)
        ));
        assert_eq!(state(&connection, &id), "claimed");
        complete(&mut connection, &claim.token, "receipt:1", Some(6)).expect("valid completion");
    }

    #[test]
    fn exact_token_and_receipt_are_required_and_duplicate_receipt_is_idempotent() {
        let mut connection = database();
        enqueue(&mut connection, intent("effect-1", 0)).expect("enqueue");
        let claim = claim_due(&mut connection, Some(0), "worker", 10)
            .expect("reserve")
            .expect("claim");
        for changed in ["owner", "fence", "id", "nonce"] {
            let mut token = claim.token.clone();
            match changed {
                "owner" => token.owner = "other".into(),
                "fence" => token.fence = "2".into(),
                "id" => token.intent_id = "2".into(),
                _ => token.nonce = uuid::Uuid::new_v4().to_string(),
            }
            assert!(matches!(
                complete(&mut connection, &token, "receipt:1", Some(1)),
                Err(OutboxError::StaleToken)
            ));
        }
        assert!(complete(&mut connection, &claim.token, "", Some(1)).is_err());
        complete(&mut connection, &claim.token, "receipt:1", Some(1)).expect("complete");
        complete(&mut connection, &claim.token, "receipt:1", Some(20))
            .expect("same terminal receipt remains idempotent");
        assert!(matches!(
            complete(&mut connection, &claim.token, "receipt:other", Some(21)),
            Err(OutboxError::ReceiptConflict)
        ));
        assert!(
            claim_due(&mut connection, Some(20), "other", 10)
                .expect("no retry completed")
                .is_none()
        );
    }

    #[test]
    fn expired_claim_becomes_unknown_and_is_never_automatically_retried() {
        let mut connection = database();
        let id = enqueue(&mut connection, intent("effect-1", 0)).expect("enqueue");
        let claim = claim_due(&mut connection, Some(100), "worker", 5)
            .expect("reserve")
            .expect("claim");
        assert!(matches!(
            complete(&mut connection, &claim.token, "late-receipt", Some(105)),
            Err(OutboxError::ReconciliationRequired)
        ));
        assert_eq!(state(&connection, &id), "unknown");
        assert!(
            claim_due(&mut connection, Some(1000), "other", 10)
                .expect("reconciliation only")
                .is_none()
        );
        assert!(matches!(
            complete(&mut connection, &claim.token, "late-receipt", Some(1000)),
            Err(OutboxError::ReconciliationRequired)
        ));
        assert!(
            connection
                .execute(
                    "UPDATE outbox_intents SET state='pending' WHERE id=?1",
                    [&id]
                )
                .is_err()
        );
    }

    #[test]
    fn explicit_unknown_preserves_ambiguity_and_requires_current_token() {
        let mut connection = database();
        let id = enqueue(&mut connection, intent("effect-1", 0)).expect("enqueue");
        let claim = claim_due(&mut connection, Some(0), "worker", 10)
            .expect("reserve")
            .expect("claim");
        let mut stale = claim.token.clone();
        stale.fence = "2".into();
        assert!(matches!(
            fail_unknown(&mut connection, &stale, "publication ambiguous"),
            Err(OutboxError::StaleToken)
        ));
        fail_unknown(&mut connection, &claim.token, "publication ambiguous")
            .expect("record uncertainty");
        assert_eq!(state(&connection, &id), "unknown");
        assert!(
            claim_due(&mut connection, Some(100), "other", 10)
                .expect("no retry unknown")
                .is_none()
        );
    }

    #[test]
    fn restart_preserves_pending_and_does_not_skip_due_rows_or_retry_expired_ones() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("outbox.sqlite");
        let mut connection = Connection::open(&path).expect("database");
        initialize(&connection).expect("schema");
        let future = enqueue(&mut connection, intent("future", 50)).expect("future enqueue");
        let uncertain = enqueue(&mut connection, intent("uncertain", 0)).expect("due enqueue");
        let pending = enqueue(&mut connection, intent("still-pending", 0)).expect("next enqueue");
        let claim = claim_due(&mut connection, Some(0), "worker", 5)
            .expect("due FIFO")
            .expect("claim");
        assert_eq!(claim.id, uncertain);
        drop(connection);
        let mut reopened = Connection::open(&path).expect("reopen");
        initialize(&reopened).expect("idempotent schema initialization");
        assert_eq!(state(&reopened, &uncertain), "claimed"); // initialization never dispatches or reads a hidden clock
        let next = claim_due(&mut reopened, Some(6), "restarted", 10)
            .expect("recovery observation")
            .expect("next due");
        assert_eq!(next.id, pending);
        assert_eq!(state(&reopened, &uncertain), "unknown");
        complete(&mut reopened, &next.token, "receipt:pending", Some(7)).expect("complete next");
        assert!(
            claim_due(&mut reopened, Some(49), "restarted", 10)
                .expect("future not due")
                .is_none()
        );
        assert_eq!(
            claim_due(&mut reopened, Some(50), "restarted", 10)
                .expect("future due")
                .expect("future claim")
                .id,
            future
        );
    }

    #[test]
    fn two_connections_cannot_claim_the_same_intent() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("outbox.sqlite");
        let mut connection = Connection::open(&path).expect("database");
        initialize(&connection).expect("schema");
        enqueue(&mut connection, intent("effect-1", 0)).expect("enqueue");
        drop(connection);
        let barrier = Arc::new(Barrier::new(2));
        let threads: Vec<_> = (0..2)
            .map(|index| {
                let path = path.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let mut connection = Connection::open(path).expect("worker connection");
                    connection
                        .busy_timeout(Duration::from_secs(5))
                        .expect("bounded contention wait");
                    barrier.wait();
                    claim_due(&mut connection, Some(0), &format!("worker-{index}"), 10)
                        .expect("serialized claim")
                })
            })
            .collect();
        let claims: Vec<_> = threads
            .into_iter()
            .filter_map(|worker| worker.join().expect("worker"))
            .collect();
        assert_eq!(claims.len(), 1);
    }

    #[test]
    fn serialized_payload_is_bounded_and_claim_ids_remain_decimal_strings() {
        let mut connection = database();
        let mut oversized = intent("large", 0);
        oversized.payload = json!("x".repeat(MAX_PAYLOAD_BYTES));
        assert!(enqueue(&mut connection, oversized).is_err());
        assert_eq!(row_count(&connection), 0);
        let mut exact = intent("exact", 0);
        exact.payload = json!("x".repeat(MAX_PAYLOAD_BYTES - 2));
        let id = enqueue(&mut connection, exact).expect("exact serialized byte boundary");
        let claim = claim_due(&mut connection, Some(0), "worker", 10)
            .expect("reserve")
            .expect("claim");
        assert_eq!(claim.id, id);
        assert_eq!(claim.token.fence, "1");
        assert_eq!(claim.lease_deadline, "10");
    }

    #[test]
    fn deep_payloads_are_rejected_before_serialization_and_large_ids_stay_exact() {
        let mut connection = database();
        let mut payload = Value::Null;
        for _ in 0..=MAX_PAYLOAD_DEPTH {
            payload = json!([payload]);
        }
        let mut deep = intent("deep", 0);
        deep.payload = payload;
        assert!(enqueue(&mut connection, deep).is_err());
        connection
            .execute(
                "INSERT INTO sqlite_sequence(name,seq) VALUES('outbox_intents',9007199254740992)",
                [],
            )
            .expect("large ID fixture");
        let id = enqueue(&mut connection, intent("large-id", 0)).expect("enqueue");
        assert_eq!(id, "9007199254740993");
        let claim = claim_due(&mut connection, Some(0), "worker", 10)
            .expect("reserve")
            .expect("claim");
        assert_eq!(claim.token.intent_id, "9007199254740993");
    }

    #[test]
    fn failed_migration_rolls_back_ddl_and_releases_the_transaction() {
        let connection = Connection::open_in_memory().expect("database");
        connection
            .execute_batch("CREATE TABLE outbox_intents(incompatible INTEGER);")
            .expect("partial incompatible schema");
        assert!(initialize(&connection).is_err());
        assert!(
            connection.is_autocommit(),
            "failed initialization must release its transaction"
        );
        let partial: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name='outbox_clock'",
                [],
                |row| row.get(0),
            )
            .expect("partial schema check");
        assert_eq!(partial, 0, "preceding DDL was rolled back");
        connection
            .execute_batch("DROP TABLE outbox_intents;")
            .expect("remove fixture fault");
        initialize(&connection).expect("connection remains usable after failure");
    }

    #[test]
    fn writer_contention_does_not_advance_clock_or_partially_claim() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("outbox.sqlite");
        let mut first = Connection::open(&path).expect("first connection");
        initialize(&first).expect("schema");
        let id = enqueue(&mut first, intent("contended", 0)).expect("intent");
        let mut second = Connection::open(&path).expect("second connection");
        second
            .busy_timeout(Duration::from_millis(20))
            .expect("bounded busy wait");
        let reservation = first
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .expect("hold writer lock");
        assert!(matches!(
            claim_due(&mut second, Some(10), "second", 5),
            Err(OutboxError::Sqlite(_))
        ));
        reservation.commit().expect("release writer lock");
        assert_eq!(state(&second, &id), "pending");
        let clocks: i64 = second
            .query_row("SELECT COUNT(*) FROM outbox_clock", [], |row| row.get(0))
            .expect("no partial clock");
        assert_eq!(clocks, 0);
        assert!(
            claim_due(&mut second, Some(0), "second", 5)
                .expect("retry at original valid clock")
                .is_some()
        );
    }
}
