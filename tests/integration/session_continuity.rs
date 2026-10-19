#![cfg(unix)]
//! Client reconnects and explicit Linux service-manager restart qualification.

use admiralty_runtime::session::daemon::{
    DaemonConfig, MAX_REQUEST_BYTES, MAX_RESPONSE_BYTES, SessionDaemon,
};
use serde_json::{Value, json};
use std::{
    fs,
    io::{self, Read, Write},
    os::unix::{
        fs::{DirBuilderExt, MetadataExt, PermissionsExt},
        net::UnixStream,
    },
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};
use tempfile::TempDir;

const WORKER_ROOT: &str = "ADMIRALTY_TEST_SESSION_WORKER_ROOT";
#[cfg(target_os = "linux")]
const COORDINATOR_ROOT: &str = "ADMIRALTY_TEST_COORDINATOR_ROOT";
const WAIT: Duration = Duration::from_secs(3);

fn private_tempdir() -> io::Result<TempDir> {
    tempfile::Builder::new()
        .prefix("as-")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in(fs::canonicalize(std::env::temp_dir())?)
}

fn remaining(deadline: Instant) -> io::Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|duration| !duration.is_zero())
        .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "fixture frame deadline"))
}

fn read_until(stream: &mut UnixStream, mut bytes: &mut [u8], deadline: Instant) -> io::Result<()> {
    stream.set_nonblocking(true)?;
    while !bytes.is_empty() {
        remaining(deadline)?;
        match stream.read(bytes) {
            Ok(0) => return Err(io::Error::from(io::ErrorKind::UnexpectedEof)),
            Ok(count) => bytes = &mut bytes[count..],
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(5).min(remaining(deadline)?));
            }
            Err(error) => {
                return Err(io::Error::new(
                    error.kind(),
                    format!("fixture read: {error}"),
                ));
            }
        }
    }
    Ok(())
}

fn write_until(stream: &mut UnixStream, mut bytes: &[u8], deadline: Instant) -> io::Result<()> {
    stream.set_nonblocking(true)?;
    while !bytes.is_empty() {
        remaining(deadline)?;
        match stream.write(bytes) {
            Ok(0) => return Err(io::Error::from(io::ErrorKind::WriteZero)),
            Ok(count) => bytes = &bytes[count..],
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(5).min(remaining(deadline)?));
            }
            Err(error) => {
                return Err(io::Error::new(
                    error.kind(),
                    format!("fixture write: {error}"),
                ));
            }
        }
    }
    Ok(())
}

fn read_response(stream: &mut UnixStream, deadline: Instant) -> io::Result<Value> {
    let mut header = [0; 4];
    read_until(stream, &mut header, deadline)?;
    let size = u32::from_be_bytes(header) as usize;
    if size == 0 || size > MAX_RESPONSE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "oversized fixture response",
        ));
    }
    let mut response = vec![0; size];
    read_until(stream, &mut response, deadline)?;
    serde_json::from_slice(&response).map_err(io::Error::other)
}

fn connect_until(socket: &Path, deadline: Instant) -> io::Result<UnixStream> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()?;
    let stream = runtime.block_on(async {
        tokio::time::timeout(
            remaining(deadline)?,
            tokio::net::UnixStream::connect(socket),
        )
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "fixture connect deadline"))?
    })?;
    // Tokio preserves nonblocking mode, as required by the absolute I/O loops.
    stream.into_std()
}

fn exchange_until(socket: &Path, request: &Value, deadline: Instant) -> io::Result<Value> {
    let mut stream = connect_until(socket, deadline)
        .map_err(|error| io::Error::new(error.kind(), format!("fixture connect: {error}")))?;
    let size = admiralty_core::limits::bounded_json_size(request, MAX_REQUEST_BYTES)
        .map_err(io::Error::other)?;
    let body = serde_json::to_vec(request).map_err(io::Error::other)?;
    let length = u32::try_from(size).map_err(io::Error::other)?;
    write_until(&mut stream, &length.to_be_bytes(), deadline)?;
    write_until(&mut stream, &body, deadline)?;
    read_response(&mut stream, deadline)
}

fn exchange(socket: &Path, request: &Value) -> io::Result<Value> {
    exchange_until(socket, request, Instant::now() + WAIT)
}

fn await_offset(socket: &Path, run_id: &str, offset: &str) -> Value {
    let deadline = Instant::now() + WAIT;
    loop {
        let response = exchange_until(
            socket,
            &json!({"command":"status","run_id":run_id}),
            deadline,
        )
        .expect("session daemon status response");
        if response["result"]["status"]["output_offset"] == offset {
            return response;
        }
        assert!(Instant::now() < deadline, "expected output offset {offset}");
        thread::sleep(Duration::from_millis(5));
    }
}

fn await_exit(socket: &Path, run_id: &str, final_offset: &str) {
    let deadline = Instant::now() + WAIT;
    loop {
        let response = exchange_until(
            socket,
            &json!({"command":"status","run_id":run_id}),
            deadline,
        )
        .expect("session daemon status response");
        if response["result"]["status"]["state"] == "exited"
            && response["result"]["status"]["capture_state"] == "complete"
            && response["result"]["status"]["final_output_offset"] == final_offset
        {
            assert_eq!(response["native_execution_qualified"], false);
            assert_eq!(response["execution_granted"], false);
            assert_eq!(response["acceptance_granted"], false);
            return;
        }
        assert!(
            Instant::now() < deadline,
            "child should exit after its input"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

fn tail_bytes(response: &Value) -> Vec<u8> {
    response["result"]["chunks"]
        .as_array()
        .expect("tail chunks")
        .iter()
        .flat_map(|chunk| {
            chunk["bytes"]
                .as_array()
                .expect("tail bytes")
                .iter()
                .map(|byte| u8::try_from(byte.as_u64().expect("byte value")).expect("bounded byte"))
        })
        .collect()
}

#[test]
fn daemon_client_reconnect_keeps_child_and_output_offsets_continuous() {
    let temporary = private_tempdir().expect("canonical private-at-creation fixture");
    let spool = temporary.path().join("p");
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&spool)
        .expect("private spool at creation");
    assert_eq!(
        fs::metadata(temporary.path()).expect("root mode").mode() & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(&spool).expect("spool mode").mode() & 0o777,
        0o700
    );
    let mut daemon = OwnedDaemon::start(temporary.path()).expect("start owned fixture daemon");
    let socket = daemon.socket.clone();

    let launch = exchange(
        &socket,
        &json!({
            "command":"launch",
            "spec":{
                "run_id":"continuous",
                "program":"/bin/sh",
                "args":["-c","printf A; IFS= read -r line; printf B"],
                "cwd":temporary.path(),
                "spool_dir":spool.join("continuous")
            }
        }),
    )
    .expect("launch child");
    assert_eq!(launch["result"]["kind"], "launched");

    let duplicate = exchange(
        &socket,
        &json!({
            "command":"launch",
            "spec":{
                "run_id":"continuous",
                "program":"/bin/true",
                "args":[],
                "cwd":temporary.path(),
                "spool_dir":spool.join("continuous")
            }
        }),
    )
    .expect("duplicate launch response");
    assert_eq!(duplicate["result"]["kind"], "error");
    assert_eq!(duplicate["result"]["code"], "duplicate_run");

    let initial = await_offset(&socket, "continuous", "1");
    let initial_pid = initial["result"]["status"]["pid"].clone();
    let initial_identity = initial["result"]["status"]["start_identity"].clone();
    let prefix = exchange(
        &socket,
        &json!({"command":"tail","run_id":"continuous","from":"0","max_bytes":1}),
    )
    .expect("read first output range");
    assert_eq!(tail_bytes(&prefix), b"A");
    assert_eq!(prefix["result"]["next_offset"], "1");

    let input = exchange(
        &socket,
        &json!({
            "command":"input",
            "run_id":"continuous",
            "expected_epoch":"0",
            "bytes":[120,10]
        }),
    )
    .expect("deliver one input frame after reconnect");
    assert_eq!(input["result"]["input_epoch"], "1");

    let overflow_epoch = exchange(
        &socket,
        &json!({
            "command":"input",
            "run_id":"continuous",
            "expected_epoch":"18446744073709551616",
            "bytes":[121,10]
        }),
    )
    .expect("overflowing input epoch response");
    assert_eq!(overflow_epoch["result"]["kind"], "error");
    assert_eq!(overflow_epoch["result"]["code"], "invalid_request");
    assert_eq!(
        exchange(&socket, &json!({"command":"status","run_id":"continuous"}))
            .expect("status after rejected overflow epoch")["result"]["status"]["input_epoch"],
        "1",
        "overflowing epoch must not advance session input state"
    );

    let continued = await_offset(&socket, "continuous", "2");
    assert_eq!(continued["result"]["status"]["pid"], initial_pid);
    assert_eq!(
        continued["result"]["status"]["start_identity"],
        initial_identity
    );
    let suffix = exchange(
        &socket,
        &json!({"command":"tail","run_id":"continuous","from":"1","max_bytes":8}),
    )
    .expect("read output after reconnect");
    assert_eq!(tail_bytes(&suffix), b"B");
    assert_eq!(suffix["result"]["next_offset"], "2");
    await_exit(&socket, "continuous", "2");
    assert!(
        daemon
            .stop()
            .expect("finite daemon shutdown and reap")
            .success()
    );
}

/// Opt-in because this creates two test-owned transient systemd units using sudo.
/// Both units run in the host's capped slice and are stopped by their owned guards.
#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires explicit local systemd transient-service qualification"]
fn systemd_coordinator_restart_preserves_child_and_journal() {
    let temporary = private_tempdir().expect("private service fixture");
    fs::DirBuilder::new()
        .mode(0o700)
        .create(temporary.path().join("p"))
        .expect("private spool");
    let session = TestService::start(
        temporary.path(),
        "session",
        "owned_daemon_worker",
        WORKER_ROOT,
    );
    let socket = temporary.path().join("s");
    let deadline = Instant::now() + WAIT;
    while !socket.exists() {
        remaining(deadline).expect("session socket readiness");
        thread::sleep(Duration::from_millis(10));
    }
    let coordinator = TestService::start(
        temporary.path(),
        "coordinator",
        "owned_coordinator_worker",
        COORDINATOR_ROOT,
    );
    let first = wait_for_file(&temporary.path().join("before.json"));
    assert_eq!(first["result"]["status"]["output_offset"], "2");
    assert_eq!(first["result"]["status"]["input_epoch"], "1");
    assert_eq!(first["result"]["status"]["state"], "running");
    assert!(
        first["result"]["status"]["start_identity"]
            .as_str()
            .is_some()
    );
    coordinator.control("restart");
    let after = wait_for_file(&temporary.path().join("after.json"));
    for field in ["pid", "start_identity"] {
        assert_eq!(
            first["result"]["status"][field],
            after["result"]["status"][field]
        );
    }
    assert_eq!(after["result"]["status"]["input_epoch"], "2");
    assert_eq!(after["result"]["status"]["output_offset"], "3");
    let tail = exchange(
        &socket,
        &json!({"command":"tail","run_id":"restart","from":"0","max_bytes":8}),
    )
    .expect("continuous journal");
    assert_eq!(tail_bytes(&tail), b"ABC");
    assert_eq!(tail["result"]["next_offset"], "3");
    await_exit(&socket, "restart", "3");
    drop(coordinator);
    drop(session);
}

#[cfg(target_os = "linux")]
fn wait_for_file(path: &Path) -> Value {
    let deadline = Instant::now() + WAIT;
    loop {
        if let Ok(bytes) = fs::read(path)
            && let Ok(value) = serde_json::from_slice(&bytes)
        {
            return value;
        }
        remaining(deadline).expect("coordinator checkpoint readiness");
        thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(target_os = "linux")]
struct TestService(String);

#[cfg(target_os = "linux")]
impl TestService {
    fn start(root: &Path, role: &str, worker: &str, variable: &str) -> Self {
        let name = format!("admiralty-p07-test-{}-{role}.service", uuid::Uuid::new_v4());
        let owned = Self(name);
        let status = Command::new("sudo")
            .args(["-n", "systemd-run", "--quiet", "--collect"])
            .arg(format!("--unit={}", owned.0))
            .args([
                "--service-type=exec",
                "--property=Slice=jarambit-evo.slice",
                "--property=RuntimeMaxSec=30",
                "--property=TimeoutStopSec=3",
                "--property=UMask=0077",
            ])
            .arg(format!("--uid={}", rust_uid()))
            .arg(format!("--setenv={variable}={}", root.display()))
            .arg("--")
            .arg(std::env::current_exe().expect("test binary"))
            .args([
                "--exact",
                worker,
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .status()
            .expect("create owned transient service");
        assert!(status.success(), "transient service start failed");
        owned
    }

    fn control(&self, verb: &str) {
        assert!(
            Command::new("sudo")
                .args(["-n", "systemctl", verb, &self.0])
                .status()
                .expect("owned transient service control")
                .success()
        );
    }
}

#[cfg(target_os = "linux")]
fn rust_uid() -> u32 {
    fs::metadata(std::env::current_exe().expect("test binary"))
        .expect("test binary owner")
        .uid()
}

#[cfg(target_os = "linux")]
impl Drop for TestService {
    fn drop(&mut self) {
        let _ = Command::new("sudo")
            .args(["-n", "systemctl", "stop", &self.0])
            .status();
    }
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "test-owned transient coordinator worker"]
fn owned_coordinator_worker() {
    let root = PathBuf::from(std::env::var_os(COORDINATOR_ROOT).expect("owned coordinator root"));
    let socket = root.join("s");
    let before = root.join("before.json");
    let checkpoint = if before.exists() {
        let saved: Value = serde_json::from_slice(&fs::read(&before).expect("saved identity"))
            .expect("saved status");
        let current = exchange(&socket, &json!({"command":"status","run_id":"restart"}))
            .expect("reconcile live child");
        for field in ["pid", "start_identity", "input_epoch", "output_offset"] {
            assert_eq!(
                saved["result"]["status"][field],
                current["result"]["status"][field]
            );
        }
        let reply = exchange(
            &socket,
            &json!({"command":"input","run_id":"restart","expected_epoch":"1","bytes":[121,10]}),
        )
        .expect("second input");
        assert_eq!(reply["result"]["input_epoch"], "2");
        (
            root.join("after.json"),
            await_offset(&socket, "restart", "3"),
        )
    } else {
        let launch = exchange(&socket, &json!({"command":"launch","spec":{
            "run_id":"restart","program":"/bin/sh",
            "args":["-c","printf A; IFS= read -r first; printf B; IFS= read -r second; printf C"],
            "cwd":root,"spool_dir":root.join("p/restart")
        }})).expect("coordinator launch");
        assert_eq!(launch["result"]["kind"], "launched");
        await_offset(&socket, "restart", "1");
        let reply = exchange(
            &socket,
            &json!({"command":"input","run_id":"restart","expected_epoch":"0","bytes":[120,10]}),
        )
        .expect("first input");
        assert_eq!(reply["result"]["input_epoch"], "1");
        (before, await_offset(&socket, "restart", "2"))
    };
    fs::write(
        checkpoint.0,
        serde_json::to_vec(&checkpoint.1).expect("checkpoint JSON"),
    )
    .expect("checkpoint write");
    thread::sleep(Duration::from_secs(20));
}

/// A subprocess of this test binary, selected only by OwnedDaemon. The parent
/// never depends on a separately built or installed CLI executable.
#[test]
#[ignore = "owned fixture worker, explicitly spawned by the reconnect test"]
fn owned_daemon_worker() {
    let root = PathBuf::from(std::env::var_os(WORKER_ROOT).expect("explicit owned fixture root"));
    // A finite backstop even if the owning test process disappears. Normal
    // shutdown takes the identity-check failure path below and runs Host Drop.
    thread::spawn(|| {
        thread::sleep(Duration::from_secs(15));
        std::process::exit(124);
    });
    let daemon = SessionDaemon::bind(DaemonConfig {
        socket: root.join("s"),
        spool_root: root.join("p"),
    })
    .expect("bind private fixture daemon");
    let error = daemon
        .serve()
        .expect_err("parent invalidates only its owned fixture socket");
    assert_eq!(error.kind(), io::ErrorKind::NotFound);
}

struct OwnedDaemon {
    child: Option<Child>,
    socket: PathBuf,
    socket_identity: Option<(u64, u64)>,
    stopped: bool,
    exit: Option<ExitStatus>,
}

impl OwnedDaemon {
    fn start(root: &Path) -> io::Result<Self> {
        let child = Command::new(std::env::current_exe()?)
            .args([
                "--exact",
                "owned_daemon_worker",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env_clear()
            .env(WORKER_ROOT, root)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        let mut owned = Self {
            child: Some(child),
            socket: root.join("s"),
            socket_identity: None,
            stopped: false,
            exit: None,
        };
        let deadline = Instant::now() + WAIT;
        loop {
            if owned
                .child
                .as_mut()
                .expect("owned daemon handle")
                .try_wait()?
                .is_some()
            {
                return Err(io::Error::other("fixture daemon exited before readiness"));
            }
            if let Ok(metadata) = fs::symlink_metadata(&owned.socket) {
                if metadata.mode() & 0o777 != 0o600 {
                    return Err(io::Error::other("fixture socket is not private"));
                }
                owned.socket_identity = Some((metadata.dev(), metadata.ino()));
                return Ok(owned);
            }
            remaining(deadline)?;
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn stop(&mut self) -> io::Result<ExitStatus> {
        if let Some(status) = self.exit {
            return Ok(status);
        }
        let deadline = Instant::now() + WAIT;
        if !self.stopped {
            self.stopped = true;
            if let Some(identity) = self.socket_identity {
                let metadata = fs::symlink_metadata(&self.socket)?;
                if (metadata.dev(), metadata.ino()) != identity {
                    return Err(io::Error::other("fixture socket identity changed"));
                }
                let retired = self.socket.with_file_name("x");
                fs::rename(&self.socket, &retired)?;
                // Wake the original listener. Its pinned path is now absent,
                // so serve fails closed and drops/reaps its owned fake session.
                drop(connect_until(&retired, deadline)?);
            }
        }
        loop {
            if let Some(status) = self
                .child
                .as_mut()
                .expect("owned daemon handle")
                .try_wait()?
            {
                self.exit = Some(status);
                return Ok(status);
            }
            remaining(deadline)?;
            thread::sleep(Duration::from_millis(5));
        }
    }
}

impl Drop for OwnedDaemon {
    fn drop(&mut self) {
        if self.exit.is_none() && self.stop().is_err() {
            let Some(mut child) = self.child.take() else {
                return;
            };
            // Child handle ownership is retained; never signal a historical PID.
            if matches!(child.try_wait(), Ok(None)) {
                let _ = child.kill();
            }
            let deadline = Instant::now() + WAIT;
            loop {
                if let Ok(Some(status)) = child.try_wait() {
                    self.exit = Some(status);
                    return;
                }
                if Instant::now() >= deadline {
                    break;
                }
                thread::sleep(Duration::from_millis(5));
            }
            // Exceptional OS-level nonreaping is not a successful cleanup.
            // Keep the owned handle in an explicit eventual reaper without
            // blocking this test's foreground or claiming it was reaped.
            eprintln!("fixture daemon cleanup incomplete; transferring owned Child to reaper");
            thread::spawn(move || {
                let _ = child.wait();
            });
        }
    }
}

#[test]
fn socket_response_length_is_bounded_before_body_read_or_allocation() {
    assert_eq!(
        connect_until(Path::new("/unused-fixture-socket"), Instant::now() - WAIT)
            .expect_err("expired connect must fail before a socket operation")
            .kind(),
        io::ErrorKind::TimedOut
    );
    for size in [0, MAX_RESPONSE_BYTES as u32 + 1, u32::MAX] {
        let (mut client, mut peer) = UnixStream::pair().expect("owned local sockets");
        peer.write_all(&size.to_be_bytes())
            .expect("header only, no body");
        assert_eq!(
            read_response(&mut client, Instant::now() + WAIT)
                .expect_err("reject before reading body")
                .kind(),
            io::ErrorKind::InvalidData
        );
    }
    let (mut client, _peer) = UnixStream::pair().expect("owned local sockets");
    assert_eq!(
        read_response(&mut client, Instant::now() + Duration::from_millis(20))
            .expect_err("absolute read deadline")
            .kind(),
        io::ErrorKind::TimedOut
    );
}
