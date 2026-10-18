//! Single-owner durable event store. Replay only applies the pure domain reducer.
pub mod artifacts;
use admiralty_core::{DomainEvent, Projection, SourceIdentity, TransitionError, reduce};
use fs2::FileExt;
use rusqlite::{Connection, OptionalExtension, params};
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
        {
            return Err(StoreError::Invalid(
                "producer identity or observation timestamp is outside bounds".into(),
            ));
        }
        let payload = serde_json::to_string(&event)?;
        if payload.len() > MAX_EVENT_BYTES {
            return Err(StoreError::Invalid("event exceeds one MiB".into()));
        }
        let digest = digest(&payload);
        self.ask(|reply| Request::Append {
            source,
            source_seq,
            event: Box::new(event),
            payload,
            digest,
            observed_utc,
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

fn fixed_sqlite(version: i32) -> bool {
    version >= 3_051_003
        || (3_050_007..3_051_000).contains(&version)
        || (3_044_006..3_045_000).contains(&version)
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
                    reply,
                } => {
                    let result =
                        self.append(source, source_seq, *event, payload, digest, observed_utc);
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

    fn append(
        &mut self,
        source: SourceIdentity,
        source_seq: i64,
        event: DomainEvent,
        payload: String,
        incoming_digest: String,
        observed_utc: String,
    ) -> Result<AppendReceipt> {
        let tx = self.connection.transaction()?;
        let existing: Option<(i64, String)> = tx.query_row(
            "SELECT seq,digest FROM domain_events WHERE producer_id=?1 AND incarnation=?2 AND producer_seq=?3",
            params![source.producer_id, source.incarnation, source_seq], |row| Ok((row.get(0)?, row.get(1)?))).optional()?;
        if let Some((seq, original_digest)) = existing {
            if original_digest != incoming_digest {
                tx.execute("INSERT INTO integrity_incidents(producer_id,incarnation,producer_seq,original_digest,conflicting_digest,observed_utc) VALUES(?1,?2,?3,?4,?5,?6)",
                    params![source.producer_id, source.incarnation, source_seq, original_digest, incoming_digest, observed_utc])?;
                tx.commit()?;
                return Err(StoreError::SourceCollision {
                    producer_id: source.producer_id,
                    incarnation: source.incarnation,
                    sequence: source.sequence,
                });
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
        tx.execute("INSERT INTO domain_events(producer_id,incarnation,producer_seq,digest,observed_utc,payload) VALUES(?1,?2,?3,?4,?5,?6)",
            params![source.producer_id, source.incarnation, source_seq, incoming_digest, observed_utc, payload])?;
        let seq = tx.last_insert_rowid();
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
    use super::fixed_sqlite;
    #[test]
    fn wal_fix_version_gate() {
        for version in [3_044_006, 3_044_007, 3_050_007, 3_051_003, 3_052_000] {
            assert!(fixed_sqlite(version));
        }
        for version in [3_044_005, 3_045_000, 3_050_006, 3_051_002] {
            assert!(!fixed_sqlite(version));
        }
    }
}
