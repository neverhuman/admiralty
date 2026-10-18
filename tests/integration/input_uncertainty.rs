#![cfg(unix)]

use admiralty_runtime::session::{InputKind, LaunchSpec, SessionError, SessionHost};
use std::{fs, os::unix::fs::PermissionsExt, thread, time::Duration};
use tempfile::tempdir;

#[test]
fn failed_delivery_is_unknown_and_cannot_be_retried() {
    let temporary = tempdir().expect("private test fixture directory");
    let spool_root = temporary.path().join("spool");
    fs::create_dir(&spool_root).expect("create spool root");
    fs::set_permissions(&spool_root, fs::Permissions::from_mode(0o700))
        .expect("make spool root private");
    let spool = spool_root.join("exited");
    let host = SessionHost::new();
    host.launch(LaunchSpec {
        run_id: "exited".into(),
        program: "/bin/sh".into(),
        args: vec!["-c".into(), "exit 0".into()],
        cwd: temporary.path().to_owned(),
        spool_dir: spool.clone(),
    })
    .expect("launch short-lived child");

    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    loop {
        if host.status("exited").expect("observe child").exit.is_some() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "child should exit promptly"
        );
        thread::sleep(Duration::from_millis(5));
    }

    let first = host.write_input("exited", InputKind::Human, b"first\n");
    assert!(matches!(first, Err(SessionError::Io(_))));
    assert!(
        host.status("exited")
            .expect("observe unknown input")
            .input_outcome_unknown
    );
    let journal_path = spool.join("input.journal");
    let journal_before_retry = fs::read(&journal_path).expect("read input journal");
    assert!(String::from_utf8_lossy(&journal_before_retry).contains("outcome_unknown"));

    assert!(matches!(
        host.write_input(
            "exited",
            InputKind::Automation { expected_epoch: 0 },
            b"retry\n"
        ),
        Err(SessionError::InputOutcomeUnknown)
    ));
    assert_eq!(
        fs::read(&journal_path).expect("read input journal after rejected retry"),
        journal_before_retry
    );
}

#[test]
fn human_input_advances_epoch_and_stales_queued_automation() {
    let temporary = tempdir().expect("private test fixture directory");
    let spool_root = temporary.path().join("spool");
    fs::create_dir(&spool_root).expect("create spool root");
    fs::set_permissions(&spool_root, fs::Permissions::from_mode(0o700))
        .expect("make spool root private");
    let host = SessionHost::new();
    host.launch(LaunchSpec {
        run_id: "input-epoch".into(),
        program: "/bin/cat".into(),
        args: Vec::new(),
        cwd: temporary.path().to_owned(),
        spool_dir: spool_root.join("input-epoch"),
    })
    .expect("launch cat child");

    let receipt = host
        .write_input("input-epoch", InputKind::Human, b"human\n")
        .expect("deliver human input");
    assert_eq!(receipt.input_epoch, 1);
    assert!(matches!(
        host.write_input(
            "input-epoch",
            InputKind::Automation { expected_epoch: 0 },
            b"stale\n"
        ),
        Err(SessionError::StaleInput {
            expected: 0,
            current: 1
        })
    ));
}
