//! Actual Rust stage/archive/install CLI children over owned synthetic bytes.
//! No shell, Python, network, provider, service, publication or runtime grant.
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
    capture_command(
        root,
        Command::new(env!("CARGO_BIN_EXE_admiralty"))
            .args(args)
            .current_dir(root)
            .env_clear()
            .env("TMPDIR", root),
    )
}
fn capture_command(root: &Path, command: &mut Command) -> CliOutput {
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
    let mut child = command
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
    archive: PathBuf,
    destination: PathBuf,
    manifest: Value,
    archive_hash: String,
    raw_hash: String,
}
fn fixture() -> Fixture {
    let root = tempfile::Builder::new()
        .prefix("install-cli-")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in(std::env::temp_dir().canonicalize().expect("canonical tmp"))
        .expect("private fixture");
    let sources = [
        (
            "target/release/admiralty",
            b"never execute this fixture".as_slice(),
            0o755,
        ),
        (
            "apps/dashboard/dist/index.html",
            b"<main>private install</main>".as_slice(),
            0o644,
        ),
        (
            "docs/operators/release.md",
            b"owned fixture".as_slice(),
            0o644,
        ),
        (
            "schemas/domain-event-v2.schema.json",
            include_bytes!("../../../schemas/domain-event-v2.schema.json").as_slice(),
            0o644,
        ),
        (
            "schemas/guards/v2/fixture.mjs",
            b"throw new Error('MUST_NOT_EXECUTE'); export const validateDomainEventV2=()=>false;"
                .as_slice(),
            0o644,
        ),
    ];
    let mut inputs = vec![];
    for (source, bytes, mode) in sources {
        let path = root.path().join(source);
        fs::create_dir_all(path.parent().expect("source parent")).expect("source dirs");
        fs::write(&path, bytes).expect("synthetic input");
        inputs.push(json!({"source":source,"expected_sha256":digest(bytes),"mode":mode}));
    }
    let allowlist = root.path().join("allowlist.json");
    write_private(&allowlist, &serde_json::to_vec(&inputs).expect("allowlist"));
    let stage = root.path().join("stage");
    let result = invoke(
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
        result.status.success(),
        "stage: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let staged: Value = serde_json::from_slice(&result.stdout).expect("stage receipt");
    let manifest = staged["receipt"]["manifest"].clone();
    let expected = root.path().join("expected.json");
    write_private(&expected, &serde_json::to_vec(&manifest).expect("manifest"));
    let raw_hash = digest(&fs::read(stage.join("MANIFEST.json")).expect("canonical manifest"));
    let archive = root.path().join("native.tar.gz");
    let result = invoke(
        root.path(),
        &[
            "release".into(),
            "archive".into(),
            "--staged-dir".into(),
            text(&stage),
            "--manifest".into(),
            text(&expected),
            "--expected-manifest-sha256".into(),
            raw_hash.clone(),
            "--output".into(),
            text(&archive),
        ],
    );
    assert!(
        result.status.success(),
        "archive: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let archived: Value = serde_json::from_slice(&result.stdout).expect("archive receipt");
    let archive_hash = archived["receipt"]["archive_sha256"]
        .as_str()
        .expect("archive hash")
        .to_owned();
    assert_eq!(
        digest(&fs::read(&archive).expect("archive bytes")),
        archive_hash
    );
    Fixture {
        destination: root.path().join("admiralty-2.0.0"),
        root,
        archive,
        manifest,
        archive_hash,
        raw_hash,
    }
}
fn args(f: &Fixture) -> Vec<String> {
    vec![
        "release".into(),
        "install".into(),
        "--archive".into(),
        text(&f.archive),
        "--expected-archive-sha256".into(),
        f.archive_hash.clone(),
        "--expected-manifest-sha256".into(),
        f.raw_hash.clone(),
        "--destination".into(),
        text(&f.destination),
    ]
}
fn no_temporary(f: &Fixture) {
    assert!(
        !fs::read_dir(f.root.path())
            .expect("parent entries")
            .any(|e| e
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .starts_with(".admiralty-native-install-"))
    );
}
fn verify_success(f: &Fixture) {
    let result = invoke(f.root.path(), &args(f));
    assert!(
        result.status.success(),
        "install: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(result.stderr.is_empty());
    let result: Value = serde_json::from_slice(&result.stdout).expect("installer receipt");
    assert_eq!(result["recorded_only"], true);
    assert_eq!(result["installed_bytes"], true);
    let receipt = &result["receipt"];
    assert_eq!(receipt["installed_bytes"], true);
    assert_eq!(
        receipt["installation_scope"],
        "private_version_directory_only"
    );
    for flag in [
        "installer_complete",
        "daemon_running",
        "autostart",
        "profile_changed",
        "path_changed",
        "publisher_authenticated",
        "source_commit_verified",
        "signed",
        "published",
        "execution_granted",
        "acceptance_granted",
        "native_execution_qualified",
        "activated",
    ] {
        assert_eq!(receipt[flag], false, "no implied {flag}");
    }
    for flag in ["execution_granted", "acceptance_granted", "activated"] {
        assert_eq!(result[flag], false);
    }
    assert_eq!(receipt["archive_sha256"], f.archive_hash);
    assert_eq!(receipt["manifest_raw_sha256"], f.raw_hash);
    let actual = fs::metadata(&f.destination).expect("installed private version");
    assert_eq!(actual.mode() & 0o7777, 0o700);
    assert_eq!(receipt["installed_device"], actual.dev().to_string());
    assert_eq!(receipt["installed_inode"], actual.ino().to_string());
    for entry in f.manifest["entries"].as_array().expect("manifest entries") {
        let file = f
            .destination
            .join(entry["archive_path"].as_str().expect("entry path"));
        let bytes = fs::read(&file).expect("installed payload");
        assert_eq!(digest(&bytes), entry["sha256"]);
        assert_eq!(
            bytes.len() as u64,
            entry["size"].as_u64().expect("entry size")
        );
        assert_eq!(
            fs::metadata(file).expect("installed metadata").mode() & 0o7777,
            entry["mode"].as_u64().expect("entry mode") as u32
        );
    }
    assert_eq!(
        digest(&fs::read(f.destination.join("MANIFEST.json")).expect("installed manifest")),
        f.raw_hash
    );
    assert_eq!(
        digest(&fs::read(&f.archive).expect("source retained")),
        f.archive_hash
    );
    no_temporary(f);
}
#[test]
fn actual_rust_stage_archive_install_children_report_only_installed_bytes() {
    verify_success(&fixture());
}
#[test]
fn actual_installer_normalizes_owned_modes_in_single_umask077_child() {
    const CHILD: &str = "ADMIRALTY_INSTALL_UMASK_CHILD";
    const TEST: &str = "actual_installer_normalizes_owned_modes_in_single_umask077_child";
    if std::env::var_os(CHILD).as_deref() == Some(std::ffi::OsStr::new("1")) {
        rustix::process::umask(rustix::fs::Mode::from_bits_truncate(0o077));
        let f = fixture();
        let probe = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o644)
            .open(f.root.path().join("umask-probe"))
            .expect("child probe");
        assert_eq!(
            probe.metadata().expect("probe metadata").mode() & 0o777,
            0o600
        );
        verify_success(&f);
        return;
    }
    let root = tempfile::Builder::new()
        .prefix("install-mask-")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in(std::env::temp_dir().canonicalize().expect("canonical tmp"))
        .expect("private child capture");
    let result = capture_command(
        root.path(),
        Command::new(std::env::current_exe().expect("actual integration executable"))
            .args(["--exact", TEST, "--nocapture", "--test-threads=1"])
            .current_dir(root.path())
            .env_clear()
            .env(CHILD, "1")
            .env("TMPDIR", root.path()),
    );
    assert!(
        result.status.success(),
        "umask child: {}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(String::from_utf8_lossy(&result.stdout).contains("1 passed; 0 failed"));
}
#[test]
fn actual_installer_wrong_pins_and_relative_paths_fail_before_stage() {
    let f = fixture();
    for (i, value) in [
        (5, "0".repeat(64)),
        (7, "0".repeat(64)),
        (5, "A".repeat(64)),
        (3, "relative.tar.gz".into()),
        (9, "relative-version".into()),
    ] {
        let mut a = args(&f);
        a[i] = value;
        let result = invoke(f.root.path(), &a);
        assert!(!result.status.success());
        assert!(result.stdout.is_empty());
        assert!(!f.destination.exists());
        no_temporary(&f);
    }
}
#[test]
fn actual_installer_never_replaces_existing_file_directory_or_symlink() {
    for kind in 0..3 {
        let f = fixture();
        match kind {
            0 => write_private(&f.destination, b"KEEP"),
            1 => {
                fs::create_dir(&f.destination).expect("existing dir");
                write_private(&f.destination.join("evidence"), b"KEEP")
            }
            _ => symlink(&f.archive, &f.destination).expect("existing symlink"),
        };
        let before = fs::symlink_metadata(&f.destination).expect("existing metadata");
        let result = invoke(f.root.path(), &args(&f));
        assert!(!result.status.success());
        assert!(result.stdout.is_empty());
        let after = fs::symlink_metadata(&f.destination).expect("preserved");
        assert_eq!((before.dev(), before.ino()), (after.dev(), after.ino()));
        if kind == 0 {
            assert_eq!(fs::read(&f.destination).expect("file preserved"), b"KEEP")
        }
        if kind == 1 {
            assert_eq!(
                fs::read(f.destination.join("evidence")).expect("tree preserved"),
                b"KEEP"
            )
        }
        no_temporary(&f);
    }
}
#[test]
fn actual_installer_rejects_source_links_and_nonprivate_destination_parent() {
    for kind in 0..3 {
        let f = fixture();
        let mut a = args(&f);
        let link = f.root.path().join("untrusted.gz");
        match kind {
            0 => symlink(&f.archive, &link).expect("source symlink"),
            1 => fs::hard_link(&f.archive, &link).expect("source hardlink"),
            _ => fs::set_permissions(f.root.path(), fs::Permissions::from_mode(0o755))
                .expect("nonprivate parent"),
        };
        if kind < 2 {
            a[3] = text(&link);
        }
        let result = invoke(f.root.path(), &a);
        assert!(!result.status.success());
        assert!(result.stdout.is_empty());
        assert!(!f.destination.exists());
        no_temporary(&f);
    }
}
#[test]
fn actual_installer_correct_integrity_pin_does_not_accept_concatenated_archive() {
    let f = fixture();
    let mut bytes = fs::read(&f.archive).expect("archive");
    bytes.extend_from_within(..);
    fs::write(&f.archive, &bytes).expect("owned invalid archive");
    let mut a = args(&f);
    a[5] = digest(&bytes);
    let result = invoke(f.root.path(), &a);
    assert!(!result.status.success());
    assert!(result.stdout.is_empty());
    assert!(!f.destination.exists());
    no_temporary(&f);
}
