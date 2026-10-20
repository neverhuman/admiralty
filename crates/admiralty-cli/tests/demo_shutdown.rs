//! Real owned demo children, including two open SSE response families.
#![cfg(unix)]
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

struct OwnedDemo {
    child: Child,
    directory: Option<tempfile::TempDir>,
}
impl Drop for OwnedDemo {
    fn drop(&mut self) {
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            let _ = self.child.kill();
            let deadline = Instant::now() + Duration::from_secs(2);
            while !matches!(self.child.try_wait(), Ok(Some(_))) {
                if Instant::now() >= deadline {
                    if let Some(directory) = self.directory.take() {
                        eprintln!(
                            "Unknown owned child outcome; preserved {}",
                            directory.keep().display()
                        );
                    }
                    return;
                }
                thread::sleep(Duration::from_millis(10));
            }
        }
    }
}

fn open_sse(address: SocketAddr, path: &str) -> TcpStream {
    let mut stream =
        TcpStream::connect_timeout(&address, Duration::from_secs(1)).expect("owned listener");
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("bounded read");
    stream
        .set_write_timeout(Some(Duration::from_secs(1)))
        .expect("bounded write");
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {address}\r\nAccept: text/event-stream\r\n\r\n"
    )
    .expect("owned SSE request");
    let mut header = Vec::new();
    while !header.ends_with(b"\r\n\r\n") {
        let mut byte = [0];
        assert_eq!(stream.read(&mut byte).expect("SSE header"), 1);
        header.push(byte[0]);
        assert!(header.len() < 8192, "bounded header");
    }
    let text = String::from_utf8(header)
        .expect("HTTP header")
        .to_ascii_lowercase();
    assert!(text.starts_with("http/1.1 200"), "{text}");
    assert!(text.contains("text/event-stream"), "{text}");
    stream
}

fn shutdown_with_open_streams(signal: rustix::process::Signal) {
    let directory = tempfile::Builder::new()
        .prefix("demo-shutdown-")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in(
            std::env::temp_dir()
                .canonicalize()
                .expect("canonical scratch"),
        )
        .expect("private scratch");
    let tmp = directory.path().join("tmp");
    fs::create_dir(&tmp).expect("owned temp parent");
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o700)).expect("private parent");
    let listener = TcpListener::bind("127.0.0.1:0").expect("reserve loopback");
    let address = listener.local_addr().expect("owned address");
    drop(listener);
    let capture = |name| {
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(directory.path().join(name))
            .expect("private capture")
    };
    let child = Command::new(env!("CARGO_BIN_EXE_admiralty"))
        .args(["demo", "--listen", &address.to_string()])
        .env_clear()
        .env("TMPDIR", &tmp)
        .stdin(Stdio::null())
        .stdout(capture("stdout"))
        .stderr(capture("stderr"))
        .spawn()
        .expect("actual owned demo");
    let mut owned = OwnedDemo {
        child,
        directory: Some(directory),
    };
    let ready_deadline = Instant::now() + Duration::from_secs(5);
    loop {
        assert!(owned.child.try_wait().expect("child status").is_none());
        if TcpStream::connect_timeout(&address, Duration::from_millis(100)).is_ok() {
            break;
        }
        assert!(Instant::now() < ready_deadline, "owned listener readiness");
        thread::sleep(Duration::from_millis(10));
    }
    let mut streams = [
        open_sse(address, "/v2/events"),
        open_sse(address, "/v2/views/events"),
    ];
    assert_eq!(fs::read_dir(&tmp).expect("live temp tree").count(), 1);
    let pid = rustix::process::Pid::from_raw(owned.child.id() as i32).expect("owned PID");
    rustix::process::kill_process(pid, signal).expect("signal only owned direct child");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = owned.child.try_wait().expect("reap owned demo") {
            assert!(status.success(), "clean signal shutdown: {status}");
            break;
        }
        assert!(
            Instant::now() < deadline,
            "demo must exit while client SSE sockets remain open"
        );
        thread::sleep(Duration::from_millis(10));
    }
    // Readers were kept open through process exit. Drain bounded final chunks.
    for stream in &mut streams {
        let mut body = Vec::new();
        stream
            .take(64 * 1024 + 1)
            .read_to_end(&mut body)
            .expect("server closes SSE");
        assert!(body.len() <= 64 * 1024);
    }
    assert_eq!(fs::read_dir(&tmp).expect("drained temp parent").count(), 0);
    assert!(TcpStream::connect_timeout(&address, Duration::from_millis(100)).is_err());
}

#[test]
fn sigint_drains_both_open_sse_families_and_removes_demo_state() {
    shutdown_with_open_streams(rustix::process::Signal::INT);
}

#[test]
fn sigterm_drains_both_open_sse_families_and_removes_demo_state() {
    shutdown_with_open_streams(rustix::process::Signal::TERM);
}
