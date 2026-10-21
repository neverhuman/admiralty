//! Ports of `scripts/check/canary-smoke.py::HarnessTests` (synthetic only:
//! no Admiralty binaries, network services or fleet state are involved).

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

use clap::Parser;

use super::*;
use crate::http::{Watchdog, Wire, monotonic};
use crate::process::{CleanupOutcome, FINAL_WAIT_SECONDS, TERM_WAIT_SECONDS, cleanup_owned_child};

const LISTENER: &[u8] = b"127.0.0.1:12345 (read-only; native execution disabled)";

fn is_smoke<T: std::fmt::Debug>(result: &Checked<T>) -> bool {
    matches!(result, Err(CanaryError::Smoke(_)))
}

fn smoke_message<T: std::fmt::Debug>(result: Checked<T>) -> String {
    match result {
        Err(CanaryError::Smoke(message)) => message,
        other => panic!("expected SmokeError, got {other:?}"),
    }
}

fn line(parts: &[&[u8]]) -> Vec<u8> {
    parts.concat()
}

fn private_tempdir() -> (tempfile::TempDir, PathBuf) {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let root = temporary
        .path()
        .canonicalize()
        .expect("resolve temporary root");
    (temporary, root)
}

#[derive(Parser)]
struct ArgvProbe {
    #[command(flatten)]
    smoke: SmokeArgs,
}

fn parse_args(argv: &[&str]) -> Result<(), String> {
    let mut full = vec!["admiralty-canary"];
    full.extend_from_slice(argv);
    let parsed = ArgvProbe::try_parse_from(full).map_err(|error| error.to_string())?;
    resolve_args(&parsed.smoke).map(|_| ())
}

/// Ported from scripts/check/canary-smoke.py::HarnessTests::test_argument_validation_requires_explicit_paths
#[test]
fn test_argument_validation_requires_explicit_paths() {
    assert!(parse_args(&[]).is_err());
    assert!(parse_args(&["--self-test", "--scratch-root", "/tmp"]).is_err());
}

/// Ported from scripts/check/canary-smoke.py::HarnessTests::test_binary_path_rejects_relative_and_symlink
#[test]
fn test_binary_path_rejects_relative_and_symlink() {
    assert!(is_smoke(&validate_absolute_owned_file(
        "relative/path",
        false
    )));
    let (_temporary, root) = private_tempdir();
    let target = root.join("target");
    fs::write(&target, "x").expect("write target");
    let link = root.join("link");
    std::os::unix::fs::symlink(&target, &link).expect("create symlink");
    assert!(is_smoke(&validate_absolute_owned_file(
        &link.to_string_lossy(),
        false
    )));
}

/// Ported from scripts/check/canary-smoke.py::HarnessTests::test_scratch_requires_private_owned_directory
#[test]
fn test_scratch_requires_private_owned_directory() {
    let (_temporary, root) = private_tempdir();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).expect("chmod 0700");
    assert_eq!(
        validate_scratch_root(&root.to_string_lossy()),
        Ok(root.clone())
    );
    fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).expect("chmod 0755");
    assert!(is_smoke(&validate_scratch_root(&root.to_string_lossy())));
}

/// Ported from scripts/check/canary-smoke.py::HarnessTests::test_listener_rejects_bad_future_and_protected_sources
#[test]
fn test_listener_rejects_bad_future_and_protected_sources() {
    assert_eq!(
        validate_listener_line(
            b"Admiralty listening on http://127.0.0.1:12345 (read-only; native execution disabled)"
        ),
        Ok(("127.0.0.1".to_owned(), 12345))
    );
    for bad in [
        &b"future listener version http://127.0.0.1:12345"[..],
        b"Admiralty listening on http://example.com:12345",
        b"Admiralty listening on http://127.0.0.1:18891",
        b"Admiralty listening on https://127.0.0.1:12345",
        b"Admiralty listening on http://127.0.0.1:0",
    ] {
        assert!(
            is_smoke(&validate_listener_line(bad)),
            "{}",
            String::from_utf8_lossy(bad)
        );
    }
}

/// Ported from scripts/check/canary-smoke.py::HarnessTests::test_listener_waits_for_every_split_and_crlf_record
#[test]
fn test_listener_waits_for_every_split_and_crlf_record() {
    for ending in [&b"\n"[..], b"\r\n"] {
        let full = line(&[LISTEN_PREFIX, LISTENER, ending]);
        for split in 0..full.len() {
            let capture = BoundedCapture::new(MAX_OUTPUT_BYTES);
            capture.append(&full[..split]);
            assert_eq!(find_listener(&capture), Ok(None), "split {split}");
            capture.append(&full[split..]);
            assert_eq!(
                find_listener(&capture),
                Ok(Some(("127.0.0.1".to_owned(), 12345))),
                "split {split}"
            );
        }
    }
}

struct Exited;

impl ChildPoll for Exited {
    fn poll(&mut self) -> Option<i32> {
        Some(0)
    }
}

/// Ported from scripts/check/canary-smoke.py::HarnessTests::test_listener_unterminated_eof_is_not_a_record
#[test]
fn test_listener_unterminated_eof_is_not_a_record() {
    let tails = [
        LISTEN_PREFIX.to_vec(),
        line(&[LISTEN_PREFIX, b"127.0.0.1:12345"]),
        line(&[LISTEN_PREFIX, LISTENER]),
    ];
    for tail in tails {
        let capture = BoundedCapture::new(MAX_OUTPUT_BYTES);
        capture.append(&line(&[b"unrelated complete diagnostic\n", &tail]));
        capture.set_eof();
        assert_eq!(find_listener(&capture), Ok(None));
        let message = smoke_message(wait_for_listener(&mut Exited, &capture, monotonic() + 0.1));
        assert!(
            message.contains("child exited before listener announcement"),
            "{message}"
        );
    }
}

/// Ported from scripts/check/canary-smoke.py::HarnessTests::test_listener_complete_malformed_record_still_fails
#[test]
fn test_listener_complete_malformed_record_still_fails() {
    let suffix: &[u8] = b" (read-only; native execution disabled)\n";
    for record in [
        line(&[LISTEN_PREFIX, b"127.0.0.1:12345 (future mode)\n"]),
        line(&[LISTEN_PREFIX, b"127.0.0.1:18890", suffix]),
        line(&[LISTEN_PREFIX, b"127.0.0.1:18891", suffix]),
        line(&[LISTEN_PREFIX, b"192.0.2.1:12345", suffix]),
        line(&[LISTEN_PREFIX, b"example.com:12345", suffix]),
    ] {
        let capture = BoundedCapture::new(MAX_OUTPUT_BYTES);
        capture.append(&record);
        assert!(
            is_smoke(&find_listener(&capture)),
            "{}",
            String::from_utf8_lossy(&record)
        );
    }
}

/// Ported from scripts/check/canary-smoke.py::HarnessTests::test_listener_ignores_only_complete_unrelated_records
#[test]
fn test_listener_ignores_only_complete_unrelated_records() {
    let capture = BoundedCapture::new(MAX_OUTPUT_BYTES);
    capture.append(&line(&[
        b"unrelated\nnot a boundary\x0b",
        LISTEN_PREFIX,
        LISTENER,
        b"\n",
    ]));
    assert_eq!(find_listener(&capture), Ok(None));
    capture.append(&line(&[
        LISTEN_PREFIX,
        b"[::1]:12345 (read-only; native execution disabled)\n",
    ]));
    assert_eq!(find_listener(&capture), Ok(Some(("::1".to_owned(), 12345))));
}

/// Ported from scripts/check/canary-smoke.py::HarnessTests::test_listener_actual_pipe_can_pause_between_prefix_and_suffix
#[test]
fn test_listener_actual_pipe_can_pause_between_prefix_and_suffix() {
    let prefix = String::from_utf8_lossy(LISTEN_PREFIX).into_owned();
    let suffix = "127.0.0.1:12345 (read-only; native execution disabled)\n";
    let mut child = Command::new("/bin/sh")
        .args([
            "-c",
            "printf '%s' \"$1\" >&2; dd bs=1 count=1 of=/dev/null 2>/dev/null; printf '%s' \"$2\" >&2; exec sleep 10",
            "sh",
            &prefix,
            suffix,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn pausing child");
    let capture = BoundedCapture::new(MAX_OUTPUT_BYTES);
    let reader = drain_pipe(child.stderr.take().expect("stderr pipe"), &capture);
    let mut stdin = child.stdin.take().expect("stdin pipe");
    let deadline = monotonic() + 2.0;
    while capture.snapshot().len() < LISTEN_PREFIX.len() && monotonic() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        assert_eq!(capture.snapshot(), LISTEN_PREFIX);
        assert_eq!(find_listener(&capture), Ok(None));
        stdin.write_all(b"x").expect("write resume byte");
        stdin.flush().expect("flush resume byte");
        assert_eq!(
            wait_for_listener(&mut child, &capture, deadline),
            Ok(("127.0.0.1".to_owned(), 12345))
        );
    }));
    cleanup_owned_child(&mut child).expect("cleanup owned child");
    drop(stdin);
    let _ = reader.join();
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

/// Ported from scripts/check/canary-smoke.py::HarnessTests::test_capture_is_bounded
#[test]
fn test_capture_is_bounded() {
    let capture = BoundedCapture::new(8);
    capture.append(b"123456789");
    assert!(capture.oversized());
    assert_eq!(capture.snapshot(), b"12345678");
}

/// Ported from scripts/check/canary-smoke.py::HarnessTests::test_json_bounds_reject_oversized_strings_and_arrays
#[test]
fn test_json_bounds_reject_oversized_strings_and_arrays() {
    let long = py_dict! { "x" => "z".repeat(MAX_STRING_BYTES + 1) };
    assert!(is_smoke(&validate_json_bounds(&long)));
    let many = py_dict! {
        "x" => (0..=MAX_ITEMS).map(PyValue::from).collect::<Vec<_>>(),
    };
    assert!(is_smoke(&validate_json_bounds(&many)));
}

fn dict(value: PyValue) -> PyDict {
    match value {
        PyValue::Dict(dict) => dict,
        other => panic!("expected dict, got {other:?}"),
    }
}

/// Ported from scripts/check/canary-smoke.py::HarnessTests::test_auth_and_real_demo_expectations_are_strict
#[test]
fn test_auth_and_real_demo_expectations_are_strict() {
    let policy = generated_model_policy();
    let endpoint = policy
        .get("models")
        .and_then(PyValue::as_list)
        .and_then(|models| models.first())
        .and_then(|model| model.get("endpoint"))
        .expect("endpoint key");
    assert!(!endpoint.truthy());
    assert!(is_smoke(&require_status(200, 401, "unauthorized request")));
    assert_eq!(require_status(401, 401, "unauthorized request"), Ok(()));
    assert!(is_smoke(&validate_fleet(
        &dict(py_dict! { "simulated" => true, "native_execution_qualified" => false }),
        false
    )));
    assert!(is_smoke(&validate_fleet(
        &dict(py_dict! { "simulated" => false }),
        false
    )));
    assert!(is_smoke(&validate_models(&dict(py_dict! {
        "native_execution_qualified" => false,
        "models" => Vec::<PyValue>::new(),
    }))));
}

/// Ported from scripts/check/canary-smoke.py::HarnessTests::test_protected_ports_are_rejected_before_connection
#[test]
fn test_protected_ports_are_rejected_before_connection() {
    for port in [18890, 18891] {
        assert!(is_smoke(&bounded_http(
            "127.0.0.1",
            port,
            "GET",
            "/healthz",
            &[],
            None,
            monotonic() + 1.0
        )));
    }
}

/// Serve `connections` connections on loopback, each on its own thread.
fn serve(connections: usize, handler: fn(TcpStream)) -> u32 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback fixture");
    let port = listener.local_addr().expect("fixture address").port();
    thread::spawn(move || {
        for _ in 0..connections {
            match listener.accept() {
                Ok((stream, _)) => {
                    thread::spawn(move || handler(stream));
                }
                Err(_) => break,
            }
        }
    });
    u32::from(port)
}

/// Read one request head and return its request line.
fn read_request(stream: &mut TcpStream) -> String {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        match stream.read(&mut byte) {
            Ok(1) => head.push(byte[0]),
            _ => break,
        }
    }
    String::from_utf8_lossy(&head)
        .lines()
        .next()
        .unwrap_or_default()
        .to_owned()
}

fn base_http_response(status_and_headers: &str) -> String {
    format!(
        "HTTP/1.0 {status_and_headers}Server: BaseHTTP/0.6 Python/3.13\r\nDate: Wed, 21 Oct 2026 00:00:00 GMT\r\n"
    )
}

/// Ported from scripts/check/canary-smoke.py::HarnessTests::test_http_response_byte_limit
#[test]
fn test_http_response_byte_limit() {
    let port = serve(1, |mut stream| {
        read_request(&mut stream);
        let head = format!(
            "{}Content-Length: {}\r\n\r\n",
            base_http_response("200 OK\r\n"),
            MAX_HTTP_BYTES + 1
        );
        let _ = stream.write_all(head.as_bytes());
    });
    assert!(is_smoke(&bounded_http(
        "127.0.0.1",
        port,
        "GET",
        "/",
        &[],
        None,
        monotonic() + 2.0
    )));
}

/// Ported from scripts/check/canary-smoke.py::HarnessTests::test_http_request_has_total_deadline_under_slow_drip
#[test]
fn test_http_request_has_total_deadline_under_slow_drip() {
    let port = serve(1, |mut stream| {
        read_request(&mut stream);
        let head = format!(
            "{}Content-Length: 100\r\n\r\nx",
            base_http_response("200 OK\r\n")
        );
        let _ = stream.write_all(head.as_bytes());
        let _ = stream.flush();
        thread::sleep(Duration::from_millis(300));
        let _ = stream.write_all(&[b'x'; 99]);
    });
    let before = monotonic();
    let limits = HttpLimits {
        request_seconds: 0.05,
    };
    let result = bounded_http_with(
        &SystemRuntime,
        limits,
        HttpCall {
            host: "127.0.0.1",
            port,
            method: "GET",
            path: "/",
            headers: &[],
            body: None,
        },
        monotonic() + 1.0,
    );
    assert!(is_smoke(&result), "{result:?}");
    assert!(monotonic() - before < 0.5);
}

/// Ported from scripts/check/canary-smoke.py::HarnessTests::test_header_drip_cannot_extend_whole_request_deadline
#[test]
fn test_header_drip_cannot_extend_whole_request_deadline() {
    let port = serve(1, |mut stream| {
        let mut request = [0u8; 4096];
        let _ = stream.read(&mut request);
        for byte in b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}" {
            if stream.write_all(&[*byte]).is_err() {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
    });
    let before = monotonic();
    let result = bounded_http("127.0.0.1", port, "GET", "/", &[], None, before + 0.08);
    assert!(is_smoke(&result), "{result:?}");
    assert!(monotonic() - before < 0.5);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Expiry {
    Clock,
    Watchdog,
}

struct FakeState {
    expiry: Expiry,
    clock: Mutex<f64>,
    callback: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    reads: AtomicUsize,
    closed: AtomicBool,
}

struct FakeRuntime(Arc<FakeState>);

struct FakeWire(Arc<FakeState>);

struct FakeTimer;

impl Watchdog for FakeTimer {
    fn cancel(self: Box<Self>) {}
}

impl HttpRuntime for FakeRuntime {
    fn monotonic(&self) -> f64 {
        *self.0.clock.lock().expect("clock lock")
    }

    fn connect(
        &self,
        _host: &str,
        _port: u16,
        _timeout: Duration,
    ) -> std::io::Result<Box<dyn Wire>> {
        Ok(Box::new(FakeWire(Arc::clone(&self.0))))
    }

    fn start_watchdog(&self, _delay: f64, abort: Box<dyn FnOnce() + Send>) -> Box<dyn Watchdog> {
        *self.0.callback.lock().expect("callback lock") = Some(abort);
        Box::new(FakeTimer)
    }
}

impl Read for FakeWire {
    fn read(&mut self, into: &mut [u8]) -> std::io::Result<usize> {
        if self.0.reads.fetch_add(1, Ordering::SeqCst) == 0 {
            // Status and headers of a close-delimited response (length None).
            let head = b"HTTP/1.1 200 OK\r\n\r\n";
            into[..head.len()].copy_from_slice(head);
            return Ok(head.len());
        }
        // A deterministic stand-in for shutdown waking the read as EOF,
        // without relying on OS timeout/timer scheduling.
        match self.0.expiry {
            Expiry::Clock => *self.0.clock.lock().expect("clock lock") = 101.0,
            Expiry::Watchdog => {
                let callback = self.0.callback.lock().expect("callback lock").take();
                if let Some(callback) = callback {
                    callback();
                }
            }
        }
        Ok(0)
    }
}

impl Write for FakeWire {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Wire for FakeWire {
    fn set_io_timeout(&mut self, _timeout: Duration) -> std::io::Result<()> {
        Ok(())
    }

    fn shutdown_handle(&self) -> std::io::Result<Box<dyn FnOnce() + Send>> {
        Ok(Box::new(|| {}))
    }
}

impl Drop for FakeWire {
    fn drop(&mut self) {
        self.0.closed.store(true, Ordering::SeqCst);
    }
}

/// Ported from scripts/check/canary-smoke.py::HarnessTests::test_eof_after_deadline_cannot_be_reported_as_success
#[test]
fn test_eof_after_deadline_cannot_be_reported_as_success() {
    for expiry in [Expiry::Clock, Expiry::Watchdog] {
        let state = Arc::new(FakeState {
            expiry,
            clock: Mutex::new(100.0),
            callback: Mutex::new(None),
            reads: AtomicUsize::new(0),
            closed: AtomicBool::new(false),
        });
        let runtime = FakeRuntime(Arc::clone(&state));
        let result = bounded_http_with(
            &runtime,
            HttpLimits::default(),
            HttpCall {
                host: "127.0.0.1",
                port: 12345,
                method: "GET",
                path: "/",
                headers: &[],
                body: None,
            },
            100.05,
        );
        let message = smoke_message(result);
        assert!(message.contains("per-request HTTP deadline"), "{message}");
        assert!(state.closed.load(Ordering::SeqCst));
    }
}

/// Ported from scripts/check/canary-smoke.py::HarnessTests::test_clean_close_delimited_response_and_premature_content_length_eof
#[test]
fn test_clean_close_delimited_response_and_premature_content_length_eof() {
    let port = serve(2, |mut stream| {
        let request_line = read_request(&mut stream);
        let length = if request_line.starts_with("GET /complete ") {
            "2"
        } else {
            "3"
        };
        let head = format!(
            "{}Content-Length: {length}\r\n\r\n{{}}",
            base_http_response("200 OK\r\n")
        );
        let _ = stream.write_all(head.as_bytes());
        let _ = stream.flush();
    });
    let (status, _, body) = bounded_http(
        "127.0.0.1",
        port,
        "GET",
        "/complete",
        &[],
        None,
        monotonic() + 1.0,
    )
    .expect("complete response");
    assert_eq!((status, body.as_slice()), (200, &b"{}"[..]));
    let message = smoke_message(bounded_http(
        "127.0.0.1",
        port,
        "GET",
        "/truncated",
        &[],
        None,
        monotonic() + 1.0,
    ));
    assert!(
        message.contains("before declared content length"),
        "{message}"
    );
}

fn spawn_stderr_child(script: &str, args: &[&str], capture: &BoundedCapture) -> Child {
    let mut child = Command::new("/bin/sh")
        .arg("-c")
        .arg(script)
        .arg("sh")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn synthetic child");
    drain_pipe(child.stderr.take().expect("stderr pipe"), capture);
    child
}

/// Ported from scripts/check/canary-smoke.py::HarnessTests::test_startup_timeout_is_bounded
#[test]
fn test_startup_timeout_is_bounded() {
    let capture = BoundedCapture::new(MAX_OUTPUT_BYTES);
    let mut child = spawn_stderr_child("exec sleep 10", &[], &capture);
    let before = monotonic();
    let result = wait_for_listener(&mut child, &capture, monotonic() + 0.05);
    cleanup_owned_child(&mut child).expect("cleanup owned child");
    assert!(is_smoke(&result));
    assert!(monotonic() - before < 2.5);
}

/// Ported from scripts/check/canary-smoke.py::HarnessTests::test_oversized_child_output_fails_startup_with_bounded_capture
#[test]
fn test_oversized_child_output_fails_startup_with_bounded_capture() {
    let capture = BoundedCapture::new(16);
    let noise = "x".repeat(4096);
    let mut child =
        spawn_stderr_child("printf '%s' \"$1\" >&2; exec sleep 10", &[&noise], &capture);
    let deadline = monotonic() + 1.0;
    while !capture.oversized() && monotonic() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    let result = wait_for_listener(&mut child, &capture, monotonic() + 0.1);
    cleanup_owned_child(&mut child).expect("cleanup owned child");
    let message = smoke_message(result);
    assert!(message.contains("exceeded bounded output"), "{message}");
}

/// Ported from scripts/check/canary-smoke.py::HarnessTests::test_child_exit_before_listener_is_reported
#[test]
fn test_child_exit_before_listener_is_reported() {
    let capture = BoundedCapture::new(MAX_OUTPUT_BYTES);
    let mut child = spawn_stderr_child("exit 7", &[], &capture);
    let message = smoke_message(wait_for_listener(&mut child, &capture, monotonic() + 1.0));
    assert!(message.contains("exit 7"), "{message}");
    assert_eq!(
        cleanup_owned_child(&mut child),
        Ok(CleanupOutcome::AlreadyExited)
    );
}

/// Ported from scripts/check/canary-smoke.py::HarnessTests::test_term_ignoring_child_gets_handle_bound_kill_and_is_reaped
#[test]
fn test_term_ignoring_child_gets_handle_bound_kill_and_is_reaped() {
    // Deliberately slow startup proves cleanup waits for the installed
    // handler, rather than racing a fixed sleep against shell startup. An
    // ignored SIGTERM survives the final exec.
    let mut child = Command::new("/bin/sh")
        .args(["-c", "sleep 0.2; trap '' TERM; echo ready; exec sleep 30"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn TERM-ignoring child");
    let mut stdout = child.stdout.take().expect("stdout pipe");
    let (sender, receiver) = mpsc::channel();
    let reader = thread::spawn(move || {
        let mut ready = [0u8; 6];
        let outcome = stdout.read_exact(&mut ready).map(|()| ready.to_vec());
        let _ = sender.send(outcome);
    });
    let ready = receiver.recv_timeout(Duration::from_secs_f64(MAX_STARTUP_SECONDS));
    let result = match ready {
        Ok(Ok(output)) => {
            assert_eq!(
                output, b"ready\n",
                "child did not install its SIGTERM handler"
            );
            let before = monotonic();
            let result = cleanup_owned_child(&mut child);
            assert!(monotonic() - before < TERM_WAIT_SECONDS + FINAL_WAIT_SECONDS + 1.0);
            result
        }
        other => {
            let _ = cleanup_owned_child(&mut child);
            panic!("child did not install its SIGTERM handler: {other:?}");
        }
    };
    assert_eq!(result, Ok(CleanupOutcome::KilledAfterTermTimeout));
    assert!(child.try_wait().expect("reaped status").is_some());
    let _ = cleanup_owned_child(&mut child);
    let _ = reader.join();
}

/// Ported from scripts/check/canary-smoke.py::HarnessTests::test_redaction_never_emits_generated_token
#[test]
fn test_redaction_never_emits_generated_token() {
    assert!(!redact(b"token=synthetic-secret", "synthetic-secret").contains("synthetic-secret"));
}
