#![cfg(unix)]

use admiralty_runtime::session::{InputKind, LaunchSpec, SessionError, SessionHost};
use std::{
    fs, io,
    os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt},
    thread,
    time::Duration,
};
use tempfile::TempDir;

fn private_tempdir() -> io::Result<TempDir> {
    let directory = tempfile::Builder::new()
        .prefix("ai-")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in(fs::canonicalize(std::env::temp_dir())?)?;
    assert_eq!(fs::metadata(directory.path())?.mode() & 0o777, 0o700);
    Ok(directory)
}

fn private_spool(directory: &std::path::Path) -> std::path::PathBuf {
    let spool = directory.join("spool");
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&spool)
        .expect("private spool at creation");
    assert_eq!(
        fs::metadata(&spool).expect("spool mode").mode() & 0o777,
        0o700
    );
    spool
}

#[test]
fn failed_delivery_is_unknown_and_cannot_be_retried() {
    let temporary = private_tempdir().expect("canonical private-at-creation fixture");
    let spool_root = private_spool(temporary.path());
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
    assert!(
        host.wait("exited")
            .expect("reap exited child and drain capture")
            .success()
    );
}

#[test]
fn human_input_advances_epoch_and_stales_queued_automation() {
    let temporary = private_tempdir().expect("canonical private-at-creation fixture");
    let spool_root = private_spool(temporary.path());
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
    host.close_input_if_epoch("input-epoch", 1)
        .expect("close only the owned child input");
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    loop {
        if host
            .status("input-epoch")
            .expect("observe owned child cleanup")
            .exit
            .is_some()
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "owned cat must exit on EOF"
        );
        thread::sleep(Duration::from_millis(5));
    }
    assert!(
        host.wait("input-epoch")
            .expect("reap owned cat and drain readers")
            .success()
    );
}

#[test]
fn stale_automation_rejection_does_not_append_to_the_input_journal() {
    let temporary = private_tempdir().expect("canonical private-at-creation fixture");
    let spool_root = private_spool(temporary.path());
    let spool = spool_root.join("stale-epoch");
    let host = SessionHost::new();
    host.launch(LaunchSpec {
        run_id: "stale-epoch".into(),
        program: "/bin/cat".into(),
        args: Vec::new(),
        cwd: temporary.path().to_owned(),
        spool_dir: spool.clone(),
    })
    .expect("launch cat child");

    host.write_input("stale-epoch", InputKind::Human, b"human\n")
        .expect("advance input epoch with human input");
    let journal_path = spool.join("input.journal");
    let journal_before_stale_write = fs::read(&journal_path).expect("read input journal");

    assert!(matches!(
        host.write_input(
            "stale-epoch",
            InputKind::Automation { expected_epoch: 0 },
            b"must-not-be-delivered\n"
        ),
        Err(SessionError::StaleInput {
            expected: 0,
            current: 1
        })
    ));
    assert_eq!(
        fs::read(&journal_path).expect("read input journal after stale write"),
        journal_before_stale_write,
        "rejected stale automation must have no journal side effect"
    );
    host.close_input_if_epoch("stale-epoch", 1)
        .expect("close owned child input");
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    loop {
        if host
            .status("stale-epoch")
            .expect("observe owned child cleanup")
            .exit
            .is_some()
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "owned stale-epoch cat must exit on EOF"
        );
        thread::sleep(Duration::from_millis(5));
    }
    assert!(
        host.wait("stale-epoch")
            .expect("reap child and drain capture")
            .success()
    );
}
