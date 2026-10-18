use serde_json::{Value, json};
use std::{
    fs::{self, OpenOptions},
    io::{self, Read},
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};
use tempfile::{Builder, TempDir};

const OUTPUT_LIMIT: usize = 64 * 1024;
const CHILD_TIMEOUT: Duration = Duration::from_secs(15);
const PRODUCER: &str = "p11t2-offline-test";
const EPOCH: &str = "fixture-epoch-1";

struct Captured {
    bytes: Vec<u8>,
    total: usize,
}

struct ResultOutput {
    status: ExitStatus,
    stdout: Captured,
    stderr: Captured,
}

fn private_fixture_root() -> TempDir {
    let target = PathBuf::from(
        std::env::var("CARGO_TARGET_DIR").expect("packet command sets private CARGO_TARGET_DIR"),
    );
    Builder::new()
        .prefix("p11t2-offline-roundtrip-")
        .tempdir_in(target)
        .expect("create private packet fixture under the assigned target")
}

fn write_private(path: &Path, bytes: &[u8]) {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).expect("create private fixture file");
    io::Write::write_all(&mut file, bytes).expect("write private fixture file");
    file.sync_all().expect("sync private fixture file");
}

fn drain_bounded(mut reader: impl Read) -> io::Result<Captured> {
    let mut captured = Captured {
        bytes: Vec::with_capacity(OUTPUT_LIMIT),
        total: 0,
    };
    let mut buffer = [0_u8; 4096];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        captured.total = captured
            .total
            .checked_add(read)
            .expect("captured output counter stays within usize");
        let remaining = OUTPUT_LIMIT.saturating_sub(captured.bytes.len());
        captured
            .bytes
            .extend_from_slice(&buffer[..read.min(remaining)]);
    }
    Ok(captured)
}

fn invoke(root: &Path, args: &[String]) -> ResultOutput {
    let mut child = Command::new(env!("CARGO_BIN_EXE_admiralty"));
    child
        .args(args)
        .env("TMPDIR", root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = child.spawn().expect("start the CLI test binary");
    let stdout_reader = child.stdout.take().expect("capture child stdout");
    let stderr_reader = child.stderr.take().expect("capture child stderr");
    let stdout_thread = thread::spawn(move || drain_bounded(stdout_reader));
    let stderr_thread = thread::spawn(move || drain_bounded(stderr_reader));
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll owned CLI child") {
            break status;
        }
        if started.elapsed() >= CHILD_TIMEOUT {
            child
                .kill()
                .expect("kill only the timed out owned CLI child");
            let _ = child
                .wait()
                .expect("reap only the timed out owned CLI child");
            let _ = stdout_thread.join().expect("join stdout reader");
            let _ = stderr_thread.join().expect("join stderr reader");
            panic!("CLI child exceeded bounded timeout: {args:?}");
        }
        thread::sleep(Duration::from_millis(10));
    };
    let stdout = stdout_thread
        .join()
        .expect("stdout reader thread must finish")
        .expect("read bounded CLI stdout");
    let stderr = stderr_thread
        .join()
        .expect("stderr reader thread must finish")
        .expect("read bounded CLI stderr");
    assert!(
        stdout.total <= OUTPUT_LIMIT,
        "CLI stdout exceeded its bound: {args:?}"
    );
    assert!(
        stderr.total <= OUTPUT_LIMIT,
        "CLI stderr exceeded its bound: {args:?}"
    );
    ResultOutput {
        status,
        stdout,
        stderr,
    }
}

fn invoke_json(root: &Path, args: Vec<String>) -> Value {
    let output = invoke(root, &args);
    assert!(
        output.status.success(),
        "CLI failed for {args:?}: {}",
        String::from_utf8_lossy(&output.stderr.bytes)
    );
    serde_json::from_slice(&output.stdout.bytes)
        .unwrap_or_else(|error| panic!("CLI stdout was not JSON for {args:?}: {error}"))
}

fn invoke_failure(root: &Path, args: Vec<String>) -> ResultOutput {
    let output = invoke(root, &args);
    assert!(
        !output.status.success(),
        "CLI unexpectedly succeeded for {args:?}: {}",
        String::from_utf8_lossy(&output.stdout.bytes)
    );
    output
}

fn string_args(args: &[&str]) -> Vec<String> {
    args.iter().map(|arg| (*arg).to_owned()).collect()
}

fn snapshot(root: &Path, database: &Path) -> Value {
    invoke_json(
        root,
        vec![
            "snapshot".into(),
            "--database".into(),
            database.display().to_string(),
        ],
    )
}

fn expect_private_file(path: &Path) {
    let metadata = fs::metadata(path).expect("database fixture exists");
    assert!(metadata.is_file());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
    }
}

#[test]
fn offline_task_message_and_restart_roundtrip_preserves_scope_without_activation() {
    let fixture = private_fixture_root();
    let root = fixture.path();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(root, fs::Permissions::from_mode(0o700))
            .expect("make fixture root private");
        assert_eq!(
            fs::metadata(root)
                .expect("fixture root exists")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }

    let database = root.join("offline.sqlite");
    let contract_path = root.join("contract.json");
    let invalid_contract_path = root.join("invalid-contract.json");
    let message_path = root.join("message.json");
    let contract = json!({
        "schema_version": 1,
        "project_id": "p11t2-project",
        "root_id": "p11t2-root",
        "task_id": "P11T2-roundtrip",
        "revision": 1,
        "title": "Offline roundtrip fixture",
        "objective": "Preserve the exact task scope; do not execute providers or claim acceptance.",
        "non_goals": ["Do not publish the candidate", "Do not start a task process"],
        "owned_paths": ["src/offline_roundtrip.rs"],
        "forbidden_paths": ["secrets"],
        "read_first": ["README.md"],
        "dependencies": [],
        "criteria": [{
            "id": "preserved-scope",
            "text": "The negative constraints and exact check arguments remain unchanged.",
            "check_ids": ["offline-roundtrip-check"]
        }],
        "checks": [{
            "id": "offline-roundtrip-check",
            "program": "cargo",
            "args": ["test", "--exact", "tests::offline_roundtrip"],
            "cwd": ".",
            "timeout_secs": 17
        }],
        "authority_ref": "fixture:candidate-only",
        "max_attempts": 1,
        "simulated": true
    });
    let contract_bytes = serde_json::to_vec(&contract).expect("serialize bounded contract fixture");
    write_private(&contract_path, &contract_bytes);
    let valid = invoke_json(
        root,
        vec![
            "work".into(),
            "validate".into(),
            "--file".into(),
            contract_path.display().to_string(),
        ],
    );
    assert_eq!(valid["valid"], true);
    assert_eq!(valid["task_id"], "P11T2-roundtrip");
    assert_eq!(valid["native_execution_qualified"], false);
    let contract_digest = valid["contract_digest"]
        .as_str()
        .expect("validation returns exact contract digest");

    let submit = invoke_json(
        root,
        vec![
            "work".into(),
            "submit".into(),
            "--database".into(),
            database.display().to_string(),
            "--file".into(),
            contract_path.display().to_string(),
            "--producer".into(),
            PRODUCER.into(),
            "--sequence".into(),
            "1".into(),
        ],
    );
    assert_eq!(submit["offline"], true);
    assert_eq!(submit["execution_started"], false);
    expect_private_file(&database);

    let first_snapshot = snapshot(root, &database);
    let task = &first_snapshot["projection"]["tasks"]["P11T2-roundtrip"];
    assert_eq!(task["contract"]["project_id"], "p11t2-project");
    assert_eq!(task["contract"]["root_id"], "p11t2-root");
    assert_eq!(task["contract"]["task_id"], "P11T2-roundtrip");
    assert_eq!(task["contract"]["objective"], contract["objective"]);
    assert_eq!(task["contract"]["non_goals"], contract["non_goals"]);
    assert_eq!(task["contract"]["owned_paths"], contract["owned_paths"]);
    assert_eq!(
        task["contract"]["forbidden_paths"],
        contract["forbidden_paths"]
    );
    assert_eq!(task["contract"]["checks"], contract["checks"]);
    assert_eq!(task["state"], "ready");
    assert!(task["candidate_ref"].is_null());
    assert!(task["attempt_id"].is_null());
    assert!(task["owner"].is_null());
    assert_eq!(first_snapshot["projection"]["acknowledgments"], json!({}));

    let context = invoke_json(
        root,
        vec![
            "context".into(),
            "--database".into(),
            database.display().to_string(),
            "--task".into(),
            "P11T2-roundtrip".into(),
            "--epoch".into(),
            EPOCH.into(),
            "--max-tokens".into(),
            "4096".into(),
        ],
    );
    assert_eq!(context["task_id"], "P11T2-roundtrip");
    assert_eq!(context["revision"], 1);
    assert_eq!(context["context_epoch"], EPOCH);
    assert_eq!(context["contract_digest"], contract_digest);
    let capsule = context["text"]
        .as_str()
        .expect("context returns rendered capsule");
    assert!(capsule.contains("do not execute providers or claim acceptance"));
    assert!(
        capsule.contains(
            r#"non_goals=["Do not publish the candidate","Do not start a task process"]"#
        )
    );
    assert!(capsule.contains(r#"checks=[{"id":"offline-roundtrip-check","program":"cargo","args":["test","--exact","tests::offline_roundtrip"],"cwd":".","timeout_secs":17}]"#));
    assert!(capsule.contains("do not publish or claim acceptance"));

    let doctor = invoke_json(root, string_args(&["doctor"]));
    assert_eq!(doctor["runtime_enabled"], false);
    assert_eq!(doctor["native_execution_qualified"], false);

    let message = json!({
        "schema_version": 1,
        "message_id": "p11t2-message",
        "project_id": "p11t2-project",
        "sender": "p11t2-worker",
        "recipient": "p11t2-captain",
        "causation_id": null,
        "context_epoch": EPOCH,
        "authority_ref": "fixture:candidate-only",
        "payload": {
            "kind": "handoff",
            "task_id": "P11T2-roundtrip",
            "candidate_ref": "candidate:fixture-only",
            "failed_check": "not-run",
            "next_action": "Run independent verification; do not accept this candidate."
        }
    });
    write_private(
        &message_path,
        &serde_json::to_vec(&message).expect("serialize supported handoff fixture"),
    );
    let queued = invoke_json(
        root,
        vec![
            "messages".into(),
            "queue".into(),
            "--database".into(),
            database.display().to_string(),
            "--file".into(),
            message_path.display().to_string(),
            "--producer".into(),
            PRODUCER.into(),
            "--sequence".into(),
            "2".into(),
        ],
    );
    assert_eq!(queued["offline"], true);
    assert_eq!(queued["terminal_delivery"], false);
    let inbox = invoke_json(
        root,
        vec![
            "messages".into(),
            "inbox".into(),
            "--database".into(),
            database.display().to_string(),
            "--recipient".into(),
            "p11t2-captain".into(),
        ],
    );
    let rows = inbox["messages"]
        .as_array()
        .expect("inbox returns message array");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["message_id"], "p11t2-message");
    assert_eq!(rows[0]["project_id"], "p11t2-project");
    assert_eq!(rows[0]["recipient"], "p11t2-captain");
    assert_eq!(rows[0]["context_epoch"], EPOCH);
    assert_eq!(rows[0]["payload"]["task_id"], "P11T2-roundtrip");

    let acknowledgment = invoke_json(
        root,
        vec![
            "messages".into(),
            "ack".into(),
            "--database".into(),
            database.display().to_string(),
            "--message".into(),
            "p11t2-message".into(),
            "--recipient".into(),
            "p11t2-captain".into(),
            "--epoch".into(),
            EPOCH.into(),
            "--producer".into(),
            PRODUCER.into(),
            "--sequence".into(),
            "3".into(),
        ],
    );
    assert_eq!(acknowledgment["acknowledgment"], "application_receipt_only");
    let after_ack = snapshot(root, &database);
    assert_eq!(after_ack["cursor"]["event_seq"], "3");
    assert_eq!(
        after_ack["projection"]["acknowledgments"]["p11t2-message"],
        EPOCH
    );
    let include_acked = invoke_json(
        root,
        vec![
            "messages".into(),
            "inbox".into(),
            "--database".into(),
            database.display().to_string(),
            "--recipient".into(),
            "p11t2-captain".into(),
            "--include-acked".into(),
        ],
    );
    assert_eq!(
        include_acked["messages"]
            .as_array()
            .expect("inbox array")
            .len(),
        1
    );
    let default_inbox = invoke_json(
        root,
        vec![
            "messages".into(),
            "inbox".into(),
            "--database".into(),
            database.display().to_string(),
            "--recipient".into(),
            "p11t2-captain".into(),
        ],
    );
    assert_eq!(
        default_inbox["messages"]
            .as_array()
            .expect("inbox array")
            .len(),
        0
    );

    for (producer, recipient, epoch) in [
        ("p11t2-negative-recipient", "another-principal", EPOCH),
        ("p11t2-negative-epoch", "p11t2-captain", "stale-epoch"),
    ] {
        let failure = invoke_failure(
            root,
            vec![
                "messages".into(),
                "ack".into(),
                "--database".into(),
                database.display().to_string(),
                "--message".into(),
                "p11t2-message".into(),
                "--recipient".into(),
                recipient.into(),
                "--epoch".into(),
                epoch.into(),
                "--producer".into(),
                producer.into(),
                "--sequence".into(),
                "1".into(),
            ],
        );
        assert!(
            String::from_utf8_lossy(&failure.stderr.bytes)
                .contains("recipient or context epoch mismatch"),
            "unsupported ACK must fail with the existing reducer reason"
        );
        let after_rejection = snapshot(root, &database);
        assert_eq!(
            after_rejection["cursor"]["event_seq"],
            after_ack["cursor"]["event_seq"]
        );
        assert_eq!(after_rejection["projection"], after_ack["projection"]);
        assert_eq!(
            after_rejection["integrity_incidents"],
            after_ack["integrity_incidents"]
        );
    }

    // Every invoke starts a fresh actual CLI process; durable state must survive them.
    let restarted_snapshot = snapshot(root, &database);
    assert_eq!(restarted_snapshot["projection"], after_ack["projection"]);
    let restarted_context = invoke_json(
        root,
        vec![
            "context".into(),
            "--database".into(),
            database.display().to_string(),
            "--task".into(),
            "P11T2-roundtrip".into(),
            "--epoch".into(),
            EPOCH.into(),
        ],
    );
    assert_eq!(restarted_context["contract_digest"], contract_digest);
    assert_eq!(restarted_context["text"], context["text"]);

    let invalid_contract = json!({
        "schema_version": 1, "project_id": "p11t2-project", "root_id": "p11t2-root",
        "task_id": "P11T2-invalid", "revision": 1, "title": "Invalid fixture",
        "objective": "Invalid path must fail before store creation", "non_goals": [],
        "owned_paths": ["../escape"], "forbidden_paths": [], "read_first": [],
        "dependencies": [], "criteria": [{"id":"c","text":"x","check_ids":["x"]}],
        "checks": [{"id":"x","program":"true","args":[],"cwd":".","timeout_secs":1}],
        "authority_ref": "fixture:candidate-only", "max_attempts": 1, "simulated": true
    });
    write_private(
        &invalid_contract_path,
        &serde_json::to_vec(&invalid_contract).expect("serialize invalid path fixture"),
    );
    let invalid_database = root.join("invalid-must-not-exist.sqlite");
    invoke_failure(
        root,
        vec![
            "work".into(),
            "submit".into(),
            "--database".into(),
            invalid_database.display().to_string(),
            "--file".into(),
            invalid_contract_path.display().to_string(),
            "--producer".into(),
            "p11t2-invalid".into(),
            "--sequence".into(),
            "1".into(),
        ],
    );
    assert!(
        !invalid_database.exists(),
        "invalid contract must not create its database"
    );

    let missing_database = root.join("missing.sqlite");
    invoke_failure(
        root,
        vec![
            "snapshot".into(),
            "--database".into(),
            missing_database.display().to_string(),
        ],
    );
    assert!(
        !missing_database.exists(),
        "snapshot of missing DB must not create it"
    );

    let missing_schemas = root.join("missing-schemas");
    invoke_failure(
        root,
        vec![
            "contracts".into(),
            "check".into(),
            "--directory".into(),
            missing_schemas.display().to_string(),
        ],
    );
    assert!(
        !missing_schemas.exists(),
        "schema check must not create its input directory"
    );

    let unknown_database = root.join("unknown-must-not-exist.sqlite");
    invoke_failure(
        root,
        vec![
            "unknown-command".into(),
            "--database".into(),
            unknown_database.display().to_string(),
        ],
    );
    assert!(
        !unknown_database.exists(),
        "unknown command must not create a database"
    );
}
