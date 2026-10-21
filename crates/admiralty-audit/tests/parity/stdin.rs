//! Ported from tests/test_sc_audit_stdin.py: children must never inherit the
//! ticker tty (SIGTTIN stop under `timeout` in screen).

use std::time::Duration;

use admiralty_audit::ctx::Audit;
use admiralty_audit::host::fake::FakeHost;
use admiralty_audit::{AuditConfig, Host, RealHost};

use crate::common::at;

/// Ported from tests/test_sc_audit_stdin.py::ChildStdinDetached::test_run_passes_devnull_stdin
#[test]
fn test_run_passes_devnull_stdin() {
    let out = RealHost.run(
        &[
            "sh",
            "-c",
            // /dev/fd works on both Linux and Darwin; /proc is Linux-only.
            "test /dev/fd/0 -ef /dev/null && printf '/dev/null\\n'",
        ],
        Some(Duration::from_secs(10)),
        None,
    );
    assert_eq!(out.rc, 0, "{out:?}");
    assert_eq!(out.stdout.trim(), "/dev/null");
}

fn rust_sources(dir: &std::path::Path, out: &mut Vec<(String, String)>) {
    for entry in std::fs::read_dir(dir).expect("read src").flatten() {
        let p = entry.path();
        if p.is_dir() {
            rust_sources(&p, out);
        } else if p.extension().is_some_and(|e| e == "rs") {
            out.push((
                p.to_string_lossy().to_string(),
                std::fs::read_to_string(&p).expect("read"),
            ));
        }
    }
}

/// Ported from tests/test_sc_audit_stdin.py::ChildStdinDetached::test_every_subprocess_call_detaches_stdin
#[test]
fn test_every_subprocess_call_detaches_stdin() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_sources(&src, &mut files);
    let mut calls = 0;
    for (path, text) in &files {
        for (i, _) in text.match_indices("Command::new(") {
            calls += 1;
            let window = &text[i..(i + 400).min(text.len())];
            assert!(
                window.contains(".stdin(Stdio::null())"),
                "{path}: Command without stdin=null"
            );
        }
    }
    assert!(calls > 0, "the single spawn site must exist");
}

/// Ported from tests/test_sc_audit_stdin.py::ChildStdinDetached::test_gpu_ssh_uses_n
#[test]
fn test_gpu_ssh_uses_n() {
    let cfg = AuditConfig::with_home("/nonexistent-home");
    let host = FakeHost::new(at("2026-10-18T16:45:00+00:00"));
    let mut audit = Audit::new(&cfg, &host);
    let _ = audit.gpu_sample("xbabe1");
    let _ = audit.remote_screens("xbabe3");
    let _ = audit.remote_hash("xbabe3", "worker");
    let calls = host.calls();
    assert_eq!(calls.len(), 3);
    for argv in calls {
        assert_eq!(&argv[..2], ["ssh", "-n"], "{argv:?}");
    }
}
