//! Opt-in, same-user Unix-socket process host. This is not fleet enrollment.
use super::{InputKind, LaunchSpec, SessionError, SessionHost, SessionStatus, safe_run_id};
use admiralty_core::limits::bounded_json_size;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::unix::{
        fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    path::{Component, Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

pub const MAX_REQUEST_BYTES: usize = super::MAX_LAUNCH_REQUEST_BYTES;
pub const MAX_RESPONSE_BYTES: usize = 128 * 1024;
const MAX_CONNECTIONS: usize = 8;
const MAX_TAIL_BYTES: usize = 8192;
const MAX_TAIL_CHUNKS: usize = 128;
const IO_DEADLINE: Duration = Duration::from_secs(2);

#[derive(Clone, Debug)]
pub struct DaemonConfig {
    pub socket: PathBuf,
    pub spool_root: PathBuf,
}

/// The private socket grants the same Unix user the ability to launch explicit
/// local programs. There is deliberately no command-string or terminate variant.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum DaemonRequest {
    Launch {
        spec: LaunchSpec,
    },
    Status {
        run_id: String,
    },
    Tail {
        run_id: String,
        from: String,
        max_bytes: u32,
    },
    Input {
        run_id: String,
        expected_epoch: String,
        bytes: Vec<u8>,
    },
    CloseInput {
        run_id: String,
        expected_epoch: String,
    },
}

#[derive(Debug, Serialize)]
pub struct DaemonResponse {
    pub protocol_version: u32,
    pub result: ResponseResult,
    native_execution_qualified: bool,
    execution_granted: bool,
    acceptance_granted: bool,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ResponseResult {
    Launched {
        run_id: String,
    },
    Status {
        run_id: String,
        status: Status,
    },
    Tail {
        run_id: String,
        chunks: Vec<Chunk>,
        next_offset: String,
    },
    InputAccepted {
        run_id: String,
        input_epoch: String,
        bytes_written: usize,
    },
    InputClosed {
        run_id: String,
        input_epoch: String,
    },
    Error {
        code: ErrorCode,
        current_epoch: Option<String>,
    },
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidFrame,
    InvalidRequest,
    UnknownRun,
    ReconciliationRequired,
    DuplicateRun,
    Busy,
    EpochMismatch,
    InputOutcomeUnknown,
    InputClosed,
    IoFailure,
}

#[derive(Debug, Serialize)]
pub struct Status {
    pub pid: u32,
    pub start_identity: Option<String>,
    pub state: &'static str,
    pub exit_code: Option<i32>,
    pub exit_success: Option<bool>,
    pub input_epoch: String,
    pub input_outcome_unknown: bool,
    pub input_closed: bool,
    pub output_offset: String,
    pub capture_failed: bool,
    pub capture_state: super::CaptureState,
    pub final_output_offset: Option<String>,
}
impl From<SessionStatus> for Status {
    fn from(value: SessionStatus) -> Self {
        Self {
            pid: value.pid,
            start_identity: value.start_identity,
            state: if value.exit.is_some() {
                "exited"
            } else {
                "running"
            },
            exit_code: value.exit.and_then(|exit| exit.0),
            exit_success: value.exit.map(|exit| exit.1),
            input_epoch: value.input_epoch.to_string(),
            input_outcome_unknown: value.input_outcome_unknown,
            input_closed: value.input_closed,
            output_offset: value.output_offset.to_string(),
            capture_failed: value.capture_failed,
            capture_state: value.capture_state,
            final_output_offset: value.final_output_offset.map(|n| n.to_string()),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct Chunk {
    pub offset: String,
    pub stream: super::OutputStream,
    pub bytes: Vec<u8>,
}

impl DaemonResponse {
    fn new(result: ResponseResult) -> Self {
        Self {
            protocol_version: 1,
            result,
            native_execution_qualified: false,
            execution_granted: false,
            acceptance_granted: false,
        }
    }
    fn error(code: ErrorCode) -> Self {
        Self::new(ResponseResult::Error {
            code,
            current_epoch: None,
        })
    }
}

#[derive(Clone)]
struct PrivateDirectory {
    path: PathBuf,
    dev: u64,
    ino: u64,
}
impl PrivateDirectory {
    fn open(path: &Path) -> io::Result<Self> {
        validate_absolute(path)?;
        let euid = rustix::process::geteuid().as_raw();
        // Reject every symlink component. Trusted root-owned sticky temporary
        // directories are allowed above the owned private leaf, never as leaf.
        let mut ancestor = PathBuf::new();
        for component in path.components() {
            ancestor.push(component.as_os_str());
            let metadata = fs::symlink_metadata(&ancestor)?;
            if !metadata.is_dir()
                || metadata.file_type().is_symlink()
                || (metadata.uid() != euid && metadata.uid() != 0)
                || (metadata.mode() & 0o022 != 0
                    && !(metadata.uid() == 0 && metadata.mode() & 0o1000 != 0))
            {
                return Err(unsafe_path());
            }
        }
        let metadata = fs::symlink_metadata(path)?;
        if metadata.uid() != euid || metadata.mode() & 0o777 != 0o700 {
            return Err(unsafe_path());
        }
        Ok(Self {
            path: path.to_owned(),
            dev: metadata.dev(),
            ino: metadata.ino(),
        })
    }
    fn check(&self) -> io::Result<()> {
        let current = Self::open(&self.path)?;
        if (current.dev, current.ino) != (self.dev, self.ino) {
            return Err(unsafe_path());
        }
        Ok(())
    }
}
fn unsafe_path() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "session paths must be owned private directories without symlinks",
    )
}
fn validate_absolute(path: &Path) -> io::Result<()> {
    if !path.is_absolute()
        || path.as_os_str().len() > 4096
        || path
            .components()
            .any(|part| !matches!(part, Component::RootDir | Component::Normal(_)))
    {
        return Err(unsafe_path());
    }
    Ok(())
}

struct State {
    host: SessionHost,
    spool: PrivateDirectory,
}
impl State {
    fn missing(&self, run_id: &str) -> DaemonResponse {
        // A reservation or spool left by another host is evidence of ambiguity,
        // not a process identity. Never read/adopt its manifest or launch again.
        let reserved = self
            .spool
            .path
            .join(format!(".admiralty-run-{run_id}.lock"));
        let run = self.spool.path.join(run_id);
        match (fs::symlink_metadata(reserved), fs::symlink_metadata(run)) {
            (Err(a), Err(b))
                if a.kind() == io::ErrorKind::NotFound && b.kind() == io::ErrorKind::NotFound =>
            {
                DaemonResponse::error(ErrorCode::UnknownRun)
            }
            _ => DaemonResponse::error(ErrorCode::ReconciliationRequired),
        }
    }
    fn session_error(&self, run_id: &str, error: SessionError) -> DaemonResponse {
        match error {
            SessionError::UnknownRun(_) => self.missing(run_id),
            SessionError::DuplicateRun(_) => DaemonResponse::error(ErrorCode::DuplicateRun),
            SessionError::Busy => DaemonResponse::error(ErrorCode::Busy),
            SessionError::StaleInput { current, .. } => {
                DaemonResponse::new(ResponseResult::Error {
                    code: ErrorCode::EpochMismatch,
                    current_epoch: Some(current.to_string()),
                })
            }
            SessionError::InputOutcomeUnknown => {
                DaemonResponse::error(ErrorCode::InputOutcomeUnknown)
            }
            SessionError::InputClosed => DaemonResponse::error(ErrorCode::InputClosed),
            SessionError::InvalidSpec(_) => DaemonResponse::error(ErrorCode::InvalidRequest),
            _ => DaemonResponse::error(ErrorCode::IoFailure),
        }
    }
    fn dispatch(&self, request: DaemonRequest) -> DaemonResponse {
        if self.spool.check().is_err() {
            return DaemonResponse::error(ErrorCode::IoFailure);
        }
        let run_id = match &request {
            DaemonRequest::Launch { spec } => &spec.run_id,
            DaemonRequest::Status { run_id }
            | DaemonRequest::Tail { run_id, .. }
            | DaemonRequest::Input { run_id, .. }
            | DaemonRequest::CloseInput { run_id, .. } => run_id,
        };
        if !safe_run_id(run_id) {
            return DaemonResponse::error(ErrorCode::InvalidRequest);
        }
        match request {
            DaemonRequest::Launch { spec } => {
                if !valid_launch(&spec, &self.spool.path) {
                    return DaemonResponse::error(ErrorCode::InvalidRequest);
                }
                let run_id = spec.run_id.clone();
                match self.host.launch(spec) {
                    Ok(()) => DaemonResponse::new(ResponseResult::Launched { run_id }),
                    Err(error) => self.session_error(&run_id, error),
                }
            }
            DaemonRequest::Status { run_id } => match self.host.status(&run_id) {
                Ok(status) => DaemonResponse::new(ResponseResult::Status {
                    run_id,
                    status: status.into(),
                }),
                Err(error) => self.session_error(&run_id, error),
            },
            DaemonRequest::Tail {
                run_id,
                from,
                max_bytes,
            } => {
                let Some(from) = counter(&from) else {
                    return DaemonResponse::error(ErrorCode::InvalidRequest);
                };
                if max_bytes == 0 || max_bytes as usize > MAX_TAIL_BYTES {
                    return DaemonResponse::error(ErrorCode::InvalidRequest);
                }
                let status = match self.host.status(&run_id) {
                    Ok(status) => status,
                    Err(error) => return self.session_error(&run_id, error),
                };
                if from > status.output_offset {
                    return DaemonResponse::error(ErrorCode::InvalidRequest);
                }
                match self.host.tail(&run_id, from, max_bytes as usize) {
                    Ok(tail) => {
                        let mut next_offset = from;
                        let chunks: Vec<_> = tail
                            .chunks
                            .into_iter()
                            .take(MAX_TAIL_CHUNKS)
                            .map(|chunk| {
                                next_offset = chunk.offset + chunk.bytes.len() as u64;
                                Chunk {
                                    offset: chunk.offset.to_string(),
                                    stream: chunk.stream,
                                    bytes: chunk.bytes,
                                }
                            })
                            .collect();
                        DaemonResponse::new(ResponseResult::Tail {
                            run_id,
                            chunks,
                            next_offset: next_offset.to_string(),
                        })
                    }
                    Err(error) => self.session_error(&run_id, error),
                }
            }
            DaemonRequest::Input {
                run_id,
                expected_epoch,
                bytes,
            } => {
                let Some(expected_epoch) = counter(&expected_epoch) else {
                    return DaemonResponse::error(ErrorCode::InvalidRequest);
                };
                if bytes.is_empty() || bytes.len() > super::MAX_INPUT_BYTES {
                    return DaemonResponse::error(ErrorCode::InvalidRequest);
                }
                match self.host.write_input(
                    &run_id,
                    InputKind::CompareAndAdvance { expected_epoch },
                    &bytes,
                ) {
                    Ok(receipt) => DaemonResponse::new(ResponseResult::InputAccepted {
                        run_id,
                        input_epoch: receipt.input_epoch.to_string(),
                        bytes_written: receipt.bytes_written,
                    }),
                    Err(error) => self.session_error(&run_id, error),
                }
            }
            DaemonRequest::CloseInput {
                run_id,
                expected_epoch,
            } => {
                let Some(expected_epoch) = counter(&expected_epoch) else {
                    return DaemonResponse::error(ErrorCode::InvalidRequest);
                };
                match self.host.close_input_if_epoch(&run_id, expected_epoch) {
                    Ok(receipt) => DaemonResponse::new(ResponseResult::InputClosed {
                        run_id,
                        input_epoch: receipt.input_epoch.to_string(),
                    }),
                    Err(error) => self.session_error(&run_id, error),
                }
            }
        }
    }
}
fn counter(value: &str) -> Option<u64> {
    if value.len() > 20 {
        return None;
    }
    value
        .parse::<u64>()
        .ok()
        .filter(|number| number.to_string() == value)
}
fn valid_launch(spec: &LaunchSpec, spool: &Path) -> bool {
    spec.spool_dir == spool.join(&spec.run_id) && super::validate_launch_envelope(spec).is_ok()
}

/// A single daemon retains one SessionHost across client disconnections.
/// Dropping it performs existing host cleanup; daemon restart cannot adopt runs.
pub struct SessionDaemon {
    listener: UnixListener,
    state: Arc<State>,
    socket_parent: PrivateDirectory,
    socket: PathBuf,
    socket_identity: (u64, u64),
    _spool_lock: File,
}
impl SessionDaemon {
    pub fn bind(config: DaemonConfig) -> io::Result<Self> {
        validate_absolute(&config.socket)?;
        let parent = config.socket.parent().ok_or_else(unsafe_path)?;
        let socket_parent = PrivateDirectory::open(parent)?;
        let spool = PrivateDirectory::open(&config.spool_root)?;
        match fs::symlink_metadata(&config.socket) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "socket path already exists; reconcile it explicitly",
                ));
            }
        }
        let spool_lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(
                (rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32,
            )
            .open(spool.path.join(".daemon.lock"))?;
        let metadata = spool_lock.metadata()?;
        if !metadata.is_file()
            || metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.mode() & 0o777 != 0o600
            || metadata.nlink() != 1
        {
            return Err(unsafe_path());
        }
        spool_lock.try_lock_exclusive()?;
        spool.check()?;
        socket_parent.check()?;
        let listener = UnixListener::bind(&config.socket)?;
        // Parent 0700 prevents a cross-user access window before chmod.
        fs::set_permissions(&config.socket, fs::Permissions::from_mode(0o600))?;
        let metadata = fs::symlink_metadata(&config.socket)?;
        if !metadata.file_type().is_socket()
            || metadata.uid() != rustix::process::geteuid().as_raw()
        {
            return Err(unsafe_path());
        }
        listener.set_nonblocking(true)?;
        Ok(Self {
            listener,
            state: Arc::new(State {
                host: SessionHost::new(),
                spool,
            }),
            socket_parent,
            socket: config.socket,
            socket_identity: (metadata.dev(), metadata.ino()),
            _spool_lock: spool_lock,
        })
    }
    pub fn serve(self) -> io::Result<()> {
        self.serve_until(&AtomicBool::new(false))
    }
    fn serve_until(self, stopped: &AtomicBool) -> io::Result<()> {
        let mut workers: Vec<JoinHandle<()>> = Vec::new();
        let outcome = loop {
            let mut index = 0;
            while index < workers.len() {
                if workers[index].is_finished() {
                    let _ = workers.swap_remove(index).join();
                } else {
                    index += 1;
                }
            }
            if stopped.load(Ordering::Acquire) {
                break Ok(());
            }
            match self.listener.accept() {
                Ok((mut stream, _)) => {
                    // BSD/macOS may inherit the listener's nonblocking flag;
                    // request I/O uses explicit absolute-deadline timeouts.
                    if let Err(error) = stream.set_nonblocking(false) {
                        break Err(error);
                    }
                    let metadata = match fs::symlink_metadata(&self.socket) {
                        Ok(metadata) => metadata,
                        Err(error) => break Err(error),
                    };
                    if self.socket_parent.check().is_err()
                        || (metadata.dev(), metadata.ino()) != self.socket_identity
                        || metadata.mode() & 0o777 != 0o600
                    {
                        break Err(unsafe_path());
                    }
                    if workers.len() >= MAX_CONNECTIONS {
                        let _ = write_frame(
                            &mut stream,
                            &DaemonResponse::error(ErrorCode::Busy),
                            Instant::now() + Duration::from_millis(50),
                        );
                        continue;
                    }
                    let state = Arc::clone(&self.state);
                    match thread::Builder::new()
                        .name("admiralty-session-client".into())
                        .spawn(move || {
                            let _ = handle_connection(&mut stream, &state);
                        }) {
                        Ok(worker) => workers.push(worker),
                        Err(error) => break Err(error),
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5))
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => break Err(error),
            }
        };
        for worker in workers {
            let _ = worker.join();
        }
        // Never unlink the path, even on shutdown or error. Its ownership/outcome
        // requires an operator decision before another daemon can bind it.
        outcome
    }
}

fn handle_connection(stream: &mut UnixStream, state: &State) -> io::Result<()> {
    handle_connection_with_dispatch(stream, IO_DEADLINE, |request| state.dispatch(request))
}
fn handle_connection_with_dispatch(
    stream: &mut UnixStream,
    io_budget: Duration,
    dispatch: impl FnOnce(DaemonRequest) -> DaemonResponse,
) -> io::Result<()> {
    let deadline = Instant::now() + io_budget;
    let frame = read_frame(stream, deadline);
    // Keep one aggregate socket-I/O budget, including partial reads/writes,
    // but do not charge dispatch's filesystem sync or process-spawn time to it.
    // Those OS calls can finish after the read deadline; discarding their
    // completed mutation receipt solely for that reason creates ambiguity.
    // Refuse dispatch if reading already exhausted the available I/O budget.
    let response_budget = remaining(deadline)?;
    let response = match frame {
        Ok(bytes) => match serde_json::from_slice::<DaemonRequest>(&bytes) {
            Ok(request) => dispatch(request),
            Err(_) => DaemonResponse::error(ErrorCode::InvalidRequest),
        },
        Err(error) if error.kind() == io::ErrorKind::InvalidData => {
            DaemonResponse::error(ErrorCode::InvalidFrame)
        }
        Err(error) => return Err(error),
    };
    write_frame(stream, &response, Instant::now() + response_budget)
}
fn remaining(deadline: Instant) -> io::Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|duration| !duration.is_zero())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::TimedOut,
                "session connection deadline exceeded",
            )
        })
}
fn read_exact_deadline(
    stream: &mut UnixStream,
    mut buffer: &mut [u8],
    deadline: Instant,
) -> io::Result<()> {
    while !buffer.is_empty() {
        stream.set_read_timeout(Some(remaining(deadline)?))?;
        match stream.read(buffer) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "incomplete session frame",
                ));
            }
            Ok(count) => {
                buffer = &mut buffer[count..];
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}
fn read_frame(stream: &mut UnixStream, deadline: Instant) -> io::Result<Vec<u8>> {
    let mut header = [0; 4];
    read_exact_deadline(stream, &mut header, deadline)?;
    let length = u32::from_be_bytes(header) as usize;
    if length == 0 || length > MAX_REQUEST_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "session request frame exceeds bounds",
        ));
    }
    let mut bytes = vec![0; length];
    read_exact_deadline(stream, &mut bytes, deadline)?;
    Ok(bytes)
}
fn write_frame(
    stream: &mut UnixStream,
    response: &DaemonResponse,
    deadline: Instant,
) -> io::Result<()> {
    bounded_json_size(response, MAX_RESPONSE_BYTES).map_err(io::Error::other)?;
    let bytes = serde_json::to_vec(response).map_err(io::Error::other)?;
    let mut frame = Vec::with_capacity(4 + bytes.len());
    frame.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    frame.extend_from_slice(&bytes);
    let mut remaining_bytes = frame.as_slice();
    while !remaining_bytes.is_empty() {
        stream.set_write_timeout(Some(remaining(deadline)?))?;
        match stream.write(remaining_bytes) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "session frame write failed",
                ));
            }
            Ok(count) => remaining_bytes = &remaining_bytes[count..],
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::os::unix::fs::DirBuilderExt;

    struct Fixture {
        _temporary: tempfile::TempDir,
        root: PathBuf,
        spool: PathBuf,
        socket: PathBuf,
        stop: Arc<AtomicBool>,
        worker: Option<JoinHandle<io::Result<()>>>,
    }
    impl Fixture {
        fn new() -> io::Result<Self> {
            let temporary = tempfile::Builder::new()
                .prefix("as")
                .tempdir_in(fs::canonicalize(std::env::temp_dir())?)?;
            let root = temporary.path().to_owned();
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
            let spool = root.join("r");
            fs::DirBuilder::new().mode(0o700).create(&spool)?;
            let socket = root.join("s");
            let daemon = SessionDaemon::bind(DaemonConfig {
                socket: socket.clone(),
                spool_root: spool.clone(),
            })?;
            let stop = Arc::new(AtomicBool::new(false));
            let signal = Arc::clone(&stop);
            let worker = thread::spawn(move || daemon.serve_until(&signal));
            Ok(Self {
                _temporary: temporary,
                root,
                spool,
                socket,
                stop,
                worker: Some(worker),
            })
        }
        fn stop(&mut self) -> io::Result<()> {
            self.stop.store(true, Ordering::Release);
            if let Some(worker) = self.worker.take() {
                worker
                    .join()
                    .map_err(|_| io::Error::other("daemon fixture panicked"))??;
            }
            Ok(())
        }
        fn launch(&self, run: &str) -> Value {
            json!({"command":"launch","spec":{
                "run_id":run,"program":"/bin/cat","args":[],"cwd":self.root,"spool_dir":self.spool.join(run)
            }})
        }
        fn request(&self, request: &Value) -> io::Result<Value> {
            exchange(&self.socket, request)
        }
        fn status(&self, run: &str) -> io::Result<Value> {
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                let value = self.request(&json!({"command":"status","run_id":run}))?;
                if value["result"]["code"] != "busy" {
                    return Ok(value);
                }
                if Instant::now() > deadline {
                    return Err(io::Error::other("fixture status stayed busy"));
                }
                thread::sleep(Duration::from_millis(5));
            }
        }
        fn await_offset(&self, run: &str, offset: &str) -> io::Result<Value> {
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                let value = self.status(run)?;
                if value["result"]["status"]["output_offset"] == offset {
                    return Ok(value);
                }
                if Instant::now() > deadline {
                    return Err(io::Error::other("fixture output missing"));
                }
                thread::sleep(Duration::from_millis(5));
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = self.stop();
        }
    }
    fn read_response(stream: &mut UnixStream) -> io::Result<Value> {
        stream.set_read_timeout(Some(Duration::from_secs(3)))?;
        let mut header = [0; 4];
        stream.read_exact(&mut header)?;
        let size = u32::from_be_bytes(header) as usize;
        assert!(size <= MAX_RESPONSE_BYTES);
        let mut body = vec![0; size];
        stream.read_exact(&mut body)?;
        let value: Value = serde_json::from_slice(&body).map_err(io::Error::other)?;
        assert_eq!(value["protocol_version"], 1);
        assert_eq!(value["native_execution_qualified"], false);
        assert_eq!(value["execution_granted"], false);
        assert_eq!(value["acceptance_granted"], false);
        Ok(value)
    }
    fn send(stream: &mut UnixStream, request: &Value) -> io::Result<()> {
        let body = serde_json::to_vec(request).map_err(io::Error::other)?;
        stream.write_all(&(body.len() as u32).to_be_bytes())?;
        stream.write_all(&body)
    }
    fn exchange(socket: &Path, request: &Value) -> io::Result<Value> {
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut stream = loop {
            match UnixStream::connect(socket) {
                Ok(stream) => break stream,
                Err(error)
                    if error.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline =>
                {
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) => return Err(error),
            }
        };
        send(&mut stream, request)?;
        read_response(&mut stream)
    }
    fn tail_bytes(value: &Value) -> Vec<u8> {
        value["result"]["chunks"]
            .as_array()
            .expect("tail fixture chunks")
            .iter()
            .flat_map(|chunk| {
                chunk["bytes"]
                    .as_array()
                    .expect("fixture bytes")
                    .iter()
                    .map(|byte| byte.as_u64().expect("fixture byte") as u8)
            })
            .collect()
    }

    #[test]
    fn socket_reconnect_retains_same_actual_child_and_continuous_offsets() -> io::Result<()> {
        let fixture = Fixture::new()?;
        assert_eq!(
            fixture.request(&fixture.launch("owned"))?["result"]["kind"],
            "launched"
        );
        let first = fixture.status("owned")?;
        assert_eq!(first["result"]["status"]["state"], "running");
        let identity = (
            &first["result"]["status"]["pid"],
            &first["result"]["status"]["start_identity"],
        );
        #[cfg(target_os = "linux")]
        assert!(identity.1.is_string());
        // The request client disconnects after admission. No output viewer is
        // connected while the daemon captures the owned child's output.
        let accepted = fixture.request(
            &json!({"command":"input","run_id":"owned","expected_epoch":"0","bytes":[97,98]}),
        )?;
        assert_eq!(accepted["result"]["input_epoch"], "1");
        let second = fixture.await_offset("owned", "2")?;
        assert_eq!(second["result"]["status"]["input_epoch"], "1");
        assert_eq!(identity.0, &second["result"]["status"]["pid"]);
        assert_eq!(identity.1, &second["result"]["status"]["start_identity"]);
        assert_eq!(
            fixture.request(
                &json!({"command":"input","run_id":"owned","expected_epoch":"0","bytes":[97,98]})
            )?["result"]["code"],
            "epoch_mismatch"
        );
        assert_eq!(
            fixture.request(
                &json!({"command":"input","run_id":"owned","expected_epoch":"1","bytes":[99,100]})
            )?["result"]["input_epoch"],
            "2"
        );
        fixture.await_offset("owned", "4")?;
        let prefix = fixture
            .request(&json!({"command":"tail","run_id":"owned","from":"0","max_bytes":2}))?;
        assert_eq!(tail_bytes(&prefix), b"ab");
        assert_eq!(prefix["result"]["next_offset"], "2");
        let suffix = fixture
            .request(&json!({"command":"tail","run_id":"owned","from":"2","max_bytes":8192}))?;
        assert_eq!(tail_bytes(&suffix), b"cd");
        assert_eq!(suffix["result"]["next_offset"], "4");
        Ok(())
    }

    #[test]
    fn explicit_eof_is_epoch_checked_idempotent_and_adds_no_bytes() -> io::Result<()> {
        let fixture = Fixture::new()?;
        fixture.request(&fixture.launch("eof"))?;
        assert_eq!(
            fixture
                .request(&json!({"command":"close_input","run_id":"eof","expected_epoch":"9"}))?["result"]
                ["code"],
            "epoch_mismatch"
        );
        fixture.request(
            &json!({"command":"input","run_id":"eof","expected_epoch":"0","bytes":[120]}),
        )?;
        fixture.await_offset("eof", "1")?;
        let request = json!({"command":"close_input","run_id":"eof","expected_epoch":"1"});
        let first = fixture.request(&request)?;
        assert_eq!(first["result"]["kind"], "input_closed");
        assert_eq!(first["result"]["input_epoch"], "2");
        assert_eq!(fixture.request(&request)?, first);
        assert_eq!(
            fixture.request(
                &json!({"command":"input","run_id":"eof","expected_epoch":"2","bytes":[10]})
            )?["result"]["code"],
            "input_closed"
        );
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let status = fixture.status("eof")?;
            if status["result"]["status"]["state"] == "exited" {
                assert_eq!(status["result"]["status"]["input_closed"], true);
                assert_eq!(status["result"]["status"]["exit_success"], true);
                break;
            }
            assert!(
                Instant::now() < deadline,
                "fake child must exit on explicit EOF"
            );
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            tail_bytes(
                &fixture.request(
                    &json!({"command":"tail","run_id":"eof","from":"0","max_bytes":8192})
                )?
            ),
            b"x"
        );
        let journal = fs::read_to_string(fixture.spool.join("eof/input.journal"))?;
        assert_eq!(journal.matches("eof_closed").count(), 1);
        Ok(())
    }

    #[test]
    fn completed_dispatch_keeps_its_receipt_when_sync_exceeds_socket_io_budget() -> io::Result<()> {
        let (mut client, mut server) = UnixStream::pair()?;
        let request = json!({"command":"close_input","run_id":"slow-sync","expected_epoch":"1"});
        // Queue the complete frame before starting the handler: this test
        // exercises a slow operation, not an idle or slow request client.
        send(&mut client, &request)?;
        let io_budget = Duration::from_millis(50);
        let worker = thread::spawn(move || {
            let mut dispatches = 0;
            handle_connection_with_dispatch(&mut server, io_budget, |request| {
                dispatches += 1;
                let DaemonRequest::CloseInput {
                    run_id,
                    expected_epoch,
                } = request
                else {
                    panic!("fixture must dispatch the EOF request");
                };
                assert_eq!(expected_epoch, "1");
                // A durable dispatch can legitimately outlast the socket-I/O
                // budget. The previous shared wall deadline dropped this
                // response even though dispatch had already completed.
                thread::sleep(io_budget * 2);
                DaemonResponse::new(ResponseResult::InputClosed {
                    run_id,
                    input_epoch: "2".into(),
                })
            })?;
            assert_eq!(dispatches, 1, "a lost receipt must never replay dispatch");
            Ok::<_, io::Error>(())
        });
        let response = read_response(&mut client);
        let handled = worker
            .join()
            .map_err(|_| io::Error::other("delayed dispatch fixture panicked"))?;
        handled?;
        let response = response?;
        assert_eq!(response["result"]["kind"], "input_closed");
        assert_eq!(response["result"]["run_id"], "slow-sync");
        assert_eq!(response["result"]["input_epoch"], "2");
        Ok(())
    }

    #[test]
    fn run_ids_launch_scope_and_replay_collisions_never_launch_twice() -> io::Result<()> {
        let fixture = Fixture::new()?;
        assert_eq!(fixture.status("missing")?["result"]["code"], "unknown_run");
        assert_eq!(
            fixture.status("../outside")?["result"]["code"],
            "invalid_request"
        );
        let mut request = fixture.launch("scope");
        request["spec"]["spool_dir"] = json!(fixture.root.join("outside"));
        assert_eq!(
            fixture.request(&request)?["result"]["code"],
            "invalid_request"
        );
        assert!(!fixture.root.join("outside").exists());
        request = fixture.launch("scope");
        fixture.request(&request)?;
        let first = fixture.status("scope")?;
        let manifest = fs::read(fixture.spool.join("scope/run.json"))?;
        assert_eq!(
            fixture.request(&request)?["result"]["code"],
            "duplicate_run"
        );
        request["spec"]["args"] = json!(["--different-argv"]);
        assert_eq!(
            fixture.request(&request)?["result"]["code"],
            "duplicate_run"
        );
        assert_eq!(fs::read(fixture.spool.join("scope/run.json"))?, manifest);
        assert_eq!(
            first["result"]["status"]["pid"],
            fixture.status("scope")?["result"]["status"]["pid"]
        );
        assert_eq!(fixture.request(&json!({"command":"input","run_id":"scope","expected_epoch":"18446744073709551615","bytes":[120]}))?["result"]["code"], "epoch_mismatch");
        assert_eq!(
            fixture
                .request(&json!({"command":"tail","run_id":"scope","from":"1","max_bytes":2}))?["result"]
                ["code"],
            "invalid_request"
        );
        assert_eq!(
            fixture
                .request(&json!({"command":"tail","run_id":"scope","from":"0","max_bytes":8193}))?
                ["result"]["code"],
            "invalid_request"
        );
        Ok(())
    }

    #[test]
    fn frame_admission_and_closed_schema_reject_untrusted_fields() -> io::Result<()> {
        let fixture = Fixture::new()?;
        for length in [0, (MAX_REQUEST_BYTES + 1) as u32, u32::MAX] {
            let mut stream = UnixStream::connect(&fixture.socket)?;
            stream.write_all(&length.to_be_bytes())?;
            assert_eq!(
                read_response(&mut stream)?["result"]["code"],
                "invalid_frame"
            );
        }
        assert_eq!(
            fixture.request(
                &json!({"command":"status","run_id":"missing","execution_granted":true})
            )?["result"]["code"],
            "invalid_request"
        );
        assert_eq!(
            fixture.request(&json!({"command":"terminate","run_id":"missing"}))?["result"]["code"],
            "invalid_request"
        );
        assert_eq!(
            fixture.request(
                &json!({"command":"input","run_id":"missing","expected_epoch":0,"bytes":[120]})
            )?["result"]["code"],
            "invalid_request"
        );
        Ok(())
    }

    #[test]
    fn private_socket_and_root_checks_refuse_unsafe_or_existing_paths() -> io::Result<()> {
        let mut fixture = Fixture::new()?;
        assert_eq!(fs::symlink_metadata(&fixture.socket)?.mode() & 0o777, 0o600);
        assert_eq!(fs::symlink_metadata(&fixture.spool)?.mode() & 0o777, 0o700);
        assert!(
            SessionDaemon::bind(DaemonConfig {
                socket: fixture.socket.clone(),
                spool_root: fixture.spool.clone()
            })
            .is_err()
        );
        assert!(
            SessionDaemon::bind(DaemonConfig {
                socket: fixture.root.join("other"),
                spool_root: fixture.spool.clone()
            })
            .is_err(),
            "same spool cannot have two live hosts"
        );
        fixture.stop()?;
        assert!(
            fixture.socket.exists(),
            "daemon must not unlink a socket on shutdown"
        );
        let unsafe_root = fixture.root.join("public");
        fs::create_dir(&unsafe_root)?;
        fs::set_permissions(&unsafe_root, fs::Permissions::from_mode(0o755))?;
        assert!(
            SessionDaemon::bind(DaemonConfig {
                socket: fixture.root.join("new"),
                spool_root: unsafe_root.clone()
            })
            .is_err()
        );
        assert!(
            SessionDaemon::bind(DaemonConfig {
                socket: unsafe_root.join("new"),
                spool_root: fixture.spool.clone()
            })
            .is_err()
        );
        let alias = fixture.root.join("alias");
        std::os::unix::fs::symlink(&fixture.spool, &alias)?;
        assert!(
            SessionDaemon::bind(DaemonConfig {
                socket: fixture.root.join("new"),
                spool_root: alias.clone()
            })
            .is_err()
        );
        assert!(
            SessionDaemon::bind(DaemonConfig {
                socket: alias.join("new"),
                spool_root: fixture.spool.clone()
            })
            .is_err()
        );
        fs::write(fixture.root.join("foreign"), b"preserve")?;
        assert!(
            SessionDaemon::bind(DaemonConfig {
                socket: fixture.root.join("foreign"),
                spool_root: fixture.spool.clone()
            })
            .is_err()
        );
        assert_eq!(fs::read(fixture.root.join("foreign"))?, b"preserve");
        Ok(())
    }

    #[test]
    fn restarted_daemon_reports_ambiguous_run_and_never_adopts_or_relaunches() -> io::Result<()> {
        let mut fixture = Fixture::new()?;
        fixture.request(&fixture.launch("prior"))?;
        fixture.stop()?;
        let journal = fs::read(fixture.spool.join("prior/output.journal"))?;
        let daemon = SessionDaemon::bind(DaemonConfig {
            socket: fixture.root.join("s2"),
            spool_root: fixture.spool.clone(),
        })?;
        assert_eq!(
            serde_json::to_value(daemon.state.dispatch(DaemonRequest::Status {
                run_id: "prior".into()
            }))
            .map_err(io::Error::other)?["result"]["code"],
            "reconciliation_required"
        );
        let spec: DaemonRequest =
            serde_json::from_value(fixture.launch("prior")).map_err(io::Error::other)?;
        assert_eq!(
            serde_json::to_value(daemon.state.dispatch(spec)).map_err(io::Error::other)?["result"]
                ["code"],
            "duplicate_run"
        );
        assert_eq!(
            fs::read(fixture.spool.join("prior/output.journal"))?,
            journal
        );
        Ok(())
    }

    #[test]
    fn idle_connections_have_absolute_deadline_and_explicit_saturation() -> io::Result<()> {
        let fixture = Fixture::new()?;
        let mut idle = Vec::new();
        for _ in 0..MAX_CONNECTIONS {
            idle.push(UnixStream::connect(&fixture.socket)?);
        }
        thread::sleep(Duration::from_millis(100));
        let mut excess = UnixStream::connect(&fixture.socket)?;
        assert_eq!(read_response(&mut excess)?["result"]["code"], "busy");
        let started = Instant::now();
        let mut byte = [0];
        idle[0].set_read_timeout(Some(Duration::from_secs(3)))?;
        assert_eq!(idle[0].read(&mut byte)?, 0);
        assert!(started.elapsed() < Duration::from_secs(3));
        drop(idle);
        assert_eq!(fixture.status("missing")?["result"]["code"], "unknown_run");
        Ok(())
    }

    #[test]
    fn prepared_codex_large_escaped_prompt_roundtrips_socket_without_inference() -> io::Result<()> {
        use crate::{
            models::ModelSpec,
            providers::{
                codex::{
                    CodexAdmission, CodexCliObservation, CodexEffort, CodexExecutionOptions,
                    CodexNdjsonStream, CodexSandbox, CodexTermination, SUPPORTED_CLI_VERSION,
                    prepare_launch,
                },
                supervision::SupervisionScope,
            },
        };
        use admiralty_core::{TaskContract, content_digest};
        let fixture = Fixture::new()?;
        let work = fixture.root.join("work");
        fs::create_dir(&work)?;
        let program = fixture.root.join("fake-codex");
        fs::write(&program,b"#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '{\"type\":\"thread.started\",\"thread_id\":\"thread\"}' '{\"type\":\"turn.started\"}' '{\"type\":\"turn.completed\",\"usage\":{}}'\n")?;
        fs::set_permissions(&program, fs::Permissions::from_mode(0o700))?;
        let parse = io::Error::other;
        let contract:TaskContract=serde_json::from_value(json!({"schema_version":1,"project_id":"project","root_id":"root","task_id":"task","revision":1,"title":"Socket fixture","objective":"Fake only","non_goals":[],"owned_paths":["src"],"forbidden_paths":[],"read_first":[],"dependencies":[],"criteria":[{"id":"C1","text":"Check","check_ids":["check"]}],"checks":[{"id":"check","program":"fixture","args":[],"cwd":".","timeout_secs":1}],"authority_ref":"authority:fixture","max_attempts":1,"simulated":false})).map_err(parse)?;
        let model:ModelSpec=serde_json::from_value(json!({"id":"codex","label":"Fake Codex","provider":"codex_cli","provider_model":"fake-model","tier":0,"enabled":true,"node_id":"node","endpoint":null,"credential_file_ref":null,"max_concurrency":1,"capabilities":["code"],"task_max_attempts":1,"escalate_to":null})).map_err(parse)?;
        let scope:SupervisionScope=serde_json::from_value(json!({"project_id":"project","root_id":"root","task_id":"task","attempt_id":"attempt","owner":"captain","incarnation":"one","authority_ref":"authority:fixture","contract_digest":content_digest(&contract).map_err(parse)?,"revision":1})).map_err(parse)?;
        let cli=CodexCliObservation::from_help("node",program,SUPPORTED_CLI_VERSION,"--json --color --ignore-user-config --strict-config --ephemeral --model --sandbox --cd","--no-daemon --ask-for-approval",Some(0)).map_err(io::Error::other)?;
        let admission = CodexAdmission {
            scope,
            attempt_ordinal: 1,
            registry_revision: "0".into(),
            selected_model_digest: content_digest(&model).map_err(parse)?,
            cli_observation_digest: cli.fingerprint().into(),
            node_id: "node".into(),
            working_dir: work,
            spool_root: fixture.spool.clone(),
            lease_ref: "lease:fixture".into(),
            resource_permit_ref: "permit:fixture".into(),
            custody_ref: "custody:fixture".into(),
            auth_context_ref: "auth:fixture".into(),
            admitted_tick: 0,
            expires_tick: 10,
        };
        let capsule = "literal \\\"quoted\\\" path\\segment\n".repeat(800);
        let prepared = prepare_launch(
            &contract,
            admission,
            &model,
            CodexExecutionOptions {
                effort: CodexEffort::Medium,
                sandbox: CodexSandbox::ReadOnly,
            },
            &cli,
            &capsule,
            Some(0),
        )
        .map_err(io::Error::other)?;
        assert!(
            prepared
                .launch_spec()
                .args
                .last()
                .is_some_and(|s| s.len() > 4096)
        );
        let request = json!({"command":"launch","spec":prepared.launch_spec()});
        assert!(serde_json::to_vec(&request).map_err(parse)?.len() > 4096);
        assert!(serde_json::to_vec(&request).map_err(parse)?.len() <= MAX_REQUEST_BYTES);
        let run = &prepared.launch_spec().run_id;
        assert_eq!(fixture.request(&request)?["result"]["kind"], "launched");
        assert_eq!(
            fixture.request(&json!({"command":"close_input","run_id":run,"expected_epoch":"0"}))?["result"]
                ["input_epoch"],
            "1"
        );
        let deadline = Instant::now() + Duration::from_secs(3);
        let status = loop {
            let status = fixture.status(run)?;
            if status["result"]["status"]["capture_state"] == "complete"
                && status["result"]["status"]["state"] == "exited"
            {
                break status;
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        };
        let final_offset = status["result"]["status"]["final_output_offset"]
            .as_str()
            .and_then(|s| s.parse::<u64>().ok())
            .ok_or_else(|| io::Error::other("final watermark absent"))?;
        let tail =
            fixture.request(&json!({"command":"tail","run_id":run,"from":"0","max_bytes":8192}))?;
        let mut parser = CodexNdjsonStream::from_prepared(&prepared).map_err(io::Error::other)?;
        for chunk in tail["result"]["chunks"]
            .as_array()
            .ok_or_else(|| io::Error::other("chunks absent"))?
        {
            let offset = chunk["offset"]
                .as_str()
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| io::Error::other("offset absent"))?;
            let stream = serde_json::from_value(chunk["stream"].clone()).map_err(parse)?;
            let bytes = serde_json::from_value(chunk["bytes"].clone()).map_err(parse)?;
            parser
                .push_chunk(&super::super::OutputChunk {
                    offset,
                    stream,
                    bytes,
                })
                .map_err(io::Error::other)?;
        }
        let summary = parser.finish_at_watermark(
            Some(0),
            super::super::CaptureState::Complete,
            Some(final_offset),
        );
        assert_eq!(summary.termination, CodexTermination::ProviderTurnCompleted);
        assert!(!summary.trusted_acceptance());
        assert!(!summary.native_execution_qualified());
        let mut overflow = fixture.launch("overflow");
        overflow["spec"]["args"] = json!(["\u{1}".repeat(super::super::MAX_LAUNCH_ARG_BYTES)]);
        assert!(serde_json::to_vec(&overflow).map_err(parse)?.len() > MAX_REQUEST_BYTES);
        let mut oversized = UnixStream::connect(&fixture.socket)?;
        oversized.write_all(
            &(serde_json::to_vec(&overflow).map_err(parse)?.len() as u32).to_be_bytes(),
        )?;
        assert_eq!(
            read_response(&mut oversized)?["result"]["code"],
            "invalid_frame"
        );
        assert!(!fixture.spool.join("overflow").exists());
        let mut overflow = fixture.launch("arg-overflow");
        overflow["spec"]["args"] = json!(["x".repeat(super::super::MAX_LAUNCH_ARG_BYTES + 1)]);
        assert_eq!(
            fixture.request(&overflow)?["result"]["code"],
            "invalid_request"
        );
        assert!(!fixture.spool.join("arg-overflow").exists());
        Ok(())
    }

    #[test]
    fn exited_child_with_held_pipe_has_no_final_watermark_until_trailing_output() -> io::Result<()>
    {
        let fixture = Fixture::new()?;
        let release = fixture.root.join("release");
        struct Release(PathBuf);
        impl Drop for Release {
            fn drop(&mut self) {
                let _ = fs::write(&self.0, b"release");
            }
        }
        let _release_on_failure = Release(release.clone());
        let request = json!({"command":"launch","spec":{"run_id":"held","program":"/bin/sh","args":["-c","(while [ ! -f \"$1\" ]; do sleep 0.01; done; printf trailing) & exit 0","fake",release],"cwd":fixture.root,"spool_dir":fixture.spool.join("held")}});
        fixture.request(&request)?;
        let deadline = Instant::now() + Duration::from_secs(3);
        let exited = loop {
            let status = fixture.status("held")?;
            if status["result"]["status"]["state"] == "exited" {
                break status;
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(exited["result"]["status"]["capture_state"], "capturing");
        assert_eq!(
            exited["result"]["status"]["final_output_offset"],
            Value::Null
        );
        assert_eq!(exited["result"]["status"]["capture_failed"], false);
        let prefix = fixture
            .request(&json!({"command":"tail","run_id":"held","from":"0","max_bytes":8192}))?;
        assert!(tail_bytes(&prefix).is_empty());
        fs::write(release, b"release")?;
        let deadline = Instant::now() + Duration::from_secs(3);
        let complete = loop {
            let status = fixture.status("held")?;
            if status["result"]["status"]["capture_state"] == "complete" {
                break status;
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(complete["result"]["status"]["final_output_offset"], "8");
        assert_eq!(
            fixture.status("held")?["result"]["status"]["final_output_offset"],
            "8"
        );
        let tail = fixture
            .request(&json!({"command":"tail","run_id":"held","from":"0","max_bytes":8192}))?;
        assert_eq!(tail_bytes(&tail), b"trailing");
        assert_eq!(tail["result"]["next_offset"], "8");
        Ok(())
    }
}
