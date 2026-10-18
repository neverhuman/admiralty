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

use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, ExitStatus, Stdio},
    sync::{Arc, Mutex},
    thread::{self, JoinHandle},
};
use thiserror::Error;

const JOURNAL_FILE: &str = "output.journal";
const INPUT_JOURNAL_FILE: &str = "input.journal";
const MANIFEST_FILE: &str = "run.json";
const MAX_RECORD: usize = 64 * 1024;

#[derive(Clone, Debug)]
pub struct LaunchSpec {
    pub run_id: String,
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub spool_dir: PathBuf,
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
    #[error("session I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("manifest serialization failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("session lock was poisoned")]
    Poisoned,
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
    Automation { expected_epoch: u64 },
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
        serde_json::to_writer(
            &mut self.0,
            &InputRecord {
                epoch,
                owner,
                phase,
                bytes,
            },
        )
        .map_err(io::Error::other)?;
        self.0.write_all(b"\n")?;
        self.0.sync_data()
    }
}

struct Journal {
    file: File,
    path: PathBuf,
    next_offset: u64,
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
        })
    }

    fn append(&mut self, stream: OutputStream, bytes: &[u8]) -> io::Result<()> {
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
            self.next_offset += part.len() as u64;
        }
        Ok(())
    }

    fn tail(&self, from: u64, max_bytes: usize) -> io::Result<Tail> {
        let mut file = File::open(&self.path)?;
        let mut chunks = Vec::new();
        let mut journal_end = 0_u64;
        let mut next_offset = 0_u64;
        let mut remaining = max_bytes;
        loop {
            let mut header = [0_u8; 13];
            match file.read_exact(&mut header) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {
                    if file.stream_position()? == file.metadata()?.len() {
                        break;
                    }
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "truncated output journal header",
                    ));
                }
                Err(error) => return Err(error),
            }
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
            if len > MAX_RECORD || record_offset != journal_end {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid journal record",
                ));
            }
            let mut bytes = vec![0; len];
            file.read_exact(&mut bytes)?;
            let record_end = record_offset + len as u64;
            journal_end = record_end;
            if record_end > from && remaining > 0 {
                let start = from.saturating_sub(record_offset) as usize;
                let take = (len - start).min(remaining);
                chunks.push(OutputChunk {
                    offset: record_offset + start as u64,
                    stream,
                    bytes: bytes[start..start + take].to_vec(),
                });
                remaining -= take;
                next_offset = record_offset + start as u64 + take as u64;
            }
        }
        if next_offset == 0 {
            next_offset = from.min(journal_end);
        }
        Ok(Tail {
            chunks,
            next_offset,
        })
    }
}

struct InputState {
    stdin: ChildStdin,
    epoch: u64,
    outcome_unknown: bool,
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
            for reader in readers {
                let _ = reader.join();
            }
            return Err(error);
        }
        let session = Arc::new(Session {
            child: Mutex::new(child),
            input: Mutex::new(InputState {
                stdin,
                epoch: 0,
                outcome_unknown: false,
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
        let session = self.get(run_id)?;
        let mut input = session.input.lock().map_err(|_| SessionError::Poisoned)?;
        if input.outcome_unknown {
            return Err(SessionError::InputOutcomeUnknown);
        }
        let owner = match kind {
            InputKind::Human => "human",
            InputKind::Automation { .. } => "automation",
        };
        match kind {
            InputKind::Human => {
                input.epoch += 1;
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
        match input
            .stdin
            .write_all(bytes)
            .and_then(|()| input.stdin.flush())
        {
            Ok(()) => {
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
            Err(error) => {
                input.outcome_unknown = true;
                input.manifest.input_outcome_unknown = true;
                write_manifest_replace(&input.manifest_path, &input.manifest)?;
                input
                    .journal
                    .record(epoch, owner, "outcome_unknown", bytes)?;
                Err(SessionError::Io(error))
            }
        }
    }

    pub fn tail(&self, run_id: &str, from: u64, max_bytes: usize) -> Result<Tail, SessionError> {
        let session = self.get(run_id)?;
        let journal = session.journal.lock().map_err(|_| SessionError::Poisoned)?;
        Ok(journal.tail(from, max_bytes)?)
    }

    pub fn wait(&self, run_id: &str) -> Result<ExitStatus, SessionError> {
        let session = self.get(run_id)?;
        let status = session
            .child
            .lock()
            .map_err(|_| SessionError::Poisoned)?
            .wait()?;
        self.finish_capture(&session)?;
        let mut input = session.input.lock().map_err(|_| SessionError::Poisoned)?;
        input.manifest.state = RunState::Exited {
            code: status.code(),
            success: status.success(),
        };
        write_manifest_replace(&input.manifest_path, &input.manifest)?;
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
        self.finish_capture(&session)?;
        let mut input = session.input.lock().map_err(|_| SessionError::Poisoned)?;
        input.manifest.state = RunState::Terminated {
            code: status.code(),
            success: status.success(),
        };
        write_manifest_replace(&input.manifest_path, &input.manifest)?;
        Ok(status)
    }

    fn finish_capture(&self, session: &Session) -> Result<(), SessionError> {
        let readers =
            std::mem::take(&mut *session.readers.lock().map_err(|_| SessionError::Poisoned)?);
        for reader in readers {
            match reader.join() {
                Ok(Ok(())) => {}
                Ok(Err(error)) => return Err(SessionError::Io(error)),
                Err(_) => {
                    return Err(SessionError::Io(io::Error::other(
                        "output capture thread panicked",
                    )));
                }
            }
        }
        session
            .journal
            .lock()
            .map_err(|_| SessionError::Poisoned)?
            .file
            .sync_all()?;
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
fn terminate_owned_child(child: &mut Child) -> io::Result<ExitStatus> {
    let process_group = format!("-{}", child.id());
    let term = Command::new("/bin/kill")
        .args(["-TERM", "--", process_group.as_str()])
        .status()?;
    if !term.success() {
        child.kill()?;
    }
    let status = child.wait()?;
    // The group was created exclusively for this session. Remove descendants
    // that retained inherited pipe handles so capture can observe EOF.
    let _ = Command::new("/bin/kill")
        .args(["-KILL", "--", process_group.as_str()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    Ok(status)
}

#[cfg(not(unix))]
fn terminate_owned_child(child: &mut Child) -> io::Result<ExitStatus> {
    child.kill()?;
    child.wait()
}

fn validate_spec(spec: &LaunchSpec) -> Result<(), SessionError> {
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
        loop {
            let count = reader.read(&mut buffer)?;
            if count == 0 {
                return Ok(());
            }
            journal
                .lock()
                .map_err(|_| io::Error::other("output journal lock poisoned"))?
                .append(stream, &buffer[..count])?;
        }
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
            .unwrap()
            .chunks
            .into_iter()
            .flat_map(|c| c.bytes)
            .collect()
    }

    #[test]
    fn captures_split_stream_output_and_supports_offsets() {
        let temp = TempDir::new().unwrap();
        let host = SessionHost::new();
        host.launch(spec(
            &temp,
            "split",
            &["-c", "printf ab; sleep 0.02; printf cd >&2"],
        ))
        .unwrap();
        host.wait("split").unwrap();
        let all = host.tail("split", 0, 100).unwrap();
        assert_eq!(all.next_offset, 4);
        assert_eq!(all.chunks.iter().map(|c| c.bytes.len()).sum::<usize>(), 4);
        let first_page = host.tail("split", 0, 2).unwrap();
        assert_eq!(first_page.next_offset, 2);
        let suffix = host.tail("split", first_page.next_offset, 100).unwrap();
        assert_eq!(
            suffix.chunks.iter().map(|c| c.bytes.len()).sum::<usize>(),
            2
        );
    }

    #[test]
    fn quiet_child_completes_without_output() {
        let temp = TempDir::new().unwrap();
        let host = SessionHost::new();
        host.launch(spec(&temp, "quiet", &["-c", "sleep 0.02"]))
            .unwrap();
        host.wait("quiet").unwrap();
        assert!(output(&host, "quiet").is_empty());
        let manifest: RunManifest = serde_json::from_slice(
            &fs::read(temp.path().join("spool-quiet").join(MANIFEST_FILE)).unwrap(),
        )
        .unwrap();
        let identity = manifest.process_identity.unwrap();
        assert!(identity.pid > 0);
        #[cfg(target_os = "linux")]
        assert!(identity.start_identity.is_some());
    }

    #[test]
    fn output_is_journaled_without_a_viewer_and_duplicate_spool_is_rejected() {
        let temp = TempDir::new().unwrap();
        let host = SessionHost::new();
        host.launch(spec(&temp, "unwatched", &["-c", "printf durable"]))
            .unwrap();
        host.wait("unwatched").unwrap();
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
    fn human_epoch_invalidates_stale_automation_input() {
        let temp = TempDir::new().unwrap();
        let host = SessionHost::new();
        host.launch(spec(&temp, "input", &["-c", "cat"])).unwrap();
        let human = host.write_input("input", InputKind::Human, b"H").unwrap();
        assert_eq!(human.input_epoch, 1);
        let journal =
            fs::read_to_string(temp.path().join("spool-input").join(INPUT_JOURNAL_FILE)).unwrap();
        assert!(journal.contains("\"phase\":\"delivered\""));
        assert!(matches!(
            host.write_input("input", InputKind::Automation { expected_epoch: 0 }, b"A"),
            Err(SessionError::StaleInput { .. })
        ));
        host.terminate("input").unwrap();
        assert_eq!(output(&host, "input"), b"H");
    }

    #[test]
    fn failed_pipe_write_is_marked_unknown_and_not_replayed() {
        let temp = TempDir::new().unwrap();
        let host = SessionHost::new();
        host.launch(spec(&temp, "closed", &["-c", "true"])).unwrap();
        host.wait("closed").unwrap();
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
            &fs::read(temp.path().join("spool-closed").join(MANIFEST_FILE)).unwrap(),
        )
        .unwrap();
        assert!(manifest.input_outcome_unknown);
    }

    #[test]
    fn a_launch_result_gap_is_not_relaunched() {
        let temp = TempDir::new().unwrap();
        let host = SessionHost::new();
        let launch = spec(&temp, "unique", &["-c", "printf once"]);
        host.launch(launch.clone()).unwrap();
        host.wait("unique").unwrap();
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
        let temp = TempDir::new().unwrap();
        let host = SessionHost::new();
        let mut launch = spec(
            &temp,
            "literal",
            &["-c", "printf '%s' \"$1\"", "sh", "a; touch marker"],
        );
        host.launch(launch.clone()).unwrap();
        host.wait("literal").unwrap();
        assert_eq!(output(&host, "literal"), b"a; touch marker");
        launch.run_id = "../escape".to_owned();
        launch.spool_dir = temp.path().join("escape");
        assert!(matches!(
            host.launch(launch),
            Err(SessionError::InvalidSpec(_))
        ));
    }
}
