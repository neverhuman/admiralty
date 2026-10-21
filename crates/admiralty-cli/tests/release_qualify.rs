//! End-to-end check of `admiralty release qualify` (the Rust replacement for
//! scripts/release/qualify_native.py) against this build's real binary.
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

#[test]
#[ignore = "heavy: copies and gzips the full binary three times (about 2 min in a debug build); run with --ignored or with --release before a release"]
fn qualify_rehearses_stage_archive_install_and_installed_demo() {
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_admiralty"));
    let repository = fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
        .expect("repository root");
    let head = Command::new("git")
        .arg("-C")
        .arg(&repository)
        .args(["rev-parse", "HEAD"])
        .output()
        .expect("git rev-parse");
    assert!(head.status.success(), "qualification needs a Git checkout");
    let commit = String::from_utf8(head.stdout)
        .expect("utf8")
        .trim()
        .to_owned();
    let scratch = tempfile::tempdir().expect("tempdir");
    let root = fs::canonicalize(scratch.path()).expect("canonical scratch");
    let assets = root.join("dist");
    fs::create_dir_all(assets.join("assets")).expect("assets");
    fs::write(
        assets.join("index.html"),
        "<!doctype html><title>Admiralty</title>",
    )
    .expect("index");
    fs::write(assets.join("assets/app.js"), "console.log('fixture')").expect("script");
    let output = root.join("evidence");

    let run = Command::new(&binary)
        .args(["release", "qualify", "--binary"])
        .arg(&binary)
        .arg("--assets")
        .arg(&assets)
        .arg("--repository")
        .arg(&repository)
        .arg("--output")
        .arg(&output)
        .args(["--source-commit", &commit, "--target", "test-host"])
        .output()
        .expect("run qualify");
    assert!(
        run.status.success(),
        "qualify failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&run.stdout).expect("report json");
    assert_eq!(report["status"], "passed");
    assert_eq!(report["reproducible_archive"], true);
    assert_eq!(report["tools_on_path"], false);
    assert_eq!(report["service_activation_qualified"], false);
    assert!(report["demo_task_count"].as_u64().is_some_and(|n| n > 0));
    let labels = report["commands"]
        .as_array()
        .expect("commands")
        .iter()
        .map(|c| {
            (
                c["label"].as_str().unwrap_or("").to_owned(),
                c["exit_code"].as_i64(),
            )
        })
        .collect::<Vec<_>>();
    let expected = [
        "stage",
        "archive-0",
        "archive-1",
        "install",
        "installed-doctor",
        "installed-demo",
        "existing-destination-refused",
        "wrong-archive-pin-refused",
    ];
    assert_eq!(
        labels.iter().map(|(l, _)| l.as_str()).collect::<Vec<_>>(),
        expected
    );
    assert!(labels[..6].iter().all(|(_, code)| *code == Some(0)));
    assert!(labels[6..].iter().all(|(_, code)| *code != Some(0)));
    let stored: serde_json::Value =
        serde_json::from_slice(&fs::read(output.join("qualification.json")).expect("receipt"))
            .expect("receipt json");
    assert_eq!(stored, report, "stdout and retained receipt agree");
    assert!(!output.join("bad-install").exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |path: &Path| fs::metadata(path).expect("metadata").permissions().mode() & 0o777;
        assert_eq!(mode(&output), 0o700);
        assert_eq!(mode(&output.join("qualification.json")), 0o600);
    }
    // Evidence directories are never reused.
    let again = Command::new(&binary)
        .args(["release", "qualify", "--assets"])
        .arg(&assets)
        .arg("--repository")
        .arg(&repository)
        .arg("--output")
        .arg(&output)
        .args(["--source-commit", &commit, "--target", "test-host"])
        .output()
        .expect("rerun");
    assert!(!again.status.success());
    let short = Command::new(&binary)
        .args(["release", "qualify", "--assets"])
        .arg(&assets)
        .arg("--repository")
        .arg(&repository)
        .arg("--output")
        .arg(root.join("other"))
        .args(["--source-commit", "abc123", "--target", "test-host"])
        .output()
        .expect("short commit");
    assert!(!short.status.success());
    assert!(
        !root.join("other").exists(),
        "invalid input creates no evidence"
    );
}
