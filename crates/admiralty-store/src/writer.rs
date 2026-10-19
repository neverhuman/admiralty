//! Authoritative writer diagnostics, WAL health inspection, collision quarantine
//! queries, and `P03` writer entrypoints (`ingest_observation`, `commit_domain_event`,
//! `reserve_and_enqueue`).
//!
//! All state mutations route through `Store`'s single-owner serialized writer queue;
//! this module never opens a competing writer connection or silently replaces
//! immutable source facts.

use crate::{
    AppendReceipt, ReservedClaimReceipt, Result, Store, StoreError, admission_reservations, outbox,
    stored_text,
};
use admiralty_core::{DomainEvent, SourceIdentity};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

pub const MAX_QUARANTINE_PAGE: usize = 256;
pub const DEFAULT_WRITER_QUEUE_CAPACITY: usize = 64;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SqliteEngineDiagnostics {
    pub version_number: i32,
    pub version_string: String,
    pub source_id: String,
    pub wal_reset_fix_qualified: bool,
    pub journal_mode: String,
    pub synchronous_level: i64,
    pub foreign_keys_enabled: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WalHealthReport {
    pub reader_blocked: bool,
    pub log_frames: i64,
    pub checkpointed_frames: i64,
    pub uncheckpointed_frames: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WriterHealthMetrics {
    pub queue_capacity: usize,
    pub current_queue_depth: usize,
    pub max_observed_queue_depth: usize,
    pub completed_turns: u64,
    pub last_turn_latency_micros: u64,
    pub max_turn_latency_micros: u64,
}

impl Default for WriterHealthMetrics {
    fn default() -> Self {
        Self::new(DEFAULT_WRITER_QUEUE_CAPACITY)
    }
}

impl WriterHealthMetrics {
    pub fn new(queue_capacity: usize) -> Self {
        Self {
            queue_capacity: queue_capacity.max(1),
            current_queue_depth: 0,
            max_observed_queue_depth: 0,
            completed_turns: 0,
            last_turn_latency_micros: 0,
            max_turn_latency_micros: 0,
        }
    }

    pub fn record_turn(
        &mut self,
        observed_queue_depth: usize,
        turn_latency_micros: u64,
    ) -> Result<()> {
        if observed_queue_depth > self.queue_capacity {
            return Err(StoreError::Invalid(
                "writer queue depth exceeds bounded capacity".into(),
            ));
        }
        self.current_queue_depth = observed_queue_depth;
        if observed_queue_depth > self.max_observed_queue_depth {
            self.max_observed_queue_depth = observed_queue_depth;
        }
        self.completed_turns = self.completed_turns.saturating_add(1);
        self.last_turn_latency_micros = turn_latency_micros;
        if turn_latency_micros > self.max_turn_latency_micros {
            self.max_turn_latency_micros = turn_latency_micros;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct QuarantinedCollisionRecord {
    pub incident_id: i64,
    pub producer_id: String,
    pub incarnation: String,
    pub producer_seq: i64,
    pub original_digest: String,
    pub conflicting_digest: String,
    pub observed_utc: String,
}

pub fn qualified_wal_reset_sqlite(version: i32) -> bool {
    version >= 3_051_003
        || (3_050_007..3_051_000).contains(&version)
        || (3_044_006..3_045_000).contains(&version)
}

/// Inspect linked SQLite engine version, source ID, and active WAL/FULL/FK pragmas.
pub fn inspect_sqlite_engine(connection: &Connection) -> Result<SqliteEngineDiagnostics> {
    let version_number = rusqlite::version_number();
    let (version_string, source_id): (String, String) =
        connection.query_row("SELECT sqlite_version(), sqlite_source_id()", [], |row| {
            Ok((stored_text(row, 0, 64)?, stored_text(row, 1, 256)?))
        })?;
    let journal_mode: String = connection
        .query_row("PRAGMA journal_mode", [], |row| stored_text(row, 0, 32))?
        .to_ascii_lowercase();
    let synchronous_level: i64 =
        connection.query_row("PRAGMA synchronous", [], |row| row.get(0))?;
    let foreign_keys: i64 = connection.query_row("PRAGMA foreign_keys", [], |row| row.get(0))?;
    let wal_reset_fix_qualified = qualified_wal_reset_sqlite(version_number);
    if !wal_reset_fix_qualified {
        return Err(StoreError::Invalid(format!(
            "linked SQLite {version_string} ({version_number}) lacks qualified WAL-reset fix"
        )));
    }
    Ok(SqliteEngineDiagnostics {
        version_number,
        version_string,
        source_id,
        wal_reset_fix_qualified,
        journal_mode,
        synchronous_level,
        foreign_keys_enabled: foreign_keys != 0,
    })
}

/// Perform a passive, non-blocking WAL health inspection (`PRAGMA wal_checkpoint(PASSIVE)`).
pub fn inspect_wal_health(connection: &Connection) -> Result<WalHealthReport> {
    let (busy, log_frames, checkpointed_frames): (i64, i64, i64) =
        connection.query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;
    let log_frames_nonneg = log_frames.max(0);
    let checkpointed_nonneg = checkpointed_frames.max(0).min(log_frames_nonneg);
    Ok(WalHealthReport {
        reader_blocked: busy != 0,
        log_frames: log_frames_nonneg,
        checkpointed_frames: checkpointed_nonneg,
        uncheckpointed_frames: log_frames_nonneg - checkpointed_nonneg,
    })
}

/// Read bounded content-collision quarantine records (`integrity_incidents`) in canonical order.
pub fn inspect_quarantined_collisions(
    connection: &Connection,
    limit: usize,
) -> Result<Vec<QuarantinedCollisionRecord>> {
    if limit == 0 || limit > MAX_QUARANTINE_PAGE {
        return Err(StoreError::Invalid(
            "quarantine query limit must be in 1..=256".into(),
        ));
    }
    let limit_i64 = i64::try_from(limit)
        .map_err(|_| StoreError::Invalid("quarantine query limit overflow".into()))?;
    let mut stmt = connection.prepare(
        "SELECT id, producer_id, incarnation, producer_seq, original_digest, conflicting_digest, observed_utc
         FROM integrity_incidents
         ORDER BY id ASC
         LIMIT ?1",
    )?;
    let mut rows = stmt.query([limit_i64])?;
    let mut records = Vec::new();
    while let Some(row) = rows.next()? {
        records.push(QuarantinedCollisionRecord {
            incident_id: row.get(0)?,
            producer_id: stored_text(row, 1, 256)?,
            incarnation: stored_text(row, 2, 256)?,
            producer_seq: row.get(3)?,
            original_digest: stored_text(row, 4, 71)?,
            conflicting_digest: stored_text(row, 5, 71)?,
            observed_utc: stored_text(row, 6, 128)?,
        });
    }
    Ok(records)
}

/// Ingest a producer observation and commit its authorized domain transition with
/// contiguous producer watermark tracking and content-collision quarantine.
pub fn ingest_observation(
    store: &Store,
    source: SourceIdentity,
    event: DomainEvent,
    observed_utc: String,
) -> Result<AppendReceipt> {
    store.append(source, event, observed_utc)
}

/// Commit an authorized domain event optionally co-committed with an atomic outbox intent.
pub fn commit_domain_event(
    store: &Store,
    source: SourceIdentity,
    event: DomainEvent,
    observed_utc: String,
    intent: Option<outbox::OutboxIntent>,
) -> Result<AppendReceipt> {
    match intent {
        Some(intent) => store.append_with_intent(source, event, observed_utc, intent),
        None => store.append(source, event, observed_utc),
    }
}

/// Atomically reserve budget, claim the task, and enqueue the launch outbox intent
/// in a single serialized writer transaction.
pub fn reserve_and_enqueue(
    store: &Store,
    source: SourceIdentity,
    event: DomainEvent,
    observed_utc: String,
    intent: outbox::OutboxIntent,
    reservation: admission_reservations::ReservationCommand,
    proof: admission_reservations::ReservationProof,
) -> Result<ReservedClaimReceipt> {
    store.claim_with_reservation(source, event, observed_utc, intent, reservation, proof)
}
