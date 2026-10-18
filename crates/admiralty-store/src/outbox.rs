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
fn payload_json(value: &Value) -> Result<String> {
    // Bound work before allocating serialized bytes, including deeply nested or
    // huge in-memory values. Sort keys explicitly regardless of serde features.
    let mut pending = vec![(value, 0usize)];
    let mut nodes = 0usize;
    let mut text_bytes = 0usize;
    while let Some((value, depth)) = pending.pop() {
        nodes += 1;
        if nodes > MAX_PAYLOAD_BYTES || depth > MAX_PAYLOAD_DEPTH {
            return Err(OutboxError::Invalid(
                "payload exceeds structural bounds".into(),
            ));
        }
        match value {
            Value::String(text) => text_bytes = text_bytes.saturating_add(text.len()),
            Value::Array(items) => {
                if pending.len().saturating_add(items.len()) > MAX_PAYLOAD_BYTES {
                    return Err(OutboxError::Invalid(
                        "payload exceeds structural bounds".into(),
                    ));
                }
                pending.extend(items.iter().map(|value| (value, depth + 1)));
            }
            Value::Object(items) => {
                if pending.len().saturating_add(items.len()) > MAX_PAYLOAD_BYTES {
                    return Err(OutboxError::Invalid(
                        "payload exceeds structural bounds".into(),
                    ));
                }
                for (key, value) in items {
                    text_bytes = text_bytes.saturating_add(key.len());
                    pending.push((value, depth + 1));
                }
            }
            _ => {}
        }
        if text_bytes > MAX_PAYLOAD_BYTES {
            return Err(OutboxError::Invalid("payload exceeds 16 KiB".into()));
        }
    }
    fn canonical(value: &Value) -> Value {
        match value {
            Value::Array(items) => Value::Array(items.iter().map(canonical).collect()),
            Value::Object(items) => {
                let mut entries: Vec<_> = items.iter().collect();
                entries.sort_by(|left, right| left.0.cmp(right.0));
                Value::Object(
                    entries
                        .into_iter()
                        .map(|(key, value)| (key.clone(), canonical(value)))
                        .collect(),
                )
            }
            _ => value.clone(),
        }
    }
    let payload = serde_json::to_string(&canonical(value))?;
    if payload.len() > MAX_PAYLOAD_BYTES {
        return Err(OutboxError::Invalid("payload exceeds 16 KiB".into()));
    }
    Ok(payload)
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
    if !bounded_text(&intent.idempotency_key, 256) {
        return Err(OutboxError::Invalid(
            "idempotency key requires 1..256 printable bytes".into(),
        ));
    }
    let not_before = integer(intent.not_before)?;
    let payload = payload_json(&intent.payload)?;
    let digest = format!("sha256:{:x}", Sha256::digest(payload.as_bytes()));
    let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let existing: Option<(i64, String, String, i64)> = tx.query_row(
        "SELECT id,payload_digest,payload,not_before FROM outbox_intents WHERE idempotency_key=?1", [&intent.idempotency_key],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))).optional()?;
    if let Some((id, previous_digest, previous_payload, previous_due)) = existing {
        if previous_digest == digest && previous_payload == payload && previous_due == not_before {
            return Ok(id.to_string());
        }
        tx.execute("INSERT INTO outbox_integrity_incidents(idempotency_key,original_digest,conflicting_digest) VALUES(?1,?2,?3)", params![intent.idempotency_key, previous_digest, digest])?;
        tx.commit()?;
        return Err(OutboxError::IdentityCollision);
    }
    tx.execute("INSERT INTO outbox_intents(idempotency_key,payload,payload_digest,not_before) VALUES(?1,?2,?3,?4)", params![intent.idempotency_key, payload, digest, not_before])?;
    let id = tx.last_insert_rowid();
    tx.commit()?;
    Ok(id.to_string())
}

/// Observe a trusted clock, fence one due pending row, and never retry uncertainty.
pub fn claim_due(
    connection: &mut Connection,
    now: Option<u64>,
    owner: &str,
    lease_secs: u64,
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
    tx.execute("UPDATE outbox_intents SET state='unknown',unknown_reason='lease_expired' WHERE state='claimed' AND lease_deadline<=?1", [now])?;
    let next: Option<(i64, String, String, String, i64)> = tx.query_row(
        "SELECT id,idempotency_key,payload_digest,payload,fence FROM outbox_intents WHERE state='pending' AND not_before<=?1 ORDER BY id LIMIT 1", [now],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?))).optional()?;
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
