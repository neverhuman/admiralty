//! A small, headless process host with a durable local output journal.
//!
//! This prototype owns children only for the lifetime of this `SessionHost`.
//! Existing spool directories are never adopted or relaunched: after a host
//! restart, a launch whose result is unclear must be reconciled by an operator.
//!
//! It uses pipes rather than a PTY. Linux records `/proc` start time with the
//! PID; other platforms currently record no second process identity. Restart
//! adoption and durable process reattachment are intentionally unsupported.
//! A run ID reservation persists in the spool root, preventing a second launch
//! with that ID even if its original child outcome still needs reconciliation.
//! Capture, input and reader pages have hard prototype limits. Input timeout is
//! an unknown outcome, never permission to retry. Process-group escape, restart
//! adoption and continuity across host destruction remain unsupported.
//! After the direct child is reaped, descendant cleanup is unknown: historical
//! numeric process groups are never signalled, and retained pipes detach boundedly.

#[cfg(unix)]
pub mod daemon;

use admiralty_core::limits::bounded_json_size;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, ExitStatus, Stdio},
    sync::{Arc, Mutex, mpsc},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use thiserror::Error;

const JOURNAL_FILE: &str = "output.journal";
const INPUT_JOURNAL_FILE: &str = "input.journal";
const MANIFEST_FILE: &str = "run.json";
const MAX_RECORD: usize = 64 * 1024;
const MAX_CAPTURE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_JOURNAL_RECORDS: usize = 8192;
const MAX_TAIL_BYTES: usize = 256 * 1024;
pub const MAX_INPUT_BYTES: usize = 4096;
const MAX_INPUT_JOURNAL_BYTES: u64 = 1024 * 1024;
const MAX_SESSIONS: usize = 32;
const INPUT_TIMEOUT: Duration = Duration::from_millis(250);
const TERM_GRACE: Duration = Duration::from_millis(200);
const KILL_GRACE: Duration = Duration::from_secs(1);
const CAPTURE_DRAIN_GRACE: Duration = Duration::from_millis(500);

// One shared envelope for direct host, daemon and prepared provider launches.
pub const MAX_LAUNCH_ARG_BYTES: usize = 64 * 1024;
pub const MAX_LAUNCH_ARGV_BYTES: usize = 128 * 1024;
pub const MAX_LAUNCH_ARGS: usize = 128;
pub const MAX_LAUNCH_SPEC_BYTES: usize = 240 * 1024;
pub const MAX_LAUNCH_REQUEST_BYTES: usize = 256 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchSpec {
    pub run_id: String,
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub spool_dir: PathBuf,
}

/// Pure wire-envelope validation; filesystem existence/custody is separate.
/// The wrapper is the exact serialized daemon Launch request, including escaping.
pub fn validate_launch_envelope(spec: &LaunchSpec) -> Result<(), SessionError> {
    if !safe_run_id(&spec.run_id)
        || !spec.program.is_absolute()
        || !spec.cwd.is_absolute()
        || !spec.spool_dir.is_absolute()
        || [&spec.program, &spec.cwd, &spec.spool_dir]
            .iter()
            .any(|p| p.as_os_str().len() > 4096)
        || spec.args.len() > MAX_LAUNCH_ARGS
        || spec
            .args
            .iter()
            .any(|arg| arg.contains('\0') || arg.len() > MAX_LAUNCH_ARG_BYTES)
        || spec
            .args
            .iter()
            .try_fold(0_usize, |total, arg| total.checked_add(arg.len()))
            .is_none_or(|n| n > MAX_LAUNCH_ARGV_BYTES)
    {
        return Err(SessionError::InvalidSpec(
            "launch arguments or paths exceed shared envelope",
        ));
    }
    bounded_json_size(spec, MAX_LAUNCH_SPEC_BYTES)
        .map_err(|_| SessionError::InvalidSpec("serialized launch spec exceeds envelope"))?;
    #[derive(Serialize)]
    struct Request<'a> {
        command: &'static str,
        spec: &'a LaunchSpec,
    }
    bounded_json_size(
        &Request {
            command: "launch",
            spec,
        },
        MAX_LAUNCH_REQUEST_BYTES,
    )
    .map_err(|_| SessionError::InvalidSpec("serialized launch request exceeds envelope"))?;
    Ok(())
}

#[derive(Debug, Error)]
pub enum SessionError {
    #[error("invalid launch specification: {0}")]
    InvalidSpec(&'static str),
    #[error("run already exists or is ambiguous: {0}")]
    DuplicateRun(String),
    #[error("unknown session: {0}")]
    UnknownRun(String),
    #[error("automation input epoch is stale (expected {expected}, current {current})")]
    StaleInput { expected: u64, current: u64 },
    #[error("input outcome is unknown; the session requires reconciliation")]
    InputOutcomeUnknown,
    #[error("session input is already closed")]
    InputClosed,
    #[error("session I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("manifest serialization failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("session lock was poisoned")]
    Poisoned,
    #[error("session observation is busy; retry without changing identity")]
    Busy,
    #[error("child process did not provide {0}")]
    MissingPipe(&'static str),
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum OutputStream {
    Stdout,
    Stderr,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputChunk {
    /// Offset in the combined raw output byte stream.
    pub offset: u64,
    pub stream: OutputStream,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tail {
    pub chunks: Vec<OutputChunk>,
    pub next_offset: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputKind {
    Human,
    Automation {
        expected_epoch: u64,
    },
    /// Daemon delivery compares and advances the epoch atomically, preventing a
    /// disconnected client's retry from writing the same input twice.
    CompareAndAdvance {
        expected_epoch: u64,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionStatus {
    pub pid: u32,
    pub start_identity: Option<String>,
    pub exit: Option<(Option<i32>, bool)>,
    pub input_epoch: u64,
    pub input_outcome_unknown: bool,
    pub input_closed: bool,
    pub output_offset: u64,
    pub capture_failed: bool,
    pub capture_state: CaptureState,
    /// Fixed producer EOF boundary. A consumer must still drain through it.
    pub final_output_offset: Option<u64>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CaptureState {
    Capturing,
    Complete,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InputReceipt {
    pub input_epoch: u64,
    pub bytes_written: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
enum RunState {
    Launching,
    Running,
    Exited { code: Option<i32>, success: bool },
    Terminated { code: Option<i32>, success: bool },
    SpawnFailed { error: String },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ProcessIdentity {
    pid: u32,
    start_identity: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct RunManifest {
    version: u32,
    run_id: String,
    program: PathBuf,
    args: Vec<String>,
    cwd: PathBuf,
    state: RunState,
    process_identity: Option<ProcessIdentity>,
    input_epoch: u64,
    input_outcome_unknown: bool,
    #[serde(default)]
    input_closed: bool,
    #[serde(default)]
    capture_error: Option<String>,
}

#[derive(Serialize)]
struct InputRecord<'a> {
    epoch: u64,
    owner: &'a str,
    phase: &'a str,
    bytes: &'a [u8],
}

struct InputJournal(File);

impl InputJournal {
    fn create(path: PathBuf) -> io::Result<Self> {
        Ok(Self(
            OpenOptions::new().write(true).create_new(true).open(path)?,
        ))
    }

    fn record(
        &mut self,
        epoch: u64,
        owner: &'static str,
        phase: &'static str,
        bytes: &[u8],
    ) -> io::Result<()> {
        let data = serde_json::to_vec(&InputRecord {
            epoch,
            owner,
            phase,
            bytes,
        })
        .map_err(io::Error::other)?;
        if self.0.metadata()?.len() + data.len() as u64 + 1 > MAX_INPUT_JOURNAL_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::StorageFull,
                "input journal limit reached",
            ));
        }
        self.0.write_all(&data)?;
        self.0.write_all(b"\n")?;
        self.0.sync_data()
    }

    fn reserve_delivery(&self, bytes: &[u8]) -> io::Result<()> {
        // Reserve both the prepared record and the largest outcome record before
        // any possible input write; accounting includes JSON byte-array expansion.
        let record = InputRecord {
            epoch: u64::MAX,
            owner: "automation",
            phase: "outcome_unknown",
            bytes,
        };
        let maximum = serde_json::to_vec(&record).map_err(io::Error::other)?.len() as u64 + 1;
        if self.0.metadata()?.len() + maximum * 2 > MAX_INPUT_JOURNAL_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::StorageFull,
                "input journal cannot reserve delivery outcome",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct RecordIndex {
    raw_offset: u64,
    file_offset: u64,
    len: usize,
    stream: OutputStream,
}

struct Journal {
    file: File,
    path: PathBuf,
    next_offset: u64,
    file_bytes: u64,
    index: Vec<RecordIndex>,
    failure: Option<String>,
    stdout_eof: bool,
    stderr_eof: bool,
    final_output_offset: Option<u64>,
}

impl Journal {
    fn create(path: PathBuf) -> io::Result<Self> {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        Ok(Self {
            file,
            path,
            next_offset: 0,
            file_bytes: 0,
            index: Vec::new(),
            failure: None,
            stdout_eof: false,
            stderr_eof: false,
            final_output_offset: None,
        })
    }

    fn append(&mut self, stream: OutputStream, bytes: &[u8]) -> io::Result<()> {
        if let Some(error) = &self.failure {
            return Err(io::Error::other(error.clone()));
        }
        if match stream {
            OutputStream::Stdout => self.stdout_eof,
            OutputStream::Stderr => self.stderr_eof,
        } {
            self.failure = Some("output after recorded stream EOF".into());
            return Err(io::Error::other("output after recorded stream EOF"));
        }
        let records = bytes.len().div_ceil(MAX_RECORD);
        let needed = bytes.len() as u64 + records as u64 * 13;
        if self.file_bytes + needed > MAX_CAPTURE_BYTES
            || self.index.len() + records > MAX_JOURNAL_RECORDS
        {
            self.failure =
                Some("output capture limit reached; acknowledged prefix preserved".into());
            return Err(io::Error::new(
                io::ErrorKind::StorageFull,
                "output capture limit reached",
            ));
        }
        let result = self.append_records(stream, bytes);
        if let Err(error) = &result {
            self.failure = Some(format!("output capture failed: {error}"));
        }
        result
    }
    fn mark_eof(&mut self, stream: OutputStream) {
        match stream {
            OutputStream::Stdout => self.stdout_eof = true,
            OutputStream::Stderr => self.stderr_eof = true,
        }
        if self.stdout_eof
            && self.stderr_eof
            && self.failure.is_none()
            && self.final_output_offset.is_none()
        {
            self.final_output_offset = Some(self.next_offset);
        }
    }

    fn append_records(&mut self, stream: OutputStream, bytes: &[u8]) -> io::Result<()> {
        for part in bytes.chunks(MAX_RECORD) {
            let offset = self.next_offset;
            self.file.write_all(&offset.to_le_bytes())?;
            self.file.write_all(&[match stream {
                OutputStream::Stdout => 0,
                OutputStream::Stderr => 1,
            }])?;
            self.file.write_all(&(part.len() as u32).to_le_bytes())?;
            self.file.write_all(part)?;
            // Each complete record is a durability boundary. A torn final record
            // is detected by tail() and is never reported as acknowledged output.
            self.file.sync_data()?;
            self.index.push(RecordIndex {
                raw_offset: offset,
                file_offset: self.file_bytes,
                len: part.len(),
                stream,
            });
            self.file_bytes += 13 + part.len() as u64;
            self.next_offset += part.len() as u64;
        }
        Ok(())
    }

    fn tail(&self, from: u64, max_bytes: usize) -> io::Result<Tail> {
        let mut file = File::open(&self.path)?;
        if file.metadata()?.len() != self.file_bytes {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "output journal length differs from durable record boundary",
            ));
        }
        let mut chunks = Vec::new();
        let mut next_offset = from.min(self.next_offset);
        let mut remaining = max_bytes.min(MAX_TAIL_BYTES);
        let start = self
            .index
            .partition_point(|record| record.raw_offset + record.len as u64 <= from);
        for record in &self.index[start..] {
            if remaining == 0 {
                break;
            }
            file.seek(SeekFrom::Start(record.file_offset))?;
            let mut header = [0_u8; 13];
            file.read_exact(&mut header)?;
            let mut offset_bytes = [0_u8; 8];
            offset_bytes.copy_from_slice(&header[0..8]);
            let record_offset = u64::from_le_bytes(offset_bytes);
            let stream = match header[8] {
                0 => OutputStream::Stdout,
                1 => OutputStream::Stderr,
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "invalid stream tag",
                    ));
                }
            };
            let mut length_bytes = [0_u8; 4];
            length_bytes.copy_from_slice(&header[9..13]);
            let len = u32::from_le_bytes(length_bytes) as usize;
            if len == 0
                || len > MAX_RECORD
                || record_offset != record.raw_offset
                || len != record.len
                || stream != record.stream
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid journal record",
                ));
            }
            let start = from.saturating_sub(record_offset) as usize;
            let take = (len - start).min(remaining);
            file.seek(SeekFrom::Current(start as i64))?;
            let mut bytes = vec![0; take];
            file.read_exact(&mut bytes)?;
            chunks.push(OutputChunk {
                offset: record_offset + start as u64,
                stream,
                bytes,
            });
            remaining -= take;
            next_offset = record_offset + start as u64 + take as u64;
        }
        Ok(Tail {
            chunks,
            next_offset,
        })
    }
}

struct InputState {
    stdin: Option<ChildStdin>,
    epoch: u64,
    outcome_unknown: bool,
    closed: bool,
    manifest_path: PathBuf,
    manifest: RunManifest,
    journal: InputJournal,
}

struct Session {
    child: Mutex<Child>,
    input: Mutex<InputState>,
    journal: Arc<Mutex<Journal>>,
    readers: Mutex<Vec<JoinHandle<io::Result<()>>>>,
}

pub struct SessionHost {
    sessions: Mutex<HashMap<String, Arc<Session>>>,
}

impl Drop for Session {
    fn drop(&mut self) {
        let child = match self.child.get_mut() {
            Ok(child) => child,
            Err(error) => error.into_inner(),
        };
        let status = match child.try_wait() {
            Ok(Some(status)) => Some((status, false)),
            _ => terminate_owned_child(child)
                .ok()
                .map(|status| (status, true)),
        };
        let input = match self.input.get_mut() {
            Ok(input) => input,
            Err(error) => error.into_inner(),
        };
        drop(input.stdin.take());
        if let Some((status, terminated)) = status {
            input.manifest.state = if terminated {
                RunState::Terminated {
                    code: status.code(),
                    success: status.success(),
                }
            } else {
                RunState::Exited {
                    code: status.code(),
                    success: status.success(),
                }
            };
            let _ = write_manifest_replace(&input.manifest_path, &input.manifest);
        }
        let readers = match self.readers.get_mut() {
            Ok(readers) => readers,
            Err(error) => error.into_inner(),
        };
        // A reaped child's numeric PID/group may be reused. Never signal that
        // historical group to clean up inherited pipes: descendant custody is
        // unknown without a separately qualified identity boundary.
        let deadline = Instant::now() + CAPTURE_DRAIN_GRACE;
        while readers.iter().any(|reader| !reader.is_finished()) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        if readers.iter().any(|reader| !reader.is_finished()) {
            input.manifest.capture_error = Some(
                "capture EOF unavailable; descendant cleanup unknown after direct-child exit"
                    .into(),
            );
            let _ = write_manifest_replace(&input.manifest_path, &input.manifest);
        }
        for reader in readers.drain(..) {
            if reader.is_finished() {
                let _ = reader.join();
            }
            // Escaped descendants are explicitly unsupported; never block Drop
            // forever waiting for a pipe holder outside this owned process group.
        }
    }
}

impl Default for SessionHost {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionHost {
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
        }
    }

    /// Launches exactly `program` with the supplied argv. No shell command string
    /// is constructed. `spool_dir` must be a new directory whose parent exists.
    pub fn launch(&self, spec: LaunchSpec) -> Result<(), SessionError> {
        validate_spec(&spec)?;
        let mut sessions = self.sessions.lock().map_err(|_| SessionError::Poisoned)?;
        if sessions.len() >= MAX_SESSIONS {
            return Err(SessionError::InvalidSpec(
                "prototype session capacity reached",
            ));
        }
        if sessions.contains_key(&spec.run_id) {
            return Err(SessionError::DuplicateRun(spec.run_id));
        }
        reserve_run_id(&spec.spool_dir, &spec.run_id)?;
        create_spool_dir(&spec.spool_dir).map_err(|error| {
            if error.kind() == io::ErrorKind::AlreadyExists {
                SessionError::DuplicateRun(spec.run_id.clone())
            } else {
                SessionError::Io(error)
            }
        })?;

        let manifest_path = spec.spool_dir.join(MANIFEST_FILE);
        let mut manifest = RunManifest {
            version: 1,
            run_id: spec.run_id.clone(),
            program: spec.program.clone(),
            args: spec.args.clone(),
            cwd: spec.cwd.clone(),
            state: RunState::Launching,
            process_identity: None,
            input_epoch: 0,
            input_outcome_unknown: false,
            input_closed: false,
            capture_error: None,
        };
        write_manifest_new(&manifest_path, &manifest)?;
        let journal = Arc::new(Mutex::new(Journal::create(
            spec.spool_dir.join(JOURNAL_FILE),
        )?));
        let input_journal = InputJournal::create(spec.spool_dir.join(INPUT_JOURNAL_FILE))?;
        let mut command = Command::new(&spec.program);
        command
            .args(&spec.args)
            .current_dir(&spec.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                manifest.state = RunState::SpawnFailed {
                    error: error.to_string(),
                };
                write_manifest_replace(&manifest_path, &manifest)?;
                return Err(SessionError::Io(error));
            }
        };
        let (stdin, stdout, stderr) =
            match (child.stdin.take(), child.stdout.take(), child.stderr.take()) {
                (Some(stdin), Some(stdout), Some(stderr)) => (stdin, stdout, stderr),
                (stdin, stdout, stderr) => {
                    drop((stdin, stdout, stderr));
                    let _ = terminate_owned_child(&mut child);
                    return Err(SessionError::MissingPipe("child stdio"));
                }
            };
        let pid = child.id();
        manifest.state = RunState::Running;
        manifest.process_identity = Some(ProcessIdentity {
            pid,
            start_identity: process_start_identity(pid),
        });
        let readers = vec![
            capture_thread(stdout, OutputStream::Stdout, Arc::clone(&journal)),
            capture_thread(stderr, OutputStream::Stderr, Arc::clone(&journal)),
        ];
        if let Err(error) = write_manifest_replace(&manifest_path, &manifest) {
            let _ = terminate_owned_child(&mut child);
            let deadline = Instant::now() + CAPTURE_DRAIN_GRACE;
            while readers.iter().any(|reader| !reader.is_finished()) && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(5));
            }
            for reader in readers {
                if reader.is_finished() {
                    let _ = reader.join();
                }
            }
            return Err(error);
        }
        let session = Arc::new(Session {
            child: Mutex::new(child),
            input: Mutex::new(InputState {
                stdin: Some(stdin),
                epoch: 0,
                outcome_unknown: false,
                closed: false,
                manifest_path,
                manifest,
                journal: input_journal,
            }),
            journal,
            readers: Mutex::new(readers),
        });
        sessions.insert(spec.run_id, session);
        Ok(())
    }

    /// Writes bytes once under the session's ordered input lock. Human input
    /// advances the epoch; older queued automation then fails its epoch check.
    pub fn write_input(
        &self,
        run_id: &str,
        kind: InputKind,
        bytes: &[u8],
    ) -> Result<InputReceipt, SessionError> {
        if bytes.len() > MAX_INPUT_BYTES {
            return Err(SessionError::InvalidSpec(
                "input exceeds prototype 4096-byte limit",
            ));
        }
        let session = self.get(run_id)?;
        let mut input = session.input.lock().map_err(|_| SessionError::Poisoned)?;
        if input.outcome_unknown {
            return Err(SessionError::InputOutcomeUnknown);
        }
        if input.closed {
            return Err(SessionError::InputClosed);
        }
        input.journal.reserve_delivery(bytes)?;
        let owner = match kind {
            InputKind::Human => "human",
            InputKind::Automation { .. } | InputKind::CompareAndAdvance { .. } => "automation",
        };
        if let InputKind::CompareAndAdvance { expected_epoch } = kind
            && expected_epoch != input.epoch
        {
            return Err(SessionError::StaleInput {
                expected: expected_epoch,
                current: input.epoch,
            });
        }
        match kind {
            InputKind::Human | InputKind::CompareAndAdvance { .. } => {
                input.epoch = input
                    .epoch
                    .checked_add(1)
                    .ok_or(SessionError::InvalidSpec("input epoch exhausted"))?;
                input.manifest.input_epoch = input.epoch;
                write_manifest_replace(&input.manifest_path, &input.manifest)?;
            }
            InputKind::Automation { expected_epoch } if expected_epoch != input.epoch => {
                return Err(SessionError::StaleInput {
                    expected: expected_epoch,
                    current: input.epoch,
                });
            }
            InputKind::Automation { .. } => {}
        }
        let epoch = input.epoch;
        input.journal.record(epoch, owner, "prepared", bytes)?;
        let mut stdin = input
            .stdin
            .take()
            .ok_or(SessionError::InputOutcomeUnknown)?;
        let payload = bytes.to_vec();
        let (sender, receiver) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("admiralty-input".into())
            .spawn(move || {
                let result = stdin.write_all(&payload).and_then(|()| stdin.flush());
                let _ = sender.send((result, stdin));
            });
        if worker.is_err() {
            input.outcome_unknown = true;
            input.manifest.input_outcome_unknown = true;
            write_manifest_replace(&input.manifest_path, &input.manifest)?;
            return Err(SessionError::InputOutcomeUnknown);
        }
        match receiver.recv_timeout(INPUT_TIMEOUT) {
            Ok((Ok(()), stdin)) => {
                input.stdin = Some(stdin);
                if input
                    .journal
                    .record(epoch, owner, "delivered", bytes)
                    .is_err()
                {
                    input.outcome_unknown = true;
                    input.manifest.input_outcome_unknown = true;
                    let _ = write_manifest_replace(&input.manifest_path, &input.manifest);
                    return Err(SessionError::InputOutcomeUnknown);
                }
                Ok(InputReceipt {
                    input_epoch: epoch,
                    bytes_written: bytes.len(),
                })
            }
            result => {
                let error = match result {
                    Ok((Err(error), stdin)) => {
                        input.stdin = Some(stdin);
                        Some(error)
                    }
                    _ => None,
                };
                input.outcome_unknown = true;
                input.manifest.input_outcome_unknown = true;
                write_manifest_replace(&input.manifest_path, &input.manifest)?;
                input
                    .journal
                    .record(epoch, owner, "outcome_unknown", bytes)?;
                match error {
                    Some(error) => Err(SessionError::Io(error)),
                    None => Err(SessionError::InputOutcomeUnknown),
                }
            }
        }
    }

    pub fn tail(&self, run_id: &str, from: u64, max_bytes: usize) -> Result<Tail, SessionError> {
        let session = self.get(run_id)?;
        let journal = session.journal.lock().map_err(|_| SessionError::Poisoned)?;
        Ok(journal.tail(from, max_bytes)?)
    }

    /// Explicit EOF to the currently owned child's stdin. It writes no bytes,
    /// adds no newline, and repeating a successful close has no further effect.
    pub fn close_input(&self, run_id: &str) -> Result<InputReceipt, SessionError> {
        self.close_input_at(run_id, None)
    }
    /// Compare before EOF. Daemon argv-prompt clients require initial epoch0.
    /// Legacy direct Automation writes need not advance epochs; this comparison
    /// alone cannot attest untouched stdin for arbitrary direct-host callers.
    pub fn close_input_if_epoch(
        &self,
        run_id: &str,
        expected_epoch: u64,
    ) -> Result<InputReceipt, SessionError> {
        self.close_input_at(run_id, Some(expected_epoch))
    }

    fn close_input_at(
        &self,
        run_id: &str,
        expected_epoch: Option<u64>,
    ) -> Result<InputReceipt, SessionError> {
        let session = self.get(run_id)?;
        let mut input = session.input.lock().map_err(|_| SessionError::Poisoned)?;
        if input.outcome_unknown {
            return Err(SessionError::InputOutcomeUnknown);
        }
        if let Some(expected) = expected_epoch
            && expected != input.epoch
            && !(input.closed && expected.checked_add(1) == Some(input.epoch))
        {
            return Err(SessionError::StaleInput {
                expected,
                current: input.epoch,
            });
        }
        if input.closed {
            return Ok(InputReceipt {
                input_epoch: input.epoch,
                bytes_written: 0,
            });
        }
        input.journal.reserve_delivery(b"")?;
        let epoch = input
            .epoch
            .checked_add(1)
            .ok_or(SessionError::InvalidSpec("input epoch exhausted"))?;
        input.epoch = epoch;
        input.manifest.input_epoch = epoch;
        write_manifest_replace(&input.manifest_path, &input.manifest)?;
        input
            .journal
            .record(epoch, "automation", "eof_prepared", b"")?;
        let stdin = input
            .stdin
            .take()
            .ok_or(SessionError::InputOutcomeUnknown)?;
        drop(stdin);
        input.closed = true;
        input.manifest.input_closed = true;
        let result = write_manifest_replace(&input.manifest_path, &input.manifest).and_then(|()| {
            input
                .journal
                .record(epoch, "automation", "eof_closed", b"")
                .map_err(SessionError::Io)
        });
        if result.is_err() {
            input.outcome_unknown = true;
            input.manifest.input_outcome_unknown = true;
            let _ = write_manifest_replace(&input.manifest_path, &input.manifest);
            return Err(SessionError::InputOutcomeUnknown);
        }
        Ok(InputReceipt {
            input_epoch: epoch,
            bytes_written: 0,
        })
    }

    /// Observe the owned direct child without waiting for its exit or joining
    /// capture threads. Contended state returns Busy instead of queueing behind
    /// a launch/input operation. An exited PID is historical and grants no signal.
    pub fn status(&self, run_id: &str) -> Result<SessionStatus, SessionError> {
        fn observe_lock<T>(lock: &Mutex<T>) -> Result<std::sync::MutexGuard<'_, T>, SessionError> {
            lock.try_lock().map_err(|error| match error {
                std::sync::TryLockError::WouldBlock => SessionError::Busy,
                std::sync::TryLockError::Poisoned(_) => SessionError::Poisoned,
            })
        }
        let session = observe_lock(&self.sessions)?
            .get(run_id)
            .cloned()
            .ok_or_else(|| SessionError::UnknownRun(run_id.to_owned()))?;
        let mut child = observe_lock(&session.child)?;
        let input = observe_lock(&session.input)?;
        let readers = observe_lock(&session.readers)?;
        let mut journal = observe_lock(&session.journal)?;
        // A finished reader without its EOF marker failed/panicked. Joining is
        // unnecessary here: status never waits for a reader or filesystem sync.
        if journal.failure.is_none()
            && readers.iter().enumerate().any(|(index, reader)| {
                reader.is_finished()
                    && !match index {
                        0 => journal.stdout_eof,
                        1 => journal.stderr_eof,
                        _ => false,
                    }
            })
        {
            journal.failure = Some("output reader ended without recorded EOF".into());
        }
        let capture_failed = journal.failure.is_some() || input.manifest.capture_error.is_some();
        let capture_state = if capture_failed {
            CaptureState::Failed
        } else if journal.final_output_offset.is_some() {
            CaptureState::Complete
        } else {
            CaptureState::Capturing
        };
        let status = child.try_wait()?;
        Ok(SessionStatus {
            pid: child.id(),
            start_identity: input
                .manifest
                .process_identity
                .as_ref()
                .and_then(|identity| identity.start_identity.clone()),
            exit: status.map(|exit| (exit.code(), exit.success())),
            input_epoch: input.epoch,
            input_outcome_unknown: input.outcome_unknown,
            input_closed: input.closed,
            output_offset: journal.next_offset,
            capture_failed,
            capture_state,
            final_output_offset: journal.final_output_offset,
        })
    }

    pub fn wait(&self, run_id: &str) -> Result<ExitStatus, SessionError> {
        let session = self.get(run_id)?;
        let status = loop {
            if let Some(status) = session
                .child
                .lock()
                .map_err(|_| SessionError::Poisoned)?
                .try_wait()?
            {
                break status;
            }
            let failed = session
                .journal
                .lock()
                .map_err(|_| SessionError::Poisoned)?
                .failure
                .is_some();
            if failed {
                let mut child = session.child.lock().map_err(|_| SessionError::Poisoned)?;
                break terminate_owned_child(&mut child)?;
            }
            thread::sleep(Duration::from_millis(5));
        };
        {
            let mut input = session.input.lock().map_err(|_| SessionError::Poisoned)?;
            input.manifest.state = RunState::Exited {
                code: status.code(),
                success: status.success(),
            };
            write_manifest_replace(&input.manifest_path, &input.manifest)?;
        }
        self.finish_capture(&session)?;
        Ok(status)
    }

    pub fn terminate(&self, run_id: &str) -> Result<ExitStatus, SessionError> {
        let session = self.get(run_id)?;
        let status = {
            let mut child = session.child.lock().map_err(|_| SessionError::Poisoned)?;
            match child.try_wait()? {
                Some(status) => status,
                None => terminate_owned_child(&mut child)?,
            }
        };
        {
            let mut input = session.input.lock().map_err(|_| SessionError::Poisoned)?;
            input.manifest.state = RunState::Terminated {
                code: status.code(),
                success: status.success(),
            };
            write_manifest_replace(&input.manifest_path, &input.manifest)?;
        }
        self.finish_capture(&session)?;
        Ok(status)
    }

    fn finish_capture(&self, session: &Session) -> Result<(), SessionError> {
        let mut guard = session.readers.lock().map_err(|_| SessionError::Poisoned)?;
        let deadline = Instant::now() + CAPTURE_DRAIN_GRACE;
        while guard.iter().any(|reader| !reader.is_finished()) {
            if Instant::now() >= deadline {
                // Leave handles owned by the Session so Drop can perform cleanup.
                let mut input = session.input.lock().map_err(|_| SessionError::Poisoned)?;
                input.manifest.capture_error = Some(
                    "capture EOF unavailable; descendant process reconciliation required".into(),
                );
                write_manifest_replace(&input.manifest_path, &input.manifest)?;
                return Err(SessionError::Io(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "capture did not reach EOF; process-group escape is unsupported",
                )));
            }
            thread::sleep(Duration::from_millis(5));
        }
        let readers = std::mem::take(&mut *guard);
        let mut failure = None;
        for reader in readers {
            match reader.join() {
                Ok(Ok(())) => {}
                Ok(Err(error)) => failure = Some(error),
                Err(_) => failure = Some(io::Error::other("output capture thread panicked")),
            }
        }
        if let Some(error) = failure {
            let mut input = session.input.lock().map_err(|_| SessionError::Poisoned)?;
            input.manifest.capture_error = Some(error.to_string());
            write_manifest_replace(&input.manifest_path, &input.manifest)?;
            return Err(SessionError::Io(error));
        }
        let mut journal = session.journal.lock().map_err(|_| SessionError::Poisoned)?;
        if let Err(error) = journal.file.sync_all() {
            journal.failure = Some(format!("final output journal sync failed: {error}"));
            return Err(SessionError::Io(error));
        }
        Ok(())
    }

    fn get(&self, run_id: &str) -> Result<Arc<Session>, SessionError> {
        self.sessions
            .lock()
            .map_err(|_| SessionError::Poisoned)?
            .get(run_id)
            .cloned()
            .ok_or_else(|| SessionError::UnknownRun(run_id.to_owned()))
    }
}

#[cfg(unix)]
fn signal_owned_group(pid: u32, signal: &str) -> io::Result<bool> {
    use rustix::process::{Pid, Signal, kill_process_group};
    // Pid::INIT would invoke kill(-1, ...), not an owned process group. Never
    // allow zero/current-group, broadcast, narrowing casts or utility parsing.
    let group = i32::try_from(pid)
        .ok()
        .filter(|pid| *pid > 1)
        .and_then(Pid::from_raw)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "owned process group ID must exceed one and fit pid_t",
            )
        })?;
    let signal = match signal {
        "TERM" => Signal::TERM,
        "KILL" => Signal::KILL,
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "unsupported owned-group signal",
            ));
        }
    };
    match kill_process_group(group, signal) {
        Ok(()) => Ok(true),
        Err(rustix::io::Errno::SRCH) => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn poll_exit(child: &mut Child, grace: Duration) -> io::Result<Option<ExitStatus>> {
    let deadline = Instant::now() + grace;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        thread::sleep(Duration::from_millis(5));
    }
}

#[cfg(unix)]
fn terminate_owned_child(child: &mut Child) -> io::Result<ExitStatus> {
    terminate_owned_child_with_signal(child, signal_owned_group)
}

#[cfg(unix)]
fn terminate_owned_child_with_signal(
    child: &mut Child,
    mut signal: impl FnMut(u32, &str) -> io::Result<bool>,
) -> io::Result<ExitStatus> {
    // The Child mutex excludes another local reaper. A None observation leaves
    // the PID reserved by the live child or its eventual unreaped zombie.
    // Once try_wait returns Some, no numeric process-group signal is safe.
    if let Some(status) = child.try_wait()? {
        return Ok(status);
    }
    let pid = child.id();
    let _ = signal(pid, "TERM");
    // Standard Child offers no non-reaping wait. Retain its kernel identity for
    // this bounded grace, then signal the group before the first possible reap.
    thread::sleep(TERM_GRACE);
    let _ = signal(pid, "KILL");
    let kill_result = child.kill();
    if let Some(status) = poll_exit(child, KILL_GRACE)? {
        return Ok(status);
    }
    kill_result?;
    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        "owned child did not exit after KILL; ownership retained",
    ))
}

#[cfg(not(unix))]
fn terminate_owned_child(child: &mut Child) -> io::Result<ExitStatus> {
    child.kill()?;
    poll_exit(child, KILL_GRACE)?.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::TimedOut,
            "owned child did not exit after termination; ownership retained",
        )
    })
}

fn validate_spec(spec: &LaunchSpec) -> Result<(), SessionError> {
    validate_launch_envelope(spec)?;
    if !safe_run_id(&spec.run_id) {
        return Err(SessionError::InvalidSpec(
            "run_id must be a safe path component",
        ));
    }
    if spec.program.as_os_str().is_empty() {
        return Err(SessionError::InvalidSpec("program must not be empty"));
    }
    if !spec.cwd.is_absolute() || !spec.cwd.is_dir() {
        return Err(SessionError::InvalidSpec(
            "cwd must be an absolute existing directory",
        ));
    }
    if !spec.spool_dir.is_absolute() {
        return Err(SessionError::InvalidSpec("spool_dir must be absolute"));
    }
    if spec.spool_dir.exists() {
        return Err(SessionError::DuplicateRun(spec.run_id.clone()));
    }
    Ok(())
}

#[cfg(unix)]
fn create_spool_dir(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    let mut builder = fs::DirBuilder::new();
    builder.mode(0o700).create(path)
}

#[cfg(not(unix))]
fn create_spool_dir(path: &Path) -> io::Result<()> {
    fs::create_dir(path)
}

fn reserve_run_id(spool_dir: &Path, run_id: &str) -> Result<(), SessionError> {
    let parent = spool_dir
        .parent()
        .ok_or(SessionError::InvalidSpec("spool_dir must have a parent"))?;
    let reservation = parent.join(format!(".admiralty-run-{run_id}.lock"));
    let mut file = match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&reservation)
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            return Err(SessionError::DuplicateRun(run_id.to_owned()));
        }
        Err(error) => return Err(SessionError::Io(error)),
    };
    file.write_all(run_id.as_bytes())?;
    file.sync_all()?;
    File::open(parent)?.sync_all()?;
    Ok(())
}

fn safe_run_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn write_manifest_new(path: &Path, manifest: &RunManifest) -> Result<(), SessionError> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    serde_json::to_writer(&mut file, manifest)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    if let Some(parent) = path.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

fn write_manifest_replace(path: &Path, manifest: &RunManifest) -> Result<(), SessionError> {
    let temp = path.with_extension("json.tmp");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)?;
    serde_json::to_writer(&mut file, manifest)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    fs::rename(&temp, path)?;
    if let Some(parent) = path.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

fn capture_thread<R: Read + Send + 'static>(
    mut reader: R,
    stream: OutputStream,
    journal: Arc<Mutex<Journal>>,
) -> JoinHandle<io::Result<()>> {
    thread::spawn(move || {
        let mut buffer = [0_u8; 8192];
        let result: io::Result<()> = (|| loop {
            let count = reader.read(&mut buffer)?;
            if count == 0 {
                journal
                    .lock()
                    .map_err(|_| io::Error::other("output journal lock poisoned"))?
                    .mark_eof(stream);
                return Ok(());
            }
            journal
                .lock()
                .map_err(|_| io::Error::other("output journal lock poisoned"))?
                .append(stream, &buffer[..count])?;
        })();
        if let Err(error) = &result
            && let Ok(mut journal) = journal.lock()
            && journal.failure.is_none()
        {
            journal.failure = Some(format!("output reader failed: {error}"));
        }
        result
    })
}

#[cfg(target_os = "linux")]
fn process_start_identity(pid: u32) -> Option<String> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // comm is parenthesized and may contain spaces or ')'; parse after its last ')'.
    let rest = stat.rsplit_once(") ")?.1;
    rest.split_whitespace().nth(19).map(str::to_owned) // field 22: starttime
}

#[cfg(not(target_os = "linux"))]
fn process_start_identity(_pid: u32) -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn spec(temp: &TempDir, run_id: &str, args: &[&str]) -> LaunchSpec {
        LaunchSpec {
            run_id: run_id.to_owned(),
            program: PathBuf::from("/bin/sh"),
            args: args.iter().map(|s| (*s).to_owned()).collect(),
            cwd: temp.path().to_owned(),
            spool_dir: temp.path().join(format!("spool-{run_id}")),
        }
    }

    fn output(host: &SessionHost, id: &str) -> Vec<u8> {
        host.tail(id, 0, usize::MAX)
            .expect("session fixture setup or observation should succeed")
            .chunks
            .into_iter()
            .flat_map(|c| c.bytes)
            .collect()
    }

    fn await_output(host: &SessionHost, id: &str, expected: &[u8]) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while output(host, id) != expected {
            assert!(
                Instant::now() < deadline,
                "fixture output was not captured before deadline"
            );
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn partial_header_and_body_are_never_clean_eof() -> Result<(), Box<dyn std::error::Error>> {
        let temporary = TempDir::new()?;
        let path = temporary.path().join("torn.journal");
        let journal = Journal::create(path.clone())?;
        OpenOptions::new()
            .append(true)
            .open(&path)?
            .write_all(&[1, 2, 3])?;
        assert_eq!(
            journal
                .tail(0, 10)
                .expect_err("torn header must fail")
                .kind(),
            io::ErrorKind::UnexpectedEof
        );
        let path = temporary.path().join("torn-body.journal");
        let mut journal = Journal::create(path.clone())?;
        journal.append(OutputStream::Stdout, b"body")?;
        journal.file.set_len(journal.file_bytes - 1)?;
        assert_eq!(
            journal.tail(0, 10).expect_err("torn body must fail").kind(),
            io::ErrorKind::UnexpectedEof
        );
        Ok(())
    }

    #[test]
    fn tail_uses_index_and_caps_bytes_without_scanning_old_records()
    -> Result<(), Box<dyn std::error::Error>> {
        let temporary = TempDir::new()?;
        let path = temporary.path().join("indexed.journal");
        let mut journal = Journal::create(path.clone())?;
        journal.append(OutputStream::Stdout, b"a")?;
        journal.append(OutputStream::Stderr, b"b")?;
        let mut corrupt = OpenOptions::new().write(true).open(&path)?;
        corrupt.seek(SeekFrom::Start(8))?;
        corrupt.write_all(&[255])?;
        assert_eq!(journal.tail(1, 1)?.chunks[0].bytes, b"b");
        assert!(journal.tail(0, 1).is_err());
        let mut large = Journal::create(temporary.path().join("large.journal"))?;
        large.append(OutputStream::Stdout, &vec![b'x'; MAX_TAIL_BYTES + 50])?;
        let page = large.tail(0, usize::MAX)?;
        assert_eq!(page.next_offset, MAX_TAIL_BYTES as u64);
        assert_eq!(
            page.chunks.iter().map(|c| c.bytes.len()).sum::<usize>(),
            MAX_TAIL_BYTES
        );
        assert_eq!(
            large.tail(page.next_offset, usize::MAX)?.chunks[0]
                .bytes
                .len(),
            50
        );
        Ok(())
    }

    #[test]
    fn input_and_capture_limits_fail_before_silent_growth() -> Result<(), Box<dyn std::error::Error>>
    {
        let temporary = TempDir::new()?;
        let mut journal = Journal::create(temporary.path().join("bounded.journal"))?;
        assert_eq!(
            journal
                .append(
                    OutputStream::Stdout,
                    &vec![b'x'; MAX_CAPTURE_BYTES as usize]
                )
                .expect_err("capture budget must include framing")
                .kind(),
            io::ErrorKind::StorageFull
        );
        assert_eq!(journal.file.metadata()?.len(), 0);
        let mut input = InputJournal::create(temporary.path().join("input.journal"))?;
        input.0.set_len(MAX_INPUT_JOURNAL_BYTES - 1)?;
        assert!(input.reserve_delivery(b"x").is_err());
        assert!(input.record(0, "human", "prepared", b"x").is_err());
        let host = SessionHost::new();
        host.launch(spec(&temporary, "bounded-input", &["-c", "cat"]))?;
        assert!(matches!(
            host.write_input(
                "bounded-input",
                InputKind::Human,
                &vec![b'x'; MAX_INPUT_BYTES + 1]
            ),
            Err(SessionError::InvalidSpec(_))
        ));
        assert_eq!(
            host.get("bounded-input")?
                .input
                .lock()
                .map_err(|_| "poisoned fixture")?
                .epoch,
            0
        );
        host.terminate("bounded-input")?;
        Ok(())
    }

    #[test]
    fn termination_escalates_when_term_is_ignored_and_drop_cleans_owned_child()
    -> Result<(), Box<dyn std::error::Error>> {
        let temporary = TempDir::new()?;
        let host = SessionHost::new();
        host.launch(spec(
            &temporary,
            "ignores-term",
            &[
                "-c",
                "trap '' TERM; printf ready; while :; do sleep 1; done",
            ],
        ))?;
        await_output(&host, "ignores-term", b"ready");
        let started = Instant::now();
        assert!(!host.terminate("ignores-term")?.success());
        assert!(started.elapsed() < Duration::from_secs(2));
        let host = SessionHost::new();
        host.launch(spec(
            &temporary,
            "drop-cleanup",
            &[
                "-c",
                "trap '' TERM; printf owned; while :; do sleep 1; done",
            ],
        ))?;
        await_output(&host, "drop-cleanup", b"owned");
        let session = host.get("drop-cleanup")?;
        let pid = session.child.lock().map_err(|_| "poisoned fixture")?.id();
        let weak = Arc::downgrade(&session);
        drop(session);
        let started = Instant::now();
        drop(host);
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(
            weak.upgrade().is_none(),
            "host destruction must release its owned session"
        );
        assert!(
            !Command::new("/bin/kill")
                .args(["-0".to_owned(), pid.to_string()])
                .stderr(Stdio::null())
                .status()?
                .success(),
            "Drop must terminate and reap the owned fixture PID"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn already_reaped_cleanup_never_signals_historical_process_group()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut child = Command::new("/bin/sh").args(["-c", "exit 0"]).spawn()?;
        let reaped = child.wait()?;
        let mut signals = Vec::new();
        let observed = terminate_owned_child_with_signal(&mut child, |pid, signal| {
            signals.push((pid, signal.to_owned()));
            Ok(true)
        })?;
        assert_eq!(observed, reaped);
        assert!(
            signals.is_empty(),
            "reaped children cannot authorize numeric group signals"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn group_signals_complete_before_reaping_child_identity()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut child = Command::new("/bin/sh").args(["-c", "exit 0"]).spawn()?;
        let mut signals = Vec::new();
        // No actual group signal is sent. The direct fixture exits itself while
        // its Child retains the unreaped identity through TERM/KILL callbacks.
        terminate_owned_child_with_signal(&mut child, |pid, signal| {
            signals.push((pid, signal.to_owned()));
            Ok(true)
        })?;
        assert!(
            signals.is_empty()
                || signals
                    .iter()
                    .map(|(_, signal)| signal.as_str())
                    .collect::<Vec<_>>()
                    == ["TERM", "KILL"]
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn native_group_kill_reaches_term_ignoring_descendant_and_drains_pipes()
    -> Result<(), Box<dyn std::error::Error>> {
        let temporary = TempDir::new()?;
        let host = SessionHost::new();
        // Both shell and inherited child ignore TERM. The background child keeps
        // both capture pipes open, so killing only the direct child cannot pass.
        // Its bounded lifespan also prevents a regressed test leaving an orphan
        // indefinitely; it exceeds the unchanged termination/capture deadlines.
        host.launch(spec(
            &temporary,
            "native-group",
            &["-c", "trap '' TERM; sleep 5 & printf group-ready; wait"],
        ))?;
        await_output(&host, "native-group", b"group-ready");
        let started = Instant::now();
        assert!(!host.terminate("native-group")?.success());
        assert!(started.elapsed() < Duration::from_secs(2));
        let status = host.status("native-group")?;
        assert_eq!(status.capture_state, CaptureState::Complete);
        assert!(!status.capture_failed);
        assert_eq!(status.final_output_offset, Some(11));
        assert_eq!(output(&host, "native-group"), b"group-ready");
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn native_group_signal_rejects_broadcast_ids_and_unknown_signals_before_effects() {
        for pid in [0, 1, i32::MAX as u32 + 1, u32::MAX] {
            assert_eq!(
                signal_owned_group(pid, "KILL")
                    .expect_err("invalid ID must not signal")
                    .kind(),
                io::ErrorKind::InvalidInput
            );
        }
        // Signal validation happens before the system call; no PID is signalled.
        assert_eq!(
            signal_owned_group(2, "unexpected")
                .expect_err("unknown signal must not dispatch")
                .kind(),
            io::ErrorKind::InvalidInput
        );
    }

    #[test]
    fn nonreading_input_is_bounded_and_unknown_delivery_cannot_retry()
    -> Result<(), Box<dyn std::error::Error>> {
        let temporary = TempDir::new()?;
        let host = SessionHost::new();
        host.launch(spec(
            &temporary,
            "no-reader",
            &["-c", "printf ready; sleep 10"],
        ))?;
        await_output(&host, "no-reader", b"ready");
        let started = Instant::now();
        let mut blocked = false;
        for _ in 0..100 {
            match host.write_input("no-reader", InputKind::Human, &[b'x'; MAX_INPUT_BYTES]) {
                Ok(_) => {}
                Err(SessionError::InputOutcomeUnknown) => {
                    blocked = true;
                    assert!(matches!(
                        host.write_input("no-reader", InputKind::Human, b"retry"),
                        Err(SessionError::InputOutcomeUnknown)
                    ));
                    break;
                }
                Err(SessionError::Io(error)) if error.kind() == io::ErrorKind::StorageFull => {
                    blocked = true;
                    break;
                }
                Err(error) => return Err(error.into()),
            }
        }
        assert!(blocked, "unread input must hit timeout or journal budget");
        assert!(started.elapsed() < Duration::from_secs(10));
        host.terminate("no-reader")?;
        Ok(())
    }

    #[test]
    fn capture_reader_errors_remain_visible_to_owner() -> Result<(), Box<dyn std::error::Error>> {
        struct FailingReader;
        impl Read for FailingReader {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::other("injected read failure"))
            }
        }
        let temporary = TempDir::new()?;
        let journal = Arc::new(Mutex::new(Journal::create(
            temporary.path().join("failure.journal"),
        )?));
        let reader = capture_thread(FailingReader, OutputStream::Stdout, Arc::clone(&journal));
        assert!(
            reader
                .join()
                .map_err(|_| "capture fixture panicked")?
                .is_err()
        );
        assert!(
            journal
                .lock()
                .map_err(|_| "poisoned fixture")?
                .failure
                .is_some()
        );
        let mut journal = journal.lock().map_err(|_| "poisoned fixture")?;
        journal.mark_eof(OutputStream::Stderr);
        assert!(!journal.stdout_eof);
        assert_eq!(journal.final_output_offset, None);
        Ok(())
    }

    #[test]
    fn captures_split_stream_output_and_supports_offsets() {
        let temp = TempDir::new().expect("session fixture setup or observation should succeed");
        let host = SessionHost::new();
        host.launch(spec(
            &temp,
            "split",
            &["-c", "printf ab; sleep 0.02; printf cd >&2"],
        ))
        .expect("session fixture setup or observation should succeed");
        host.wait("split")
            .expect("session fixture setup or observation should succeed");
        let all = host
            .tail("split", 0, 100)
            .expect("session fixture setup or observation should succeed");
        assert_eq!(all.next_offset, 4);
        assert_eq!(all.chunks.iter().map(|c| c.bytes.len()).sum::<usize>(), 4);
        let first_page = host
            .tail("split", 0, 2)
            .expect("session fixture setup or observation should succeed");
        assert_eq!(first_page.next_offset, 2);
        let suffix = host
            .tail("split", first_page.next_offset, 100)
            .expect("session fixture setup or observation should succeed");
        assert_eq!(
            suffix.chunks.iter().map(|c| c.bytes.len()).sum::<usize>(),
            2
        );
    }

    #[test]
    fn quiet_child_completes_without_output() {
        let temp = TempDir::new().expect("session fixture setup or observation should succeed");
        let host = SessionHost::new();
        host.launch(spec(&temp, "quiet", &["-c", "sleep 0.02"]))
            .expect("session fixture setup or observation should succeed");
        host.wait("quiet")
            .expect("session fixture setup or observation should succeed");
        assert!(output(&host, "quiet").is_empty());
        let manifest: RunManifest = serde_json::from_slice(
            &fs::read(temp.path().join("spool-quiet").join(MANIFEST_FILE))
                .expect("session fixture setup or observation should succeed"),
        )
        .expect("session fixture setup or observation should succeed");
        let identity = manifest
            .process_identity
            .expect("session fixture setup or observation should succeed");
        assert!(identity.pid > 0);
        #[cfg(target_os = "linux")]
        assert!(identity.start_identity.is_some());
    }

    #[test]
    fn output_is_journaled_without_a_viewer_and_duplicate_spool_is_rejected() {
        let temp = TempDir::new().expect("session fixture setup or observation should succeed");
        let host = SessionHost::new();
        host.launch(spec(&temp, "unwatched", &["-c", "printf durable"]))
            .expect("session fixture setup or observation should succeed");
        host.wait("unwatched")
            .expect("session fixture setup or observation should succeed");
        assert_eq!(output(&host, "unwatched"), b"durable");
        let duplicate = LaunchSpec {
            spool_dir: temp.path().join("spool-unwatched"),
            ..spec(&temp, "again", &["-c", "true"])
        };
        assert!(matches!(
            host.launch(duplicate),
            Err(SessionError::DuplicateRun(_))
        ));
    }

    #[test]
    fn explicit_close_is_idempotent_and_status_never_waits_for_child_or_input_lock()
    -> Result<(), Box<dyn std::error::Error>> {
        let temporary = TempDir::new()?;
        let host = SessionHost::new();
        host.launch(spec(&temporary, "explicit-eof", &["-c", "cat"]))?;
        assert!(host.status("explicit-eof")?.exit.is_none());
        let session = host.get("explicit-eof")?;
        let input = session.input.lock().map_err(|_| "poisoned fixture input")?;
        assert!(matches!(
            host.status("explicit-eof"),
            Err(SessionError::Busy)
        ));
        drop(input);
        let readers = session
            .readers
            .lock()
            .map_err(|_| "poisoned fixture readers")?;
        assert!(matches!(
            host.status("explicit-eof"),
            Err(SessionError::Busy)
        ));
        drop(readers);
        let receipt = host.close_input("explicit-eof")?;
        assert_eq!(receipt.input_epoch, 1);
        assert_eq!(receipt.bytes_written, 0);
        assert_eq!(host.close_input("explicit-eof")?, receipt);
        assert!(host.wait("explicit-eof")?.success());
        let status = host.status("explicit-eof")?;
        assert_eq!(status.capture_state, CaptureState::Complete);
        assert_eq!(status.final_output_offset, Some(0));
        assert!(!status.capture_failed);
        assert!(output(&host, "explicit-eof").is_empty());
        Ok(())
    }

    #[test]
    fn shared_launch_envelope_counts_escaped_json_and_refuses_before_reservation()
    -> Result<(), Box<dyn std::error::Error>> {
        let temporary = TempDir::new()?;
        let mut launch = spec(&temporary, "too-large", &[]);
        launch.args = vec!["\u{1}".repeat(41 * 1024)];
        assert!(launch.args[0].len() < MAX_LAUNCH_ARG_BYTES);
        assert!(serde_json::to_vec(&launch)?.len() > MAX_LAUNCH_SPEC_BYTES);
        assert!(validate_launch_envelope(&launch).is_err());
        let host = SessionHost::new();
        assert!(matches!(
            host.launch(launch.clone()),
            Err(SessionError::InvalidSpec(_))
        ));
        assert!(!launch.spool_dir.exists());
        assert!(
            !launch
                .spool_dir
                .parent()
                .ok_or("parent absent")?
                .join(".admiralty-run-too-large.lock")
                .exists()
        );
        launch.args = vec!["x".repeat(48 * 1024); 3];
        assert!(validate_launch_envelope(&launch).is_err());
        launch.args = vec!["x".repeat(MAX_LAUNCH_ARG_BYTES + 1)];
        assert!(validate_launch_envelope(&launch).is_err());
        launch.args = vec!["x".repeat(MAX_LAUNCH_ARG_BYTES)];
        assert!(validate_launch_envelope(&launch).is_ok());
        Ok(())
    }

    #[test]
    fn human_epoch_invalidates_stale_automation_input() {
        let temp = TempDir::new().expect("session fixture setup or observation should succeed");
        let host = SessionHost::new();
        host.launch(spec(&temp, "input", &["-c", "cat"]))
            .expect("session fixture setup or observation should succeed");
        let human = host
            .write_input("input", InputKind::Human, b"H")
            .expect("session fixture setup or observation should succeed");
        assert_eq!(human.input_epoch, 1);
        assert!(matches!(
            host.close_input_if_epoch("input", 0),
            Err(SessionError::StaleInput { current: 1, .. })
        ));
        let owned = host.get("input").expect("owned fixture");
        assert!(!owned.input.lock().expect("fixture input lock").closed);
        let journal = fs::read_to_string(temp.path().join("spool-input").join(INPUT_JOURNAL_FILE))
            .expect("session fixture setup or observation should succeed");
        assert!(journal.contains("\"phase\":\"delivered\""));
        assert!(matches!(
            host.write_input("input", InputKind::Automation { expected_epoch: 0 }, b"A"),
            Err(SessionError::StaleInput { .. })
        ));
        await_output(&host, "input", b"H");
        host.terminate("input")
            .expect("session fixture setup or observation should succeed");
        assert_eq!(output(&host, "input"), b"H");
    }

    #[test]
    fn failed_pipe_write_is_marked_unknown_and_not_replayed() {
        let temp = TempDir::new().expect("session fixture setup or observation should succeed");
        let host = SessionHost::new();
        host.launch(spec(&temp, "closed", &["-c", "true"]))
            .expect("session fixture setup or observation should succeed");
        host.wait("closed")
            .expect("session fixture setup or observation should succeed");
        assert!(matches!(
            host.write_input("closed", InputKind::Human, b"once"),
            Err(SessionError::Io(_))
        ));
        assert!(matches!(
            host.write_input(
                "closed",
                InputKind::Automation { expected_epoch: 1 },
                b"retry"
            ),
            Err(SessionError::InputOutcomeUnknown)
        ));
        let manifest: RunManifest = serde_json::from_slice(
            &fs::read(temp.path().join("spool-closed").join(MANIFEST_FILE))
                .expect("session fixture setup or observation should succeed"),
        )
        .expect("session fixture setup or observation should succeed");
        assert!(manifest.input_outcome_unknown);
    }

    #[test]
    fn a_launch_result_gap_is_not_relaunched() {
        let temp = TempDir::new().expect("session fixture setup or observation should succeed");
        let host = SessionHost::new();
        let launch = spec(&temp, "unique", &["-c", "printf once"]);
        host.launch(launch.clone())
            .expect("session fixture setup or observation should succeed");
        host.wait("unique")
            .expect("session fixture setup or observation should succeed");
        let new_host = SessionHost::new();
        assert!(matches!(
            new_host.launch(launch),
            Err(SessionError::DuplicateRun(_))
        ));
        let mut alternate_spool = spec(&temp, "unique", &["-c", "printf duplicate"]);
        alternate_spool.spool_dir = temp.path().join("alternate-unique");
        assert!(matches!(
            new_host.launch(alternate_spool),
            Err(SessionError::DuplicateRun(_))
        ));
    }

    #[test]
    fn launch_uses_literal_argv_and_validates_run_ids() {
        let temp = TempDir::new().expect("session fixture setup or observation should succeed");
        let host = SessionHost::new();
        let mut launch = spec(
            &temp,
            "literal",
            &["-c", "printf '%s' \"$1\"", "sh", "a; touch marker"],
        );
        host.launch(launch.clone())
            .expect("session fixture setup or observation should succeed");
        host.wait("literal")
            .expect("session fixture setup or observation should succeed");
        assert_eq!(output(&host, "literal"), b"a; touch marker");
        launch.run_id = "../escape".to_owned();
        launch.spool_dir = temp.path().join("escape");
        assert!(matches!(
            host.launch(launch),
            Err(SessionError::InvalidSpec(_))
        ));
    }
}
