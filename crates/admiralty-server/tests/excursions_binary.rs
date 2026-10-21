//! Real `admiraltyd` incident-journal smoke, including restart. Rust port of
//! the retired scripts/check/excursions-smoke.py: same requests, same
//! assertions, now part of `cargo test` instead of an unwired script.
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

const TOKEN: &str = "excursions-smoke-token-0123456789abcdef0123";

fn write_private(path: &Path, bytes: &[u8]) {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(path)
        .and_then(|mut file| file.write_all(bytes))
        .expect("private fixture");
}

fn post(address: SocketAddr, path: &str, value: &Value, credential: &str) -> (u16, Value) {
    let body = serde_json::to_vec(value).expect("body");
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(3)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("timeout");
    write!(
        stream,
        "POST /v2/incidents/{path} HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {credential}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .expect("head");
    stream.write_all(&body).expect("body");
    let mut response = Vec::new();
    stream
        .take(2 * 1024 * 1024 + 4096)
        .read_to_end(&mut response)
        .expect("response");
    let text = String::from_utf8_lossy(&response);
    let (head, body) = text.split_once("\r\n\r\n").expect("http response");
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .expect("status");
    let body = if head
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked")
    {
        dechunk(body)
    } else {
        body.to_owned()
    };
    (status, serde_json::from_str(&body).unwrap_or(Value::Null))
}

fn dechunk(mut body: &str) -> String {
    let mut out = String::new();
    while let Some((size, rest)) = body.split_once("\r\n") {
        let Ok(size) = usize::from_str_radix(size.trim(), 16) else {
            break;
        };
        if size == 0 || rest.len() < size {
            break;
        }
        out.push_str(&rest[..size]);
        body = rest[size..].trim_start_matches("\r\n");
    }
    out
}

struct Daemon(Child);

impl Drop for Daemon {
    /// Graceful SIGTERM first (as the retired script did), then a bounded kill.
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(pid) = i32::try_from(self.0.id())
            .ok()
            .and_then(rustix::process::Pid::from_raw)
        {
            let _ = rustix::process::kill_process(pid, rustix::process::Signal::TERM);
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline {
                if matches!(self.0.try_wait(), Ok(Some(_))) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn start(root: &Path, address: SocketAddr) -> Daemon {
    // Launch hermetically, like intake_startup.rs: no inherited environment (a
    // self-hosted runner's variables must not change startup), a private
    // TMPDIR, and stderr kept so a failed start explains itself.
    let log_path = root.join("daemon.log");
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .expect("daemon log");
    let stderr_text = || std::fs::read_to_string(&log_path).unwrap_or_default();
    let child = Command::new(env!("CARGO_BIN_EXE_admiraltyd"))
        .env_clear()
        .env("TMPDIR", root)
        .arg("--database")
        .arg(root.join("incidents.db"))
        .arg("--listen")
        .arg(address.to_string())
        .arg("--token-file")
        .arg(root.join("token"))
        .arg("--native-config")
        .arg(root.join("config.json"))
        .env("ADMIRALTY_LOG", "warn")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(log))
        .spawn()
        .expect("spawn admiraltyd");
    let mut daemon = Daemon(child);
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if let Ok(Some(status)) = daemon.0.try_wait() {
            panic!(
                "isolated binary exited before readiness: {status}\n--- admiraltyd stderr ---\n{}",
                stderr_text()
            );
        }
        if let Ok(mut stream) = TcpStream::connect_timeout(&address, Duration::from_millis(500)) {
            let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
            let _ =
                stream.write_all(b"GET /healthz HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
            let mut response = String::new();
            let _ = stream.read_to_string(&mut response);
            if response.starts_with("HTTP/1.1 200") {
                return daemon;
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!(
        "isolated binary readiness deadline\n--- admiraltyd stderr ---\n{}",
        stderr_text()
    );
}

#[cfg(unix)]
#[test]
fn incident_journal_survives_restart_with_scope_isolation_and_exact_retry() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path().join("scratch");
    std::fs::create_dir(&root).expect("scratch");
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).expect("chmod");
    let scope = json!({"project":"p27-smoke","root":"root","task":"task","contract_revision":1,
        "contract_digest":format!("sha256:{}", "a".repeat(64)),
        "protected_oracle_digest":format!("sha256:{}", "b".repeat(64))});
    write_private(&root.join("token"), TOKEN.as_bytes());
    write_private(
        &root.join("config.json"),
        &serde_json::to_vec(&json!({"schema_version":1,"operator":null,"agents":[],
            "excursions":[{"scope":scope,"actions":["read","observe","propose"],"evidence_digests":[]}]}))
        .expect("config"),
    );
    let address = TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .expect("ephemeral port");
    let incident = |identity: &str, parent: Option<&str>| {
        json!({"scope":scope,"event":{"event_id":identity,"family_signature":"storage-timeout",
            "kind":if parent.is_some() {"escalation"} else {"failure"},"cause_parent":parent,
            "evidence_digests":[],"observed_tick":null}})
    };
    let call = |path: &str, value: &Value, expected: u16| {
        let (status, body) = post(address, path, value, TOKEN);
        assert_eq!(status, expected, "{path}: {body}");
        body
    };

    let daemon = start(&root, address);
    assert_eq!(call("scopes", &json!({}), 200), json!([scope]));
    assert_eq!(post(address, "read", &scope, "wrong-token").0, 401);
    let mut foreign = scope.clone();
    foreign["root"] = json!("foreign");
    call("read", &foreign, 403);
    let initial = json!({"scope":scope,"command_id":"initial","expected_revision":0,
        "operation":{"kind":"observe","incidents":[incident("failure", None), incident("escalation", Some("failure"))]}});
    let receipt = call("commands", &initial, 200);
    assert_eq!(
        call("read", &scope, 200)["state"]["episodes"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );
    let recurrence = json!({"scope":scope,"command_id":"recurrence","expected_revision":1,
        "operation":{"kind":"observe","incidents":[incident("recurrence", None)]}});
    call("commands", &recurrence, 200);
    let state = call("read", &scope, 200);
    assert_eq!(state["state"]["episodes"].as_array().map(Vec::len), Some(2));
    let fields = json!({"scope":"p27-smoke/root/task","steps":"Replay recorded failure and escalation",
        "expected":"One incident per causal lineage","actual":"Writer contention observed",
        "tried_approaches":"Do not add retries; prior retry failed",
        "candidate_fix":"Inspect bounded queue ownership","measurable_outcome":"No duplicate episodes"});
    let proposal = json!({"scope":scope,"command_id":"proposal","expected_revision":2,
        "operation":{"kind":"propose","episode_id":state["state"]["episodes"][0]["episode_id"],
            "reproduction":{"fields":fields}}});
    let mut unsafe_proposal = proposal.clone();
    unsafe_proposal["operation"]["reproduction"]["fields"]["actual"] =
        json!("token=secret-fixture");
    call("commands", &unsafe_proposal, 422);
    call("commands", &proposal, 200);
    let replay = call("commands", &initial, 200);
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["receipt"], receipt["receipt"]);
    let mut stale = initial.clone();
    stale["command_id"] = json!("stale");
    call("commands", &stale, 409);
    let expected = call("read", &scope, 200);
    let local = &expected["proposals"][0]["proposal"];
    assert_eq!(local["execution_granted"], false);
    assert_eq!(local["external_publication_allowed"], false);
    assert_eq!(local["proposed_investigator_max_model_turns"], 1);
    assert!(local["uncertainty"].to_string().contains("unknown"));
    assert_eq!(local["reproduction_contract"]["fields"], fields);
    drop(daemon);

    let _restarted = start(&root, address);
    assert_eq!(
        call("read", &scope, 200),
        expected,
        "durable across restart"
    );
    assert_eq!(call("commands", &proposal, 200)["replayed"], true);
}
