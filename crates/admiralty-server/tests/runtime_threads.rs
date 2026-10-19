//! Actual binary startup must not scale its control-plane runtime to host CPUs
//! or an inherited provider environment's requested worker count.
#![cfg(target_os = "linux")]
use std::{
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

struct OwnedServer(Child);
impl Drop for OwnedServer {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn actual_daemon_bounds_runtime_threads_despite_large_inherited_worker_count() {
    let root = tempfile::Builder::new()
        .prefix("admiralty-runtime-threads-")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .expect("private fixture");
    let token = "runtime-fixture-token-0123456789abcdef";
    let token_path = root.path().join("owner-token");
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&token_path)
        .expect("private token")
        .write_all(token.as_bytes())
        .expect("token bytes");
    let listener = TcpListener::bind("127.0.0.1:0").expect("reserve loopback port");
    let address = listener.local_addr().expect("address");
    drop(listener);
    let mut server = OwnedServer(
        Command::new(env!("CARGO_BIN_EXE_admiraltyd"))
            .args(["--listen", &address.to_string(), "--database"])
            .arg(root.path().join("native.sqlite"))
            .arg("--token-file")
            .arg(&token_path)
            .env("TOKIO_WORKER_THREADS", "128")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start exact Cargo-built binary"),
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut ready = false;
    while Instant::now() < deadline {
        assert!(
            server.0.try_wait().expect("owned process state").is_none(),
            "daemon exited before health read"
        );
        if let Ok(mut stream) = TcpStream::connect_timeout(&address, Duration::from_millis(100)) {
            stream
                .set_read_timeout(Some(Duration::from_secs(1)))
                .expect("read bound");
            stream
                .set_write_timeout(Some(Duration::from_secs(1)))
                .expect("write bound");
            write!(stream,"GET /v2/fleet HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n").expect("health request");
            let mut response = String::new();
            if stream.take(64 * 1024).read_to_string(&mut response).is_ok()
                && response.starts_with("HTTP/1.1 200")
            {
                ready = true;
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        ready,
        "actual daemon did not become ready within the owned startup deadline"
    );
    let status = fs::read_to_string(format!("/proc/{}/status", server.0.id()))
        .expect("owned process thread count");
    let threads: usize = status
        .lines()
        .find_map(|line| line.strip_prefix("Threads:\t"))
        .expect("Threads field")
        .trim()
        .parse()
        .expect("thread counter");
    assert!(
        (3..=8).contains(&threads),
        "control-plane daemon created {threads} threads despite its bounded runtime"
    );
}
