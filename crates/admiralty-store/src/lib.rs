//! Single-owner durable event store. Replay only applies the pure domain reducer.
pub mod artifacts;
pub mod outbox;
use admiralty_core::{DomainEvent, Projection, SourceIdentity, TransitionError, reduce};
use fs2::FileExt;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    thread::{self, JoinHandle},
    time::Duration,
};

const QUEUE_CAPACITY: usize = 64;
const MAX_EVENT_BYTES: usize = 1024 * 1024;
const MAX_QUERY_EVENTS: usize = 1000;
pub type Result<T> = std::result::Result<T, StoreError>;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("filesystem error: {0}")]
    Io(#[from] std::io::Error),
    #[error("event JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Transition(#[from] TransitionError),
    #[error("outbox storage initialization failed: {0}")]
    Outbox(#[from] outbox::OutboxError),
    #[error("store already has an owner")]
    AlreadyOwned,
    #[error("store writer is unavailable")]
    Closed,
    #[error("invalid store request: {0}")]
    Invalid(String),
    #[error(
        "producer identity collision at {producer_id}/{incarnation}/{sequence}; incident recorded"
    )]
    SourceCollision {
        producer_id: String,
        incarnation: String,
        sequence: String,
    },
    #[error("event replay failed at sequence {sequence}: {reason}")]
    Replay { sequence: String, reason: String },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Cursor {
    pub recovery_generation: String,
    pub event_seq: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Snapshot {
    pub schema_version: u32,
    pub cursor: Cursor,
    pub projection: Projection,
    pub sqlite_version: String,
    pub integrity_incidents: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppendReceipt {
    pub event_seq: String,
    pub source_contiguous_seq: String,
    pub duplicate: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct EventRecord {
    pub seq: String,
    pub observed_utc: String,
    pub event: DomainEvent,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct EventBatch {
    /// Last returned event, rather than the database head, preserves pagination.
    pub cursor: Cursor,
    pub events: Vec<EventRecord>,
}

type Reply<T> = mpsc::Sender<Result<T>>;
enum Request {
    Append {
        source: SourceIdentity,
        source_seq: i64,
        event: Box<DomainEvent>,
        payload: String,
        digest: String,
        observed_utc: String,
        intent: Option<Box<outbox::PreparedIntent>>,
        reply: Reply<AppendReceipt>,
    },
    Snapshot(Reply<Snapshot>),
    Events {
        seq: i64,
        limit: usize,
        reply: Reply<Vec<EventRecord>>,
    },
    EventsSince {
        cursor: Cursor,
        limit: usize,
        reply: Reply<EventBatch>,
    },
    Backup {
        destination: PathBuf,
        reply: Reply<Snapshot>,
    },
    Rebuild(Reply<Snapshot>),
    Shutdown,
}

struct AppendInput {
    source: SourceIdentity,
    source_seq: i64,
    event: DomainEvent,
    payload: String,
    digest: String,
    observed_utc: String,
    intent: Option<outbox::PreparedIntent>,
}

struct Inner {
    sender: mpsc::SyncSender<Request>,
    worker: Mutex<Option<JoinHandle<()>>>,
    changes: tokio::sync::watch::Sender<()>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        // Last Store clone drains every earlier request, then releases the lock.
        let _ = self.sender.send(Request::Shutdown);
        if let Ok(slot) = self.worker.get_mut()
            && let Some(worker) = slot.take()
        {
            let _ = worker.join();
        }
    }
}

#[derive(Clone)]
pub struct Store {
    inner: Arc<Inner>,
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = canonical_database_path(path.as_ref())?;
        let mut lock_path = path.as_os_str().to_os_string();
        lock_path.push(".writer.lock");
        let lock = private_file(&PathBuf::from(lock_path), false)?;
        lock.try_lock_exclusive().map_err(|error| {
            if error.kind() == std::io::ErrorKind::WouldBlock {
                StoreError::AlreadyOwned
            } else {
                StoreError::Io(error)
            }
        })?;
        let (sender, receiver) = mpsc::sync_channel(QUEUE_CAPACITY);
        let (ready_tx, ready_rx) = mpsc::channel();
        let (changes, _) = tokio::sync::watch::channel(());
        let writer_changes = changes.clone();
        let worker = thread::Builder::new()
            .name("admiralty-store-writer".into())
            .spawn(move || match Writer::open(&path, lock) {
                Ok(mut writer) => {
                    if ready_tx.send(Ok(())).is_ok() {
                        writer.run(receiver, writer_changes);
                    }
                }
                Err(error) => {
                    let _ = ready_tx.send(Err(error));
                }
            })?;
        match ready_rx.recv().map_err(|_| StoreError::Closed)? {
            Ok(()) => Ok(Self {
                inner: Arc::new(Inner {
                    sender,
                    worker: Mutex::new(Some(worker)),
                    changes,
                }),
            }),
            Err(error) => {
                let _ = worker.join();
                Err(error)
            }
        }
    }

    fn ask<T>(&self, make: impl FnOnce(Reply<T>) -> Request) -> Result<T> {
        let (sender, receiver) = mpsc::channel();
        self.inner
            .sender
            .send(make(sender))
            .map_err(|_| StoreError::Closed)?;
        receiver.recv().map_err(|_| StoreError::Closed)?
    }

    pub fn append(
        &self,
        source: SourceIdentity,
        event: DomainEvent,
        observed_utc: String,
    ) -> Result<AppendReceipt> {
        self.append_inner(source, event, observed_utc, None)
    }

    /// Atomically persist the event, immutable effect intent, and their association.
    /// This only records a proposal; it never claims or executes the effect.
    pub fn append_with_intent(
        &self,
        source: SourceIdentity,
        event: DomainEvent,
        observed_utc: String,
        intent: outbox::OutboxIntent,
    ) -> Result<AppendReceipt> {
        let intent = outbox::prepare_intent(&intent)?;
        self.append_inner(source, event, observed_utc, Some(intent))
    }

    fn append_inner(
        &self,
        source: SourceIdentity,
        event: DomainEvent,
        observed_utc: String,
        intent: Option<outbox::PreparedIntent>,
    ) -> Result<AppendReceipt> {
        let sequence = source.validate().map_err(StoreError::Invalid)?;
        let source_seq = i64::try_from(sequence).map_err(|_| {
            StoreError::Invalid("source sequence exceeds SQLite signed integer range".into())
        })?;
        if source.producer_id.len() > 256
            || source.incarnation.len() > 256
            || source.producer_id.contains('\0')
            || source.incarnation.contains('\0')
            || observed_utc.is_empty()
            || observed_utc.len() > 128
            || observed_utc.chars().any(char::is_control)
        {
            return Err(StoreError::Invalid(
                "producer identity or observation timestamp is outside bounds".into(),
            ));
        }
        let payload = bounded_json(&event, MAX_EVENT_BYTES).map_err(|()| {
            StoreError::Invalid("event exceeds one MiB or cannot be serialized".into())
        })?;
        let digest = digest(&payload);
        self.ask(|reply| Request::Append {
            source,
            source_seq,
            event: Box::new(event),
            payload,
            digest,
            observed_utc,
            intent: intent.map(Box::new),
            reply,
        })
    }

    /// Coalesced wake hints only; callers must read durable state with their cursor.
    pub fn subscribe_changes(&self) -> tokio::sync::watch::Receiver<()> {
        self.inner.changes.subscribe()
    }

    pub fn snapshot(&self) -> Result<Snapshot> {
        self.ask(Request::Snapshot)
    }

    pub fn events_after(&self, seq: u64, limit: usize) -> Result<Vec<EventRecord>> {
        let seq = i64::try_from(seq).map_err(|_| {
            StoreError::Invalid("event cursor exceeds SQLite signed integer range".into())
        })?;
        self.ask(|reply| Request::Events {
            seq,
            limit: limit.min(MAX_QUERY_EVENTS),
            reply,
        })
    }

    /// Validate generation and read its next bounded page in one writer turn.
    pub fn events_since(&self, cursor: &Cursor, limit: usize) -> Result<EventBatch> {
        self.ask(|reply| Request::EventsSince {
            cursor: cursor.clone(),
            limit: limit.min(MAX_QUERY_EVENTS),
            reply,
        })
    }

    pub fn backup(&self, destination: &Path) -> Result<Snapshot> {
        self.ask(|reply| Request::Backup {
            destination: destination.to_owned(),
            reply,
        })
    }

    /// Recompute the projection from durable events without running external effects.
    /// Successful explicit rebuild changes the cursor generation for subscribers.
    pub fn rebuild(&self) -> Result<Snapshot> {
        self.ask(Request::Rebuild)
    }
}

// Existing parents may be searchable, but never writable by another Unix user.
// Newly created data directories are private. SQLite inherits the0600 database
// mode for WAL/SHM; reject pre-existing unsafe sidecars before opening SQLite.
fn canonical_database_path(path: &Path) -> Result<PathBuf> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(parent)?;
    let parent = parent.canonicalize()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if parent.metadata()?.permissions().mode() & 0o022 != 0 {
            return Err(StoreError::Invalid(
                "database parent must not be group/world writable".into(),
            ));
        }
    }
    let name = path
        .file_name()
        .ok_or_else(|| StoreError::Invalid("database path requires a filename".into()))?;
    let path = parent.join(name);
    drop(private_file(&path, false)?);
    for suffix in ["-wal", "-shm"] {
        let mut sidecar = path.as_os_str().to_os_string();
        sidecar.push(suffix);
        validate_private_file(&PathBuf::from(sidecar))?;
    }
    Ok(path)
}

fn validate_private_file(path: &Path) -> Result<()> {
    match path.symlink_metadata() {
        Ok(metadata) => {
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err(StoreError::Invalid(
                    "database files must be regular files, never symlinks".into(),
                ));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::{MetadataExt, PermissionsExt};
                if metadata.permissions().mode() & 0o077 != 0 || metadata.nlink() != 1 {
                    return Err(StoreError::Invalid(
                        "database files require private0600 permissions and one link".into(),
                    ));
                }
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn private_file(path: &Path, create_new: bool) -> Result<File> {
    validate_private_file(path)?;
    let mut options = OpenOptions::new();
    options.read(true).write(true).truncate(false);
    if create_new {
        options.create_new(true);
    } else {
        options.create(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path)?;
    validate_private_file(path)?;
    Ok(file)
}

fn digest(payload: &str) -> String {
    format!("sha256:{:x}", Sha256::digest(payload.as_bytes()))
}

struct BoundedJsonWriter {
    bytes: Vec<u8>,
    maximum: usize,
    overflowed: bool,
}

impl std::io::Write for BoundedJsonWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self
            .bytes
            .len()
            .checked_add(bytes.len())
            .is_none_or(|size| size > self.maximum)
        {
            self.overflowed = true;
            return Err(std::io::Error::new(
                std::io::ErrorKind::WriteZero,
                "JSON byte limit exceeded",
            ));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn bounded_json(value: &impl serde::Serialize, maximum: usize) -> std::result::Result<String, ()> {
    let mut writer = BoundedJsonWriter {
        bytes: Vec::with_capacity(maximum.min(4096)),
        maximum,
        overflowed: false,
    };
    if serde_json::to_writer(&mut writer, value).is_err() || writer.overflowed {
        return Err(());
    }
    String::from_utf8(writer.bytes).map_err(|_| ())
}

fn identity_digest(event_digest: &str, observed_utc: &str, intent_digest: Option<&str>) -> String {
    let mut hash = Sha256::new();
    hash.update(event_digest.as_bytes());
    hash.update([0]);
    hash.update(observed_utc.as_bytes());
    hash.update([0]);
    hash.update(intent_digest.unwrap_or("plain-event").as_bytes());
    format!("sha256:{:x}", hash.finalize())
}

fn fixed_sqlite(version: i32) -> bool {
    version >= 3_051_003
        || (3_050_007..3_051_000).contains(&version)
        || (3_044_006..3_045_000).contains(&version)
}

fn source_collision(source: &SourceIdentity) -> StoreError {
    StoreError::SourceCollision {
        producer_id: source.producer_id.clone(),
        incarnation: source.incarnation.clone(),
        sequence: source.sequence.clone(),
    }
}

fn record_source_collision(
    tx: &Transaction<'_>,
    source: &SourceIdentity,
    source_seq: i64,
    original_digest: &str,
    conflicting_digest: &str,
    observed_utc: &str,
) -> Result<()> {
    tx.execute(
        "INSERT INTO integrity_incidents(producer_id,incarnation,producer_seq,original_digest,conflicting_digest,observed_utc) VALUES(?1,?2,?3,?4,?5,?6)",
        params![source.producer_id, source.incarnation, source_seq, original_digest, conflicting_digest, observed_utc],
    )?;
    Ok(())
}

fn associated_intent(
    tx: &Transaction<'_>,
    event_seq: i64,
) -> Result<Option<outbox::PreparedIntent>> {
    struct AssociationRow {
        association_id: i64,
        association_key: String,
        association_payload_digest: String,
        association_due: i64,
        association_intent_digest: String,
        intent_id: i64,
        key: String,
        payload: String,
        payload_digest: String,
        due: i64,
    }
    let row = tx.query_row(
        "SELECT a.outbox_intent_id,a.idempotency_key,a.payload_digest,a.not_before,a.intent_digest,o.id,o.idempotency_key,o.payload,o.payload_digest,o.not_before FROM event_outbox_associations a JOIN outbox_intents o ON o.id=a.outbox_intent_id WHERE a.event_seq=?1",
        [event_seq],
        |row| Ok(AssociationRow {
            association_id: row.get(0)?, association_key: row.get(1)?,
            association_payload_digest: row.get(2)?, association_due: row.get(3)?,
            association_intent_digest: row.get(4)?, intent_id: row.get(5)?,
            key: row.get(6)?, payload: row.get(7)?, payload_digest: row.get(8)?, due: row.get(9)?,
        }),
    ).optional()?;
    let Some(row) = row else {
        return Ok(None);
    };
    let calculated_payload_digest = digest(&row.payload);
    let calculated_intent_digest = outbox::prepared_intent_digest(&row.key, &row.payload, row.due);
    if row.association_id != row.intent_id
        || row.association_key != row.key
        || row.association_payload_digest != row.payload_digest
        || row.association_due != row.due
        || row.payload_digest != calculated_payload_digest
        || row.association_intent_digest != calculated_intent_digest
    {
        return Err(StoreError::Replay {
            sequence: event_seq.to_string(),
            reason: "event/outbox association integrity mismatch".into(),
        });
    }
    Ok(Some(outbox::PreparedIntent {
        idempotency_key: row.key,
        payload: row.payload,
        payload_digest: row.payload_digest,
        intent_digest: row.association_intent_digest,
        not_before: row.due,
    }))
}

struct Writer {
    connection: Connection,
    projection: Projection,
    generation: String,
    _lock: File,
}

impl Writer {
    fn open(path: &Path, lock: File) -> Result<Self> {
        if !fixed_sqlite(rusqlite::version_number()) {
            return Err(StoreError::Invalid(format!(
                "SQLite {} lacks the required WAL fix",
                rusqlite::version()
            )));
        }
        let connection = Connection::open(path)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;",
        )?;
        let journal: String = connection.query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
        let synchronous: i64 = connection.query_row("PRAGMA synchronous", [], |row| row.get(0))?;
        let foreign_keys: i64 =
            connection.query_row("PRAGMA foreign_keys", [], |row| row.get(0))?;
        if journal != "wal" || synchronous != 2 || foreign_keys != 1 {
            return Err(StoreError::Invalid(
                "required SQLite durability pragmas were not applied".into(),
            ));
        }
        let schema: u32 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if schema > 1 {
            return Err(StoreError::Invalid(
                "database schema is newer than this executable".into(),
            ));
        }
        connection.execute_batch(include_str!("../migrations/0001_foundation.sql"))?;
        // Storage initialization only. Startup/replay never reserves or delivers
        // an outbox intent; explicit trusted-clock helpers remain separate.
        outbox::initialize(&connection)?;
        connection.execute_batch(include_str!("../migrations/0003_atomic_effects.sql"))?;
        let generation = connection
            .query_row(
                "SELECT value FROM metadata WHERE key='recovery_generation'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        let generation = match generation {
            Some(value) => value,
            None => {
                let value = uuid::Uuid::new_v4().to_string();
                connection.execute(
                    "INSERT INTO metadata(key,value) VALUES('recovery_generation',?1)",
                    [&value],
                )?;
                value
            }
        };
        let projection = replay(&connection)?;
        Ok(Self {
            connection,
            projection,
            generation,
            _lock: lock,
        })
    }

    fn run(&mut self, receiver: mpsc::Receiver<Request>, changes: tokio::sync::watch::Sender<()>) {
        while let Ok(request) = receiver.recv() {
            match request {
                Request::Append {
                    source,
                    source_seq,
                    event,
                    payload,
                    digest,
                    observed_utc,
                    intent,
                    reply,
                } => {
                    let result = self.append(AppendInput {
                        source,
                        source_seq,
                        event: *event,
                        payload,
                        digest,
                        observed_utc,
                        intent: intent.map(|intent| *intent),
                    });
                    if result.as_ref().is_ok_and(|receipt| !receipt.duplicate) {
                        changes.send_replace(());
                    }
                    let _ = reply.send(result);
                }
                Request::Snapshot(reply) => {
                    let _ = reply.send(self.snapshot());
                }
                Request::Events { seq, limit, reply } => {
                    let _ = reply.send(self.events_after(seq, limit));
                }
                Request::EventsSince {
                    cursor,
                    limit,
                    reply,
                } => {
                    let _ = reply.send(self.events_since(&cursor, limit));
                }
                Request::Backup { destination, reply } => {
                    let _ = reply.send(self.backup(&destination));
                }
                Request::Rebuild(reply) => {
                    let result = self.rebuild();
                    if result.is_ok() {
                        changes.send_replace(());
                    }
                    let _ = reply.send(result);
                }
                Request::Shutdown => break,
            }
        }
        let _ = self
            .connection
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
    }

    fn append(&mut self, input: AppendInput) -> Result<AppendReceipt> {
        let AppendInput {
            source,
            source_seq,
            event,
            payload,
            digest: incoming_digest,
            observed_utc,
            intent,
        } = input;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing: Option<(i64, String, String, String)> = tx.query_row(
            "SELECT seq,digest,payload,observed_utc FROM domain_events WHERE producer_id=?1 AND incarnation=?2 AND producer_seq=?3",
            params![source.producer_id, source.incarnation, source_seq], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))).optional()?;
        if let Some((seq, original_digest, original_payload, original_observed)) = existing {
            let original_intent = associated_intent(&tx, seq)?;
            let intent_matches = match (&original_intent, &intent) {
                (None, None) => true,
                (Some(original), Some(incoming)) => {
                    original.idempotency_key == incoming.idempotency_key
                        && original.payload == incoming.payload
                        && original.payload_digest == incoming.payload_digest
                        && original.not_before == incoming.not_before
                        && original.intent_digest == incoming.intent_digest
                }
                _ => false,
            };
            if original_digest != incoming_digest
                || original_payload != payload
                || original_observed != observed_utc
                || !intent_matches
            {
                let original_identity = identity_digest(
                    &original_digest,
                    &original_observed,
                    original_intent
                        .as_ref()
                        .map(|value| value.intent_digest.as_str()),
                );
                let conflicting_identity = identity_digest(
                    &incoming_digest,
                    &observed_utc,
                    intent.as_ref().map(|value| value.intent_digest.as_str()),
                );
                record_source_collision(
                    &tx,
                    &source,
                    source_seq,
                    &original_identity,
                    &conflicting_identity,
                    &observed_utc,
                )?;
                tx.commit()?;
                return Err(source_collision(&source));
            }
            let contiguous: i64 = tx.query_row("SELECT contiguous_seq FROM producer_watermarks WHERE producer_id=?1 AND incarnation=?2",
                params![source.producer_id, source.incarnation], |row| row.get(0)).optional()?.unwrap_or(0);
            return Ok(AppendReceipt {
                event_seq: seq.to_string(),
                source_contiguous_seq: contiguous.to_string(),
                duplicate: true,
            });
        }
        let next_projection = reduce(&self.projection, &event)?;
        let mut associated_intent = None;
        if let Some(intent) = &intent {
            match outbox::enqueue_prepared_in_transaction(&tx, intent)? {
                outbox::EnqueueResult::Inserted { id } => associated_intent = Some((id, intent)),
                outbox::EnqueueResult::ExistingExact { .. } => {
                    tx.execute("INSERT INTO outbox_integrity_incidents(idempotency_key,original_digest,conflicting_digest) VALUES(?1,?2,?2)", params![intent.idempotency_key, intent.intent_digest])?;
                    tx.commit()?;
                    return Err(StoreError::Outbox(outbox::OutboxError::IdentityCollision));
                }
                outbox::EnqueueResult::Collision => {
                    tx.commit()?;
                    return Err(StoreError::Outbox(outbox::OutboxError::IdentityCollision));
                }
            }
        }
        tx.execute("INSERT INTO domain_events(producer_id,incarnation,producer_seq,digest,observed_utc,payload) VALUES(?1,?2,?3,?4,?5,?6)",
            params![source.producer_id, source.incarnation, source_seq, incoming_digest, observed_utc, payload])?;
        let seq = tx.last_insert_rowid();
        if let Some((intent_id, intent)) = associated_intent {
            tx.execute("INSERT INTO event_outbox_associations(event_seq,outbox_intent_id,idempotency_key,payload_digest,not_before,intent_digest) VALUES(?1,?2,?3,?4,?5,?6)",
                params![seq, intent_id, intent.idempotency_key, intent.payload_digest, intent.not_before, intent.intent_digest])?;
        }
        let mut contiguous: i64 = tx.query_row("SELECT contiguous_seq FROM producer_watermarks WHERE producer_id=?1 AND incarnation=?2",
            params![source.producer_id, source.incarnation], |row| row.get(0)).optional()?.unwrap_or(0);
        while let Some(next) = contiguous.checked_add(1) {
            let present: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM domain_events WHERE producer_id=?1 AND incarnation=?2 AND producer_seq=?3)",
                params![source.producer_id, source.incarnation, next], |row| row.get(0))?;
            if !present {
                break;
            }
            contiguous = next;
        }
        tx.execute("INSERT INTO producer_watermarks(producer_id,incarnation,contiguous_seq) VALUES(?1,?2,?3) ON CONFLICT(producer_id,incarnation) DO UPDATE SET contiguous_seq=excluded.contiguous_seq",
            params![source.producer_id, source.incarnation, contiguous])?;
        tx.commit()?;
        self.projection = next_projection;
        Ok(AppendReceipt {
            event_seq: seq.to_string(),
            source_contiguous_seq: contiguous.to_string(),
            duplicate: false,
        })
    }

    fn snapshot(&self) -> Result<Snapshot> {
        let seq: i64 = self.connection.query_row(
            "SELECT COALESCE(MAX(seq),0) FROM domain_events",
            [],
            |row| row.get(0),
        )?;
        let incidents: i64 =
            self.connection
                .query_row("SELECT COUNT(*) FROM integrity_incidents", [], |row| {
                    row.get(0)
                })?;
        Ok(Snapshot {
            schema_version: 1,
            cursor: Cursor {
                recovery_generation: self.generation.clone(),
                event_seq: seq.to_string(),
            },
            projection: self.projection.clone(),
            sqlite_version: rusqlite::version().to_owned(),
            integrity_incidents: incidents.to_string(),
        })
    }

    fn events_after(&self, seq: i64, limit: usize) -> Result<Vec<EventRecord>> {
        let mut query = self.connection.prepare(
            "SELECT seq,observed_utc,payload,digest FROM domain_events WHERE seq>?1 ORDER BY seq LIMIT ?2",
        )?;
        let mut rows = query.query(params![seq, limit as i64])?;
        let mut events = Vec::new();
        while let Some(row) = rows.next()? {
            let seq: i64 = row.get(0)?;
            let payload: String = row.get(2)?;
            let expected: String = row.get(3)?;
            if payload.len() > MAX_EVENT_BYTES || digest(&payload) != expected {
                return Err(StoreError::Replay {
                    sequence: seq.to_string(),
                    reason: "stored event size/digest mismatch".into(),
                });
            }
            events.push(EventRecord {
                seq: seq.to_string(),
                observed_utc: row.get(1)?,
                event: serde_json::from_str(&payload)?,
            });
        }
        Ok(events)
    }

    fn events_since(&self, cursor: &Cursor, limit: usize) -> Result<EventBatch> {
        let invalid = |reason: &str| StoreError::Invalid(format!("snapshot_required: {reason}"));
        if cursor.recovery_generation != self.generation {
            return Err(invalid("recovery generation changed"));
        }
        let seq = cursor
            .event_seq
            .parse::<i64>()
            .ok()
            .filter(|seq| *seq >= 0 && seq.to_string() == cursor.event_seq)
            .ok_or_else(|| invalid("invalid event sequence"))?;
        let head: i64 = self.connection.query_row(
            "SELECT COALESCE(MAX(seq),0) FROM domain_events",
            [],
            |row| row.get(0),
        )?;
        if seq > head {
            return Err(invalid("event cursor is ahead of durable state"));
        }
        let events = self.events_after(seq, limit)?;
        let event_seq = events
            .last()
            .map(|event| event.seq.clone())
            .unwrap_or_else(|| cursor.event_seq.clone());
        Ok(EventBatch {
            cursor: Cursor {
                recovery_generation: self.generation.clone(),
                event_seq,
            },
            events,
        })
    }

    fn rebuild(&mut self) -> Result<Snapshot> {
        let projection = replay(&self.connection)?;
        let generation = uuid::Uuid::new_v4().to_string();
        self.connection.execute(
            "UPDATE metadata SET value=?1 WHERE key='recovery_generation'",
            [&generation],
        )?;
        self.projection = projection;
        self.generation = generation;
        self.snapshot()
    }

    fn backup(&self, destination: &Path) -> Result<Snapshot> {
        // Hard-link publication is atomic and refuses an existing destination.
        // Staging and destination share a directory, so publication cannot cross filesystems.
        let parent = destination
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let stage = parent.join(format!(".admiralty-backup-{}.sqlite", uuid::Uuid::new_v4()));
        let stage_file = private_file(&stage, true)?;
        drop(stage_file);
        let result = (|| {
            if destination.symlink_metadata().is_ok() {
                return Err(StoreError::Invalid(
                    "backup destination already exists".into(),
                ));
            }
            let mut target = Connection::open(&stage)?;
            {
                let backup = rusqlite::backup::Backup::new(&self.connection, &mut target)?;
                backup.run_to_completion(128, Duration::from_millis(5), None)?;
            }
            let check: String = target.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
            if check != "ok" {
                return Err(StoreError::Invalid(
                    "backup SQLite quick_check failed".into(),
                ));
            }
            target
                .close()
                .map_err(|(_, error)| StoreError::Sqlite(error))?;
            File::open(&stage)?.sync_all()?;
            fs::hard_link(&stage, destination)?;
            File::open(parent)?.sync_all()?;
            self.snapshot()
        })();
        let _ = fs::remove_file(&stage);
        result
    }
}

fn replay(connection: &Connection) -> Result<Projection> {
    let mut projection = Projection::default();
    let mut statement =
        connection.prepare("SELECT seq,digest,payload FROM domain_events ORDER BY seq")?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let seq: i64 = row.get(0)?;
        let expected: String = row.get(1)?;
        let payload: String = row.get(2)?;
        if digest(&payload) != expected {
            return Err(StoreError::Replay {
                sequence: seq.to_string(),
                reason: "stored event digest mismatch".into(),
            });
        }
        let event: DomainEvent =
            serde_json::from_str(&payload).map_err(|error| StoreError::Replay {
                sequence: seq.to_string(),
                reason: error.to_string(),
            })?;
        projection = reduce(&projection, &event).map_err(|error| StoreError::Replay {
            sequence: seq.to_string(),
            reason: error.to_string(),
        })?;
    }
    Ok(projection)
}

#[cfg(test)]
mod tests {
    use super::{Store, fixed_sqlite, outbox};
    use admiralty_core::{DomainEvent, SourceIdentity, TaskContract, TaskId};
    use rusqlite::Connection;
    use serde_json::json;

    fn source(sequence: &str) -> SourceIdentity {
        SourceIdentity {
            producer_id: "test-producer".into(),
            incarnation: "incarnation-a".into(),
            sequence: sequence.into(),
        }
    }

    fn submitted(task: &str) -> DomainEvent {
        let contract: TaskContract = serde_json::from_value(json!({
            "schema_version":1,"project_id":"project-a","root_id":"root-a","task_id":task,
            "revision":1,"title":"Atomic storage fixture","objective":"Persist event with effect intent",
            "non_goals":[],"owned_paths":["src/lib.rs"],"forbidden_paths":[],"read_first":[],"dependencies":[],
            "criteria":[{"id":"C1","text":"Commit atomically","check_ids":["unit"]}],
            "checks":[{"id":"unit","program":"cargo","args":["test"],"cwd":".","timeout_secs":10}],
            "authority_ref":"fixture:bounded","max_attempts":1,"simulated":true
        })).expect("valid task contract fixture");
        DomainEvent::TaskSubmitted { contract }
    }

    fn intent(key: &str, payload: serde_json::Value, not_before: u64) -> outbox::OutboxIntent {
        outbox::OutboxIntent {
            idempotency_key: key.into(),
            payload,
            not_before,
        }
    }

    fn counts(path: &std::path::Path) -> (i64, i64, i64) {
        let connection = Connection::open(path).expect("inspection connection");
        let events = connection
            .query_row("SELECT COUNT(*) FROM domain_events", [], |row| row.get(0))
            .expect("event count");
        let effects = connection
            .query_row("SELECT COUNT(*) FROM outbox_intents", [], |row| row.get(0))
            .expect("intent count");
        let links = connection
            .query_row(
                "SELECT COUNT(*) FROM event_outbox_associations",
                [],
                |row| row.get(0),
            )
            .expect("association count");
        (events, effects, links)
    }

    fn private_tempdir() -> tempfile::TempDir {
        let root = std::path::PathBuf::from(
            std::env::var_os("CARGO_TARGET_DIR")
                .expect("private CARGO_TARGET_DIR must be configured"),
        )
        .join("store-fixtures");
        std::fs::create_dir_all(&root).expect("create private fixture root");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))
                .expect("restrict fixture root permissions");
        }
        let directory = tempfile::Builder::new()
            .prefix("adm-store-")
            .tempdir_in(root)
            .expect("private fixture directory");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .expect("restrict fixture directory permissions");
        }
        directory
    }
    #[test]
    fn wal_fix_version_gate() {
        for version in [3_044_006, 3_044_007, 3_050_007, 3_051_003, 3_052_000] {
            assert!(fixed_sqlite(version));
        }
        for version in [3_044_005, 3_045_000, 3_050_006, 3_051_002] {
            assert!(!fixed_sqlite(version));
        }
    }

    #[test]
    fn store_open_initializes_outbox_and_reopen_replay_do_not_dispatch_it() {
        let directory = private_tempdir();
        let path = directory.path().join("store.sqlite");
        let store = Store::open(&path).expect("normal store initializes migrations");
        let initial = store.snapshot().expect("initial snapshot");
        drop(store);
        // The Store is closed before this explicit helper fixture writes rows.
        let mut fixture = rusqlite::Connection::open(&path).expect("fixture connection");
        let tables: i64 = fixture.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('outbox_clock','outbox_intents','outbox_integrity_incidents')", [], |row| row.get(0)).expect("outbox tables");
        assert_eq!(tables, 3);
        let atomic_table: i64 = fixture.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='event_outbox_associations'", [], |row| row.get(0)).expect("atomic association migration");
        assert_eq!(atomic_table, 1);
        let user_version: i64 = fixture
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("supported schema version");
        assert_eq!(
            user_version, 1,
            "component migration preserves integration-owned version"
        );
        let claimed_id = outbox::enqueue(
            &mut fixture,
            outbox::OutboxIntent {
                idempotency_key: "claimed".into(),
                payload: serde_json::json!({"reference":"first"}),
                not_before: 0,
            },
        )
        .expect("first intent");
        let pending_id = outbox::enqueue(
            &mut fixture,
            outbox::OutboxIntent {
                idempotency_key: "pending".into(),
                payload: serde_json::json!({"reference":"second"}),
                not_before: 0,
            },
        )
        .expect("second intent");
        let claim = outbox::claim_due(&mut fixture, Some(0), "fixture-owner", 5)
            .expect("explicit reservation")
            .expect("claim");
        assert_eq!(claim.id, claimed_id);
        drop(fixture);

        let reopened = Store::open(&path).expect("normal reopen reruns idempotent migrations");
        assert_eq!(reopened.snapshot().expect("reopened snapshot"), initial);
        let rebuilt = reopened.rebuild().expect("pure event replay");
        assert_eq!(rebuilt.projection, initial.projection);
        assert_eq!(rebuilt.cursor.event_seq, "0");
        drop(reopened);
        let mut fixture = rusqlite::Connection::open(&path).expect("inspect outbox after replay");
        let rows: Vec<(String, String)> = fixture
            .prepare("SELECT id,state FROM outbox_intents ORDER BY id")
            .expect("states query")
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?.to_string(), row.get(1)?))
            })
            .expect("states")
            .collect::<rusqlite::Result<_>>()
            .expect("state rows");
        assert_eq!(
            rows,
            vec![
                (claimed_id.clone(), "claimed".into()),
                (pending_id.clone(), "pending".into())
            ]
        );
        let observed: i64 = fixture
            .query_row(
                "SELECT last_observed FROM outbox_clock WHERE singleton=1",
                [],
                |row| row.get(0),
            )
            .expect("recorded clock");
        assert_eq!(observed, 0, "startup and rebuild never invent a clock");
        let next = outbox::claim_due(&mut fixture, Some(6), "restarted-owner", 5)
            .expect("explicit recovery observation")
            .expect("next due intent");
        assert_eq!(next.id, pending_id);
        let old_state: String = fixture
            .query_row(
                "SELECT state FROM outbox_intents WHERE id=?1",
                [&claimed_id],
                |row| row.get(0),
            )
            .expect("old claim state");
        assert_eq!(old_state, "unknown");
    }

    #[test]
    fn atomic_append_exact_replay_reopen_and_rebuild_preserve_one_pending_association() {
        let directory = private_tempdir();
        let path = directory.path().join("atomic.sqlite");
        let store = Store::open(&path).expect("open store");
        let mut changes = store.subscribe_changes();
        let event = submitted("task-one");
        let effect = intent("effect-one", json!({"z":2,"a":1}), 15);
        let appended = store
            .append_with_intent(
                source("1"),
                event.clone(),
                "2026-01-01T00:00:00Z".into(),
                effect.clone(),
            )
            .expect("atomic append");
        assert!(!appended.duplicate);
        assert!(changes.has_changed().expect("watch status"));
        changes.borrow_and_update();
        let duplicate = store
            .append_with_intent(
                source("1"),
                event.clone(),
                "2026-01-01T00:00:00Z".into(),
                effect.clone(),
            )
            .expect("exact event/effect replay");
        assert!(duplicate.duplicate);
        assert!(!changes.has_changed().expect("duplicate does not notify"));
        assert_eq!(counts(&path), (1, 1, 1));
        drop(store);

        let reopened = Store::open(&path).expect("reopen");
        assert_eq!(
            reopened
                .snapshot()
                .expect("snapshot")
                .projection
                .tasks
                .len(),
            1
        );
        let rebuilt = reopened.rebuild().expect("pure projection rebuild");
        assert_eq!(rebuilt.projection.tasks.len(), 1);
        assert_eq!(counts(&path), (1, 1, 1));
        drop(reopened);
        let connection = Connection::open(&path).expect("inspect outbox");
        assert!(
            connection
                .execute(
                    "UPDATE event_outbox_associations SET not_before=16 WHERE event_seq=1",
                    []
                )
                .is_err(),
            "association identity is immutable"
        );
        let state: String = connection
            .query_row(
                "SELECT state FROM outbox_intents WHERE idempotency_key='effect-one'",
                [],
                |row| row.get(0),
            )
            .expect("pending effect");
        assert_eq!(state, "pending", "reopen/rebuild never claims or delivers");
    }

    #[test]
    fn event_replay_rejects_changed_effect_key_schedule_and_plain_mismatch() {
        let directory = private_tempdir();
        let path = directory.path().join("replay.sqlite");
        let store = Store::open(&path).expect("open store");
        let event = submitted("task-one");
        let original = intent("effect-one", json!({"reference":"a"}), 10);
        store
            .append_with_intent(
                source("1"),
                event.clone(),
                "time-a".into(),
                original.clone(),
            )
            .expect("append");
        let changed_payload = intent("effect-one", json!({"reference":"b"}), 10);
        let changed_key = intent("effect-two", json!({"reference":"a"}), 10);
        let changed_time = intent("effect-one", json!({"reference":"a"}), 11);
        for changed in [changed_payload, changed_key, changed_time] {
            assert!(matches!(
                store.append_with_intent(source("1"), event.clone(), "time-a".into(), changed),
                Err(super::StoreError::SourceCollision { .. })
            ));
        }
        assert!(
            matches!(
                store.append(source("1"), event.clone(), "time-a".into()),
                Err(super::StoreError::SourceCollision { .. })
            ),
            "plain replay cannot detach effect"
        );
        assert!(
            matches!(
                store.append_with_intent(
                    source("1"),
                    submitted("different-task"),
                    "time-a".into(),
                    original.clone()
                ),
                Err(super::StoreError::SourceCollision { .. })
            ),
            "changed event bytes collide even when the effect is unchanged"
        );
        assert!(
            matches!(
                store.append_with_intent(source("1"), event, "time-b".into(), original),
                Err(super::StoreError::SourceCollision { .. })
            ),
            "event observation time is part of exact replay"
        );
        assert_eq!(counts(&path), (1, 1, 1));
        let incidents: i64 = Connection::open(&path)
            .expect("incident connection")
            .query_row("SELECT COUNT(*) FROM integrity_incidents", [], |row| {
                row.get(0)
            })
            .expect("incident count");
        assert_eq!(incidents, 6);
    }

    #[test]
    fn plain_event_cannot_be_upgraded_to_effect_and_intent_cannot_bind_twice() {
        let directory = private_tempdir();
        let path = directory.path().join("binding.sqlite");
        let store = Store::open(&path).expect("open store");
        let first = submitted("task-one");
        store
            .append(source("1"), first.clone(), "time-a".into())
            .expect("plain append");
        assert!(matches!(
            store.append_with_intent(
                source("1"),
                first,
                "time-a".into(),
                intent("effect-one", json!({"x":1}), 0)
            ),
            Err(super::StoreError::SourceCollision { .. })
        ));
        let other = submitted("task-two");
        store
            .append_with_intent(
                source("2"),
                other.clone(),
                "time-b".into(),
                intent("effect-two", json!({"x":2}), 0),
            )
            .expect("second event intent");
        assert!(matches!(
            store.append_with_intent(
                source("3"),
                submitted("task-three"),
                "time-c".into(),
                intent("effect-two", json!({"x":2}), 0)
            ),
            Err(super::StoreError::Outbox(
                outbox::OutboxError::IdentityCollision
            ))
        ));
        assert_eq!(counts(&path), (2, 1, 1));
        let incident_count: i64 = Connection::open(&path)
            .expect("incidents connection")
            .query_row(
                "SELECT COUNT(*) FROM outbox_integrity_incidents",
                [],
                |row| row.get(0),
            )
            .expect("outbox incident count");
        assert_eq!(
            incident_count, 1,
            "reusing an already associated exact intent is a durable collision"
        );
    }

    #[test]
    fn invalid_reducer_transition_and_sql_association_failure_leave_no_partial_state_or_watch() {
        let directory = private_tempdir();
        let path = directory.path().join("rollback.sqlite");
        let store = Store::open(&path).expect("open store");
        let before = store.snapshot().expect("initial projection");
        let changes = store.subscribe_changes();
        assert!(matches!(
            store.append_with_intent(
                source("1"),
                DomainEvent::TaskStarted {
                    task_id: TaskId::new("missing").expect("task fixture"),
                    attempt_id: admiralty_core::AttemptId::new("attempt").expect("attempt fixture")
                },
                "time-a".into(),
                intent("invalid", json!({"x":1}), 0)
            ),
            Err(super::StoreError::Transition(_))
        ));
        assert_eq!(
            store.snapshot().expect("unchanged snapshot").projection,
            before.projection
        );
        assert!(
            !changes
                .has_changed()
                .expect("invalid transition does not notify")
        );
        assert_eq!(counts(&path), (0, 0, 0));

        Connection::open(&path).expect("trigger connection").execute_batch("CREATE TRIGGER reject_event_outbox_link BEFORE INSERT ON event_outbox_associations BEGIN SELECT RAISE(ABORT,'injected link failure'); END;").expect("install association fault");
        assert!(
            store
                .append_with_intent(
                    source("2"),
                    submitted("task-two"),
                    "time-b".into(),
                    intent("rollback-effect", json!({"x":2}), 0)
                )
                .is_err()
        );
        assert_eq!(
            store
                .snapshot()
                .expect("failed append leaves projection")
                .projection,
            before.projection
        );
        assert!(
            !changes
                .has_changed()
                .expect("failed SQL transaction does not notify")
        );
        assert_eq!(store.events_after(0, 10).expect("event query").len(), 0);
        assert_eq!(
            counts(&path),
            (0, 0, 0),
            "event and outbox insert both roll back"
        );
    }

    #[test]
    fn bounded_inputs_reject_oversized_key_payload_time_event_and_sqlite_counter() {
        let directory = private_tempdir();
        let path = directory.path().join("bounds.sqlite");
        let store = Store::open(&path).expect("open store");
        let baseline = submitted("task-one");
        let oversized_key = intent(&"k".repeat(257), json!({"x":1}), 0);
        assert!(matches!(
            store.append_with_intent(source("1"), baseline.clone(), "time".into(), oversized_key),
            Err(super::StoreError::Outbox(outbox::OutboxError::Invalid(_)))
        ));
        let oversized_payload = intent(
            "large-payload",
            json!("x".repeat(outbox::MAX_PAYLOAD_BYTES)),
            0,
        );
        assert!(matches!(
            store.append_with_intent(
                source("1"),
                baseline.clone(),
                "time".into(),
                oversized_payload
            ),
            Err(super::StoreError::Outbox(outbox::OutboxError::Invalid(_)))
        ));
        assert!(matches!(
            store.append_with_intent(
                source("1"),
                baseline.clone(),
                "t".repeat(129),
                intent("long-time", json!({}), 0)
            ),
            Err(super::StoreError::Invalid(_))
        ));
        assert!(matches!(
            store.append_with_intent(
                source("1"),
                baseline.clone(),
                "time".into(),
                intent("counter-overflow", json!({}), u64::MAX)
            ),
            Err(super::StoreError::Outbox(outbox::OutboxError::Overflow))
        ));
        let oversized_event = DomainEvent::CandidateSubmitted {
            task_id: TaskId::new("task-one").expect("task fixture"),
            attempt_id: admiralty_core::AttemptId::new("attempt").expect("attempt fixture"),
            candidate_ref: "x".repeat(1024 * 1024),
        };
        assert!(matches!(
            store.append(source("1"), oversized_event, "time".into()),
            Err(super::StoreError::Invalid(_))
        ));
        assert!(
            matches!(
                store.append(source("9223372036854775808"), baseline, "time".into()),
                Err(super::StoreError::Invalid(_))
            ),
            "u64 source sequences outside SQLite's signed range are rejected"
        );
        assert_eq!(counts(&path), (0, 0, 0));
    }
}
