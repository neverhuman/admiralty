use admiralty_core::{DomainEvent, SourceIdentity};
use admiralty_store::Store;
use serde_json::json;
use std::{
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

fn event(id: &str) -> DomainEvent {
    serde_json::from_value(json!({"kind":"task_submitted","contract":{
        "schema_version":1,"project_id":"project","root_id":"root","task_id":id,"revision":1,
        "title":"Recover committed task","objective":"Preserve acknowledged state","non_goals":[],
        "owned_paths":["src/task.rs"],"forbidden_paths":[],"read_first":[],"dependencies":[],
        "criteria":[{"id":"C1","text":"Recovery check passes","check_ids":["T1"]}],
        "checks":[{"id":"T1","program":"cargo","args":["test"],"cwd":".","timeout_secs":60}],
        "authority_ref":"policy:test","max_attempts":1,"simulated":true
    }}))
    .expect("valid task")
}

#[test]
fn crash_writer_child() {
    let Some(path) = std::env::var_os("ADMIRALTY_STORE_CRASH_TEST_DB") else {
        return;
    };
    let path = PathBuf::from(path);
    let store = Store::open(&path).expect("child store");
    store
        .append(
            SourceIdentity {
                producer_id: "child".into(),
                incarnation: "boot-1".into(),
                sequence: "1".into(),
            },
            event("committed"),
            "2026-10-18T12:00:00Z".into(),
        )
        .expect("durable acknowledged append");
    // Fault injection: a second raw SQLite connection leaves a write uncommitted.
    // Production writers must use Store; this transaction exists only to test recovery.
    let mut fault = rusqlite::Connection::open(&path).expect("fault connection");
    let transaction = fault.transaction().expect("uncommitted transaction");
    transaction.execute("INSERT INTO domain_events(producer_id,incarnation,producer_seq,digest,observed_utc,payload) VALUES('fault','boot',1,'uncommitted','now',?1)",
        [serde_json::to_string(&event("uncommitted")).expect("serialize")]).expect("uncommitted insertion");
    std::fs::write(
        path.with_extension("ready"),
        b"acknowledged event 1; transaction 2 uncommitted",
    )
    .expect("ready marker");
    loop {
        std::hint::black_box(&store);
        std::hint::black_box(&transaction);
        std::thread::sleep(Duration::from_secs(1));
    }
}

#[test]
fn abrupt_process_kill_preserves_acknowledged_events_and_rolls_back_uncommitted_write() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("state.sqlite");
    let mut child = Command::new(std::env::current_exe().expect("test executable"))
        .args(["--exact", "crash_writer_child", "--nocapture"])
        .env("ADMIRALTY_STORE_CRASH_TEST_DB", &path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn crash child");
    let deadline = Instant::now() + Duration::from_secs(15);
    while !path.with_extension("ready").exists() {
        if let Some(status) = child.try_wait().expect("child status") {
            panic!("child exited before ready: {status}");
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("crash child did not acknowledge write within deadline");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    child
        .kill()
        .expect("abruptly kill process without store Drop");
    assert!(!child.wait().expect("wait child").success());
    let reopened = Store::open(&path).expect("WAL recovery and owner lock release");
    let snapshot = reopened.snapshot().expect("snapshot");
    assert_eq!(snapshot.cursor.event_seq, "1");
    assert_eq!(snapshot.projection.tasks.len(), 1);
    assert_eq!(reopened.events_after(0, 100).expect("events").len(), 1);
    assert!(
        snapshot
            .projection
            .tasks
            .keys()
            .all(|id| id.as_str() == "committed")
    );
    assert_eq!(
        reopened.rebuild().expect("replay after crash").projection,
        snapshot.projection
    );
}
