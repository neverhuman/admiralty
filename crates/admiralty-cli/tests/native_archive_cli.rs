//! Actual Rust CLI children and an owned synthetic stage. No shell, Python,
//! network, provider, install, publishing, or runtime acceptance is involved.
#![cfg(unix)]
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt, symlink},
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

const CAPTURE_LIMIT: u64 = 64 * 1024;
const CHILD_TIMEOUT: Duration = Duration::from_secs(15);
struct CliOutput {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn write_private(path: &Path, bytes: &[u8]) {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .expect("new private input");
    file.write_all(bytes).expect("write private input");
    file.sync_all().expect("sync private input");
    assert_eq!(
        file.metadata().expect("input metadata").mode() & 0o777,
        0o600
    );
}
fn capture(mut file: File) -> Vec<u8> {
    use std::io::Seek;
    file.rewind().expect("capture rewind");
    let mut bytes = Vec::new();
    file.take(CAPTURE_LIMIT + 1)
        .read_to_end(&mut bytes)
        .expect("bounded capture");
    assert!(
        bytes.len() as u64 <= CAPTURE_LIMIT,
        "CLI output exceeded bound"
    );
    bytes
}
fn invoke(root: &Path, args: &[String]) -> CliOutput {
    let id = uuid::Uuid::new_v4().to_string();
    let make_capture = |suffix| {
        OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(root.join(format!("capture-{id}-{suffix}")))
            .expect("private child capture")
    };
    let stdout = make_capture("stdout");
    let stderr = make_capture("stderr");
    let mut child = Command::new(env!("CARGO_BIN_EXE_admiralty"))
        .args(args)
        .current_dir(root)
        .env_clear()
        .env("TMPDIR", root)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout.try_clone().expect("stdout handle")))
        .stderr(Stdio::from(stderr.try_clone().expect("stderr handle")))
        .spawn()
        .expect("actual Rust CLI child");
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll owned CLI child") {
            break status;
        }
        if start.elapsed() >= CHILD_TIMEOUT
            || stdout.metadata().expect("stdout size").len() > CAPTURE_LIMIT
            || stderr.metadata().expect("stderr size").len() > CAPTURE_LIMIT
        {
            child.kill().expect("kill only this owned CLI child");
            let _ = child.wait();
            panic!("owned CLI exceeded deadline/capture bound");
        }
        thread::sleep(Duration::from_millis(10));
    };
    CliOutput {
        status,
        stdout: capture(stdout),
        stderr: capture(stderr),
    }
}
fn text(path: &Path) -> String {
    path.to_str().expect("fixture UTF8 path").to_owned()
}
struct Fixture {
    root: tempfile::TempDir,
    stage: PathBuf,
    expected: PathBuf,
    manifest: Value,
    raw_hash: String,
}
fn fixture() -> Fixture {
    let root = tempfile::Builder::new()
        .prefix("native-archive-cli-")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in(
            std::env::temp_dir()
                .canonicalize()
                .expect("canonical private tmp"),
        )
        .expect("private fixture");
    let sources = [
        (
            "target/release/admiralty",
            b"synthetic binary payload".as_slice(),
            0o755,
        ),
        (
            "apps/dashboard/dist/index.html",
            b"<main>owned CLI fixture</main>".as_slice(),
            0o644,
        ),
        (
            "docs/operators/release.md",
            b"owned release fixture".as_slice(),
            0o644,
        ),
        (
            "schemas/domain-event-v2.schema.json",
            include_bytes!("../../../schemas/domain-event-v2.schema.json").as_slice(),
            0o644,
        ),
        (
            "packages/contracts/generated/validators.mjs",
            b"export const validateDomainEventV2 = () => true;".as_slice(),
            0o644,
        ),
    ];
    let mut inputs = Vec::new();
    for (source, bytes, mode) in sources {
        let path = root.path().join(source);
        fs::create_dir_all(path.parent().expect("source parent")).expect("source parents");
        fs::write(&path, bytes).expect("owned source");
        inputs.push(json!({"source":source,"expected_sha256":digest(bytes),"mode":mode}));
    }
    let allowlist = root.path().join("allowlist.json");
    write_private(
        &allowlist,
        &serde_json::to_vec(&inputs).expect("allowlist JSON"),
    );
    let stage = root.path().join("stage");
    let output = invoke(
        root.path(),
        &[
            "release".into(),
            "stage".into(),
            "--input-root".into(),
            text(root.path()),
            "--output".into(),
            text(&stage),
            "--allowlist".into(),
            text(&allowlist),
            "--version".into(),
            "2.0.0".into(),
            "--target".into(),
            "fixture-unix".into(),
            "--source-commit".into(),
            "a".repeat(40),
        ],
    );
    assert!(
        output.status.success(),
        "actual stage failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let receipt: Value = serde_json::from_slice(&output.stdout).expect("stage receipt JSON");
    assert_eq!(receipt["recorded_only"], true);
    assert_eq!(receipt["receipt"]["archive_created"], false);
    let manifest = receipt["receipt"]["manifest"].clone();
    assert!(manifest.is_object());
    let expected = root.path().join("expected-manifest.json");
    // This deliberately has different formatting from canonical staged bytes.
    // --expected-manifest-sha256 binds complete stage MANIFEST.json, not this file.
    write_private(
        &expected,
        &serde_json::to_vec_pretty(&manifest).expect("expected manifest"),
    );
    let raw = fs::read(stage.join("MANIFEST.json")).expect("actual staged manifest bytes");
    assert_eq!(raw.last(), Some(&b'\n'));
    let raw_hash = digest(&raw);
    assert_ne!(
        digest(&fs::read(&expected).expect("expected input bytes")),
        raw_hash
    );
    Fixture {
        root,
        stage,
        expected,
        manifest,
        raw_hash,
    }
}
fn archive_args(f: &Fixture, output: &Path, raw_hash: &str) -> Vec<String> {
    vec![
        "release".into(),
        "archive".into(),
        "--staged-dir".into(),
        text(&f.stage),
        "--manifest".into(),
        text(&f.expected),
        "--expected-manifest-sha256".into(),
        raw_hash.to_owned(),
        "--output".into(),
        text(output),
    ]
}
#[test]
fn actual_archive_cli_creates_truthful_receipt_and_exact_extractable_bytes() {
    let f = fixture();
    let output = f.root.path().join("bundle.tar.gz");
    let result = invoke(f.root.path(), &archive_args(&f, &output, &f.raw_hash));
    assert!(
        result.status.success(),
        "actual archive failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let result: Value = serde_json::from_slice(&result.stdout).expect("archive receipt JSON");
    assert_eq!(result["recorded_only"], true);
    for flag in [
        "installed",
        "signed",
        "published",
        "execution_granted",
        "acceptance_granted",
        "native_execution_qualified",
        "activated",
        "source_commit_verified",
    ] {
        assert_eq!(result[flag], false, "no implied {flag}");
    }
    let receipt = &result["receipt"];
    assert_eq!(receipt["archive_created"], true);
    assert_eq!(receipt["installer_complete"], false);
    assert_eq!(receipt["manifest_raw_sha256"], f.raw_hash);
    assert_eq!(
        receipt["unsigned_manifest_sha256"],
        f.manifest["manifest_sha256"]
    );
    let bytes = fs::read(&output).expect("actual archive");
    assert_eq!(receipt["archive_sha256"], digest(&bytes));
    assert_eq!(receipt["archive_bytes"], bytes.len() as u64);
    let expected = f.manifest["entries"].as_array().expect("expected entries");
    let mut seen = Vec::new();
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(bytes.as_slice()));
    for entry in archive.entries().expect("tar entries") {
        let mut entry = entry.expect("tar entry");
        assert!(entry.header().entry_type().is_file());
        let path = entry
            .path()
            .expect("entry path")
            .to_str()
            .expect("ASCII")
            .to_owned();
        let mut actual = Vec::new();
        entry
            .read_to_end(&mut actual)
            .expect("small fixture contents");
        if path == "MANIFEST.json" {
            assert_eq!(
                actual,
                fs::read(f.stage.join("MANIFEST.json")).expect("original manifest")
            );
        } else {
            let e = expected
                .iter()
                .find(|e| e["archive_path"] == path)
                .expect("only manifest input");
            assert_eq!(digest(&actual), e["sha256"]);
            assert_eq!(actual.len() as u64, e["size"]);
        }
        seen.push(path);
    }
    assert_eq!(seen.len(), expected.len() + 1);
    assert_eq!(seen[0], "MANIFEST.json");
    let extracted = f.root.path().join("extracted");
    fs::create_dir(&extracted).expect("new owned extraction directory");
    fs::set_permissions(&extracted, fs::Permissions::from_mode(0o700)).expect("private extraction");
    tar::Archive::new(flate2::read::GzDecoder::new(bytes.as_slice()))
        .unpack(&extracted)
        .expect("owned roundtrip only");
    for e in expected {
        let path = extracted.join(e["archive_path"].as_str().expect("path"));
        assert_eq!(
            digest(&fs::read(&path).expect("extracted bytes")),
            e["sha256"]
        );
        assert_eq!(
            fs::metadata(path).expect("mode").mode() & 0o777,
            e["mode"].as_u64().expect("mode") as u32
        );
    }
    eprintln!(
        "native-archive-cli archive_sha256={} raw_manifest_sha256={}",
        receipt["archive_sha256"], receipt["manifest_raw_sha256"]
    );
}
#[test]
fn actual_archive_cli_preserves_existing_destination_and_reports_failure() {
    let f = fixture();
    let output = f.root.path().join("keep.gz");
    write_private(&output, b"KEEP ORIGINAL");
    let result = invoke(f.root.path(), &archive_args(&f, &output, &f.raw_hash));
    assert!(!result.status.success());
    assert!(result.stdout.is_empty());
    assert_eq!(
        fs::read(output).expect("preserved destination"),
        b"KEEP ORIGINAL"
    );
}
#[test]
fn actual_archive_cli_wrong_complete_hash_fails_before_creating_output() {
    let f = fixture();
    let output = f.root.path().join("wrong.gz");
    let result = invoke(f.root.path(), &archive_args(&f, &output, &"0".repeat(64)));
    assert!(!result.status.success());
    assert!(result.stdout.is_empty());
    assert!(!output.exists());
    assert!(
        !fs::read_dir(f.root.path())
            .expect("fixture entries")
            .any(|e| e
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .starts_with(".admiralty-native-archive-"))
    );
}
#[test]
fn actual_archive_cli_requires_private_expected_manifest_and_absolute_paths() {
    let f = fixture();
    let output = f.root.path().join("refused.gz");
    fs::set_permissions(&f.expected, fs::Permissions::from_mode(0o644))
        .expect("insecure expected fixture");
    assert!(
        !invoke(f.root.path(), &archive_args(&f, &output, &f.raw_hash))
            .status
            .success()
    );
    assert!(!output.exists());
    fs::set_permissions(&f.expected, fs::Permissions::from_mode(0o600)).expect("restore fixture");
    let link = f.root.path().join("manifest-link.json");
    symlink(&f.expected, &link).expect("fixture symlink");
    let mut args = archive_args(&f, &output, &f.raw_hash);
    args[5] = text(&link);
    assert!(!invoke(f.root.path(), &args).status.success());
    assert!(!output.exists());
    let mut args = archive_args(&f, &output, &f.raw_hash);
    args[3] = "stage".into();
    assert!(!invoke(f.root.path(), &args).status.success());
    assert!(!output.exists());
    fs::write(
        &f.expected,
        b"{\"schema_version\":\"PRIVATE_MANIFEST_CANARY\"}",
    )
    .expect("invalid private fixture");
    let invalid = invoke(f.root.path(), &archive_args(&f, &output, &f.raw_hash));
    assert!(!invalid.status.success());
    assert!(invalid.stdout.is_empty());
    assert!(!String::from_utf8_lossy(&invalid.stderr).contains("PRIVATE_MANIFEST_CANARY"));
    assert!(!output.exists());
}
