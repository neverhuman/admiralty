#![cfg(unix)]
//! Same daemon and owned child across client reconnects, not service restart.

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
