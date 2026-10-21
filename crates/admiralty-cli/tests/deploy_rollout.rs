//! scripts/deploy/rollout.sh against real `admiralty` binaries and a stub
//! `systemctl`: success keeps the new version; a version that never becomes
//! ready is rolled back automatically.
#![cfg(unix)]
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
};

fn repository() -> PathBuf {
    fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")).expect("repository")
}

fn release(root: &Path, version: &str) {
    let bin = root.join(version).join("bin");
    fs::create_dir_all(&bin).expect("bin");
    let cli = Path::new(env!("CARGO_BIN_EXE_admiralty"));
    if fs::hard_link(cli, bin.join("admiralty")).is_err() {
        fs::copy(cli, bin.join("admiralty")).expect("copy cli");
    }
    fs::write(
        root.join(version).join("MANIFEST.json"),
        format!("{{\"v\":\"{version}\"}}\n"),
    )
    .expect("manifest");
}

struct Fixture {
    _scratch: tempfile::TempDir,
    root: PathBuf,
    stub_bin: PathBuf,
    calls: PathBuf,
}

fn fixture() -> Fixture {
    let scratch = tempfile::tempdir().expect("tempdir");
    let base = fs::canonicalize(scratch.path()).expect("canonical");
    let root = base.join("releases");
    fs::create_dir(&root).expect("root");
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).expect("chmod");
    release(&root, "v1");
    release(&root, "v2");
    let stub_bin = base.join("stub-bin");
    fs::create_dir(&stub_bin).expect("stub");
    let calls = base.join("systemctl.calls");
    fs::write(
        stub_bin.join("systemctl"),
        format!("#!/bin/sh\necho \"$@\" >> '{}'\n", calls.display()),
    )
    .expect("stub systemctl");
    fs::set_permissions(
        stub_bin.join("systemctl"),
        fs::Permissions::from_mode(0o755),
    )
    .expect("chmod");
    Fixture {
        _scratch: scratch,
        root,
        stub_bin,
        calls,
    }
}

fn rollout(fixture: &Fixture, listen: &str, extra: &[&str]) -> std::process::Output {
    let path = format!(
        "{}:{}",
        fixture.stub_bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    Command::new("bash")
        .arg(repository().join("scripts/deploy/rollout.sh"))
        .arg("--releases-root")
        .arg(&fixture.root)
        .args([
            "--version",
            "v2",
            "--listen",
            listen,
            "--timeout-seconds",
            "1",
        ])
        .args(extra)
        .env("PATH", path)
        .output()
        .expect("run rollout")
}

fn current(root: &Path) -> PathBuf {
    fs::read_link(root.join("current")).expect("current link")
}

#[test]
fn unready_release_is_rolled_back_and_ready_release_stays_active() {
    let fixture = fixture();
    let activate = Command::new(env!("CARGO_BIN_EXE_admiralty"))
        .args(["release", "activate", "--releases-root"])
        .arg(&fixture.root)
        .args(["--version", "v1"])
        .output()
        .expect("activate v1");
    assert!(activate.status.success());

    // Nothing listens on this port: v2 never becomes ready.
    let closed = TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .expect("port")
        .to_string();
    let failed = rollout(&fixture, &closed, &[]);
    assert_eq!(
        failed.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&failed.stderr)
    );
    assert!(
        String::from_utf8_lossy(&failed.stderr).contains("rolling back"),
        "{}",
        String::from_utf8_lossy(&failed.stderr)
    );
    assert_eq!(current(&fixture.root), Path::new("v1"));
    let calls = fs::read_to_string(&fixture.calls).expect("calls");
    assert_eq!(
        calls.lines().count(),
        2,
        "restart for v2, then for the rollback"
    );
    assert!(
        calls
            .lines()
            .all(|l| l == "restart admiraltyd admiralty-observe")
    );

    // A ready endpoint keeps v2 active.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let address = listener.local_addr().expect("address").to_string();
    let server = std::thread::spawn(move || {
        if let Some(Ok(mut stream)) = listener.incoming().next() {
            let mut request = Vec::new();
            let mut buffer = [0u8; 512];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                match stream.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => request.extend_from_slice(&buffer[..n]),
                }
            }
            let body = r#"{"status":"ready"}"#;
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
        }
    });
    let ready = rollout(&fixture, &address, &["--user", "--units", "admiraltyd"]);
    server.join().expect("server");
    assert!(
        ready.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&ready.stdout),
        String::from_utf8_lossy(&ready.stderr)
    );
    assert_eq!(current(&fixture.root), Path::new("v2"));
    let calls = fs::read_to_string(&fixture.calls).expect("calls");
    assert_eq!(calls.lines().last(), Some("--user restart admiraltyd"));
}

#[test]
fn dry_run_and_invalid_arguments_change_nothing() {
    let fixture = fixture();
    let dry = rollout(&fixture, "127.0.0.1:9", &["--dry-run"]);
    assert!(dry.status.success());
    assert!(fs::symlink_metadata(fixture.root.join("current")).is_err());
    let script = repository().join("scripts/deploy/rollout.sh");
    for args in [
        vec!["--releases-root", "relative", "--version", "v2"],
        vec!["--releases-root", "/abs", "--version", "a/b"],
        vec!["--version", "v2"],
        vec![
            "--releases-root",
            "/abs",
            "--version",
            "v2",
            "--timeout-seconds",
            "soon",
        ],
    ] {
        let output = Command::new("bash")
            .arg(&script)
            .args(&args)
            .output()
            .expect("run");
        assert_eq!(output.status.code(), Some(2), "{args:?}");
    }
    let missing = Command::new("bash")
        .arg(&script)
        .arg("--releases-root")
        .arg(&fixture.root)
        .args(["--version", "v9"])
        .output()
        .expect("run");
    assert_eq!(missing.status.code(), Some(2));
    assert!(!fixture.calls.exists(), "systemctl never ran");
}
