//! Release qualification and activation, in Rust.
//!
//! `qualify` replaces `scripts/release/qualify_native.py` with the same
//! evidence: stage explicit inputs, archive twice and require identical
//! bytes, install privately, run the installed `doctor` and synthetic demo
//! with an empty `PATH` and no inherited environment, and prove that an
//! existing destination and a wrong archive pin are both refused. Every
//! command's exit code and output digests are retained in the evidence
//! directory; nothing is cleared or reused.
//!
//! `activate`/`rollback` add the step the private installer deliberately
//! left out: an atomic `current` symlink inside an owner-controlled
//! releases root, with an append-only journal so the previous version can
//! be restored. They never start, stop or reconfigure services.
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

const COMMAND_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_CAPTURE_BYTES: u64 = 4 * 1024 * 1024;
pub const QUALIFICATION_VERSION: &str = "2.0.0-private-qualification";
pub const JOURNAL: &str = "activation.jsonl";
pub const CURRENT: &str = "current";

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(unix)]
fn create_private_dir(path: &Path) -> Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new().mode(0o700).create(path)?;
    Ok(())
}

#[cfg(not(unix))]
fn create_private_dir(path: &Path) -> Result<()> {
    fs::create_dir(path)?;
    Ok(())
}

fn write_new(path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(mode);
    }
    #[cfg(not(unix))]
    let _ = mode;
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn write_json(path: &Path, value: &Value) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    write_new(path, &bytes, 0o600)
}

/// No symlink in the path or any ancestor may become an input root.
fn regular_input(path: &Path) -> Result<PathBuf> {
    for ancestor in path.ancestors() {
        if fs::symlink_metadata(ancestor).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(format!("symlink input: {}", ancestor.display()).into());
        }
    }
    if !fs::symlink_metadata(path)?.is_file() {
        return Err(format!("nonregular input: {}", path.display()).into());
    }
    Ok(path.to_owned())
}

/// Evidence must land under ancestors no other user can rewrite.
#[cfg(unix)]
fn protected_ancestors(path: &Path) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    let uid = rustix::process::geteuid().as_raw();
    for ancestor in path.ancestors().skip(1) {
        if ancestor.as_os_str().is_empty() {
            continue;
        }
        let metadata = fs::symlink_metadata(ancestor)?;
        let sticky = metadata.mode() & 0o1000 != 0;
        if metadata.file_type().is_symlink()
            || !metadata.is_dir()
            || (metadata.uid() != 0 && metadata.uid() != uid)
            || (metadata.mode() & 0o022 != 0 && !sticky)
        {
            return Err(format!("unprotected output ancestor: {}", ancestor.display()).into());
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn protected_ancestors(_: &Path) -> Result<()> {
    Err("release qualification is supported on Unix only".into())
}

fn full_commit(text: &str) -> bool {
    matches!(text.len(), 40 | 64)
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

struct Invoker {
    output: PathBuf,
    path: PathBuf,
    scratch: PathBuf,
    commands: Vec<Value>,
}

impl Invoker {
    /// Run with no inherited environment, stdin closed, output captured to
    /// exclusive files and a hard deadline. Returns parsed stdout on success.
    fn run(
        &mut self,
        program: &Path,
        args: &[&str],
        label: &str,
        succeeds: bool,
    ) -> Result<Option<Value>> {
        let stdout_path = self.output.join(format!("{label}.stdout"));
        let stderr_path = self.output.join(format!("{label}.stderr"));
        let stdout = create_capture(&stdout_path)?;
        let stderr = create_capture(&stderr_path)?;
        let mut child = Command::new(program)
            .args(args)
            .current_dir(&self.output)
            .env_clear()
            .env("PATH", &self.path)
            .env("TMPDIR", &self.scratch)
            .env("LANG", "C")
            .stdin(Stdio::null())
            .stdout(stdout)
            .stderr(stderr)
            .spawn()?;
        let started = Instant::now();
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if started.elapsed() > COMMAND_TIMEOUT {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{label}: exceeded {COMMAND_TIMEOUT:?}").into());
            }
            std::thread::sleep(Duration::from_millis(25));
        };
        let code = status.code();
        self.commands.push(json!({"label":label,"exit_code":code,
            "stdout_sha256":sha256_file(&stdout_path)?,"stderr_sha256":sha256_file(&stderr_path)?}));
        if status.success() != succeeds {
            return Err(format!(
                "{label}: unexpected exit {code:?}; inspect retained captures in {}",
                self.output.display()
            )
            .into());
        }
        for path in [&stdout_path, &stderr_path] {
            if fs::metadata(path)?.len() > MAX_CAPTURE_BYTES {
                return Err(format!("{label}: capture exceeds 4 MiB").into());
            }
        }
        if !succeeds {
            return Ok(None);
        }
        Ok(Some(serde_json::from_slice(&fs::read(&stdout_path)?)?))
    }
}

fn create_capture(path: &Path) -> Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    Ok(options.open(path)?)
}

pub struct QualifyRequest<'a> {
    pub binary: &'a Path,
    pub assets: &'a Path,
    pub repository: &'a Path,
    pub output: &'a Path,
    pub source_commit: &'a str,
    pub target: &'a str,
}

/// Run the complete private archive/install/demo qualification.
pub fn qualify(request: &QualifyRequest<'_>) -> Result<Value> {
    let binary = regular_input(&std::path::absolute(request.binary)?)?;
    let assets = std::path::absolute(request.assets)?;
    regular_input(&assets.join("index.html"))?;
    let repository = fs::canonicalize(request.repository)?;
    if !full_commit(request.source_commit) {
        return Err("source commit must be a complete lowercase Git ID".into());
    }
    let resolved = Command::new("git")
        .arg("-C")
        .arg(&repository)
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{}^{{commit}}", request.source_commit),
        ])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()?;
    if !resolved.status.success()
        || String::from_utf8_lossy(&resolved.stdout).trim() != request.source_commit
    {
        return Err("source commit must identify an existing local commit".into());
    }
    let output = std::path::absolute(request.output)?;
    protected_ancestors(&output)?;
    create_private_dir(&output)?;
    let inputs = output.join("inputs");
    let scratch = output.join("scratch");
    let empty_path = output.join("empty-path");
    for directory in [&inputs, &scratch, &empty_path] {
        create_private_dir(directory)?;
    }
    let mut allowlist = Vec::<Value>::new();
    let mut add = |source: &Path, relative: &str, mode: u32| -> Result<()> {
        regular_input(source)?;
        let destination = inputs.join(relative);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        let bytes = fs::read(source)?;
        write_new(&destination, &bytes, mode)?;
        allowlist.push(
            json!({"source":relative,"expected_sha256":sha256_file(&destination)?,"mode":mode}),
        );
        Ok(())
    };
    add(&binary, "target/release/admiralty", 0o755)?;
    let mut assets_found = Vec::new();
    collect_files(&assets, &assets, &mut assets_found)?;
    assets_found.sort();
    for relative in &assets_found {
        add(
            &assets.join(relative),
            &format!("apps/dashboard/dist/{relative}"),
            0o644,
        )?;
    }
    add(
        &repository.join("schemas/domain-event-v2.schema.json"),
        "schemas/domain-event-v2.schema.json",
        0o644,
    )?;
    let mut guards = fs::read_dir(repository.join("packages/contracts/generated"))?
        .filter_map(|entry| {
            entry
                .ok()
                .map(|e| e.file_name().to_string_lossy().into_owned())
        })
        .filter(|name| name.starts_with("validators") && name.ends_with(".mjs"))
        .collect::<Vec<_>>();
    guards.sort();
    for name in &guards {
        add(
            &repository.join("packages/contracts/generated").join(name),
            &format!("packages/contracts/generated/{name}"),
            0o644,
        )?;
    }
    for name in [
        "native-install.md",
        "recovery.md",
        "upgrades.md",
        "release-status.md",
    ] {
        add(
            &repository.join("docs/operators").join(name),
            &format!("docs/operators/{name}"),
            0o644,
        )?;
    }
    for name in ["LICENSE", "CONTRIBUTING.md", "SECURITY.md"] {
        add(&repository.join(name), &format!("docs/{name}"), 0o644)?;
    }
    let allowlist_file = output.join("allowlist.json");
    write_json(&allowlist_file, &json!(allowlist))?;
    let mut invoker = Invoker {
        output: output.clone(),
        path: empty_path,
        scratch: scratch.clone(),
        commands: Vec::new(),
    };
    let text = |path: &Path| path.to_string_lossy().into_owned();
    let stage = output.join("stage");
    let staged = invoker
        .run(
            &binary,
            &[
                "release",
                "stage",
                "--input-root",
                &text(&inputs),
                "--output",
                &text(&stage),
                "--allowlist",
                &text(&allowlist_file),
                "--version",
                QUALIFICATION_VERSION,
                "--target",
                request.target,
                "--source-commit",
                request.source_commit,
            ],
            "stage",
            true,
        )?
        .ok_or("stage produced no receipt")?;
    let expected = output.join("expected-manifest.json");
    write_json(&expected, &staged["receipt"]["manifest"])?;
    let manifest_hash = sha256_file(&stage.join("MANIFEST.json"))?;
    let mut archive_hashes = Vec::new();
    for number in 0..2 {
        let archive = output.join(format!("native-{number}.tar.gz"));
        let archived = invoker
            .run(
                &binary,
                &[
                    "release",
                    "archive",
                    "--staged-dir",
                    &text(&stage),
                    "--manifest",
                    &text(&expected),
                    "--expected-manifest-sha256",
                    &manifest_hash,
                    "--output",
                    &text(&archive),
                ],
                &format!("archive-{number}"),
                true,
            )?
            .ok_or("archive produced no receipt")?;
        let actual = sha256_file(&archive)?;
        if archived["receipt"]["archive_sha256"].as_str() != Some(actual.as_str()) {
            return Err("archive receipt differs from actual bytes".into());
        }
        archive_hashes.push(actual);
    }
    if archive_hashes[0] != archive_hashes[1] {
        return Err("repeated archive build differs".into());
    }
    let destination = output.join("installed");
    let archive = text(&output.join("native-0.tar.gz"));
    let install_args = |pin: &str, destination: &Path| -> Vec<String> {
        vec![
            "release".to_owned(),
            "install".into(),
            "--archive".into(),
            archive.clone(),
            "--expected-archive-sha256".into(),
            pin.to_owned(),
            "--expected-manifest-sha256".into(),
            manifest_hash.clone(),
            "--destination".into(),
            text(destination),
        ]
    };
    fn refs(args: &[String]) -> Vec<&str> {
        args.iter().map(String::as_str).collect()
    }
    let args = install_args(&archive_hashes[0], &destination);
    let installed = invoker
        .run(&binary, &refs(&args), "install", true)?
        .ok_or("install produced no receipt")?;
    let executable = destination.join("bin/admiralty");
    let binary_sha = sha256_file(&binary)?;
    if sha256_file(&executable)? != binary_sha {
        return Err("installed executable differs from input binary".into());
    }
    let doctor = invoker
        .run(&executable, &["doctor"], "installed-doctor", true)?
        .ok_or("doctor produced no output")?;
    let dashboard = text(&destination.join("share/admiralty/dashboard"));
    let demo = invoker
        .run(
            &executable,
            &["demo", "--snapshot-only", "--assets", &dashboard],
            "installed-demo",
            true,
        )?
        .ok_or("demo produced no snapshot")?;
    if doctor["capabilities"]["isolated_offline_demo"] != true
        || doctor["native_execution_qualified"] != false
    {
        return Err("unexpected doctor capability boundary".into());
    }
    let tasks = demo["projection"]["tasks"]
        .as_object()
        .filter(|tasks| !tasks.is_empty())
        .ok_or("demo did not seed useful task state")?;
    if !tasks
        .values()
        .all(|task| task["contract"]["simulated"] == true)
    {
        return Err("demo contains a nonsimulated task".into());
    }
    invoker.run(&binary, &refs(&args), "existing-destination-refused", false)?;
    if sha256_file(&executable)? != binary_sha {
        return Err("refused reinstall changed installed binary".into());
    }
    let bad_destination = output.join("bad-install");
    let bad = install_args(&"0".repeat(64), &bad_destination);
    invoker.run(&binary, &refs(&bad), "wrong-archive-pin-refused", false)?;
    if bad_destination.exists() {
        return Err("bad archive pin published a destination".into());
    }
    if fs::read_dir(&scratch)?.next().is_some() {
        return Err("demo left temporary runtime state".into());
    }
    let qualifier = std::env::current_exe()
        .ok()
        .and_then(|path| sha256_file(&path).ok());
    let report = json!({
        "schema_version": 1, "status": "passed", "scope": "private_current_host_archive_install_demo",
        "platform": format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
        "source_commit_label": request.source_commit, "target_label": request.target,
        "qualifier": "admiralty release qualify", "qualifier_binary_sha256": qualifier,
        "allowlist_sha256": sha256_file(&allowlist_file)?,
        "binary_sha256": binary_sha, "manifest_raw_sha256": manifest_hash,
        "archive_sha256": archive_hashes[0], "reproducible_archive": true,
        "installed_file_count": installed["receipt"]["installed_file_count"],
        "demo_task_count": tasks.len(), "commands": invoker.commands,
        "account_required": false, "tools_on_path": false, "node_server_required": false,
        "source_commit_verified": false, "signed": false, "published": false,
        "service_activation_qualified": false, "full_packet_acceptance": false,
    });
    write_json(&output.join("qualification.json"), &report)?;
    Ok(report)
}

fn collect_files(root: &Path, directory: &Path, found: &mut Vec<String>) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let path = entry.path();
        if kind.is_symlink() {
            return Err(format!("symlink asset: {}", path.display()).into());
        }
        if kind.is_dir() {
            collect_files(root, &path, found)?;
        } else if kind.is_file() {
            let relative = path
                .strip_prefix(root)?
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            found.push(relative);
        }
        if found.len() > 1024 {
            return Err("dashboard asset count exceeds 1024".into());
        }
    }
    Ok(())
}

/// Receipt for an activation or rollback.
#[derive(Debug, serde::Serialize)]
pub struct Activation {
    pub action: &'static str,
    pub releases_root: PathBuf,
    pub previous: Option<String>,
    pub current: String,
    pub manifest_sha256: String,
    pub journal: PathBuf,
    pub services_restarted: bool,
}

fn valid_version_name(name: &str) -> bool {
    (1..=128).contains(&name.len())
        && name != CURRENT
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

#[cfg(unix)]
fn check_root(root: &Path) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    if !root.is_absolute() {
        return Err("releases root must be absolute".into());
    }
    let metadata = fs::symlink_metadata(root)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o022 != 0
    {
        return Err("releases root must be an owned directory without group/other write".into());
    }
    Ok(())
}

#[cfg(not(unix))]
fn check_root(_: &Path) -> Result<()> {
    Err("release activation is supported on Unix only".into())
}

/// The installed version must be a real directory directly under the root
/// with a regular MANIFEST.json and at least one Admiralty binary.
fn check_version(root: &Path, name: &str) -> Result<String> {
    if !valid_version_name(name) {
        return Err("version must be a plain directory name under the releases root".into());
    }
    let directory = root.join(name);
    let metadata = fs::symlink_metadata(&directory)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err("version must be a real directory, not a symlink".into());
    }
    let manifest = directory.join("MANIFEST.json");
    if !fs::symlink_metadata(&manifest)?.is_file() {
        return Err(
            "version has no regular MANIFEST.json; install it with `admiralty release install`"
                .into(),
        );
    }
    let has_binary = ["admiraltyd", "admiralty", "admiralty-observe"]
        .iter()
        .any(|bin| {
            fs::symlink_metadata(directory.join("bin").join(bin)).is_ok_and(|m| m.is_file())
        });
    if !has_binary {
        return Err("version contains no Admiralty binary under bin/".into());
    }
    sha256_file(&manifest)
}

fn current_target(root: &Path) -> Result<Option<String>> {
    let link = root.join(CURRENT);
    match fs::symlink_metadata(&link) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
        Ok(metadata) if !metadata.file_type().is_symlink() => {
            Err("current exists and is not a symlink; refusing to replace it".into())
        }
        Ok(_) => {
            let target = fs::read_link(&link)?;
            let name = target.to_string_lossy().into_owned();
            Ok(Some(name))
        }
    }
}

#[cfg(unix)]
fn swap_current(root: &Path, name: &str) -> Result<()> {
    let temporary = root.join(format!(".current-{}", uuid::Uuid::new_v4().simple()));
    std::os::unix::fs::symlink(name, &temporary)?;
    // rename(2) atomically replaces the old link: readers see old or new.
    if let Err(error) = fs::rename(&temporary, root.join(CURRENT)) {
        let _ = fs::remove_file(&temporary);
        return Err(error.into());
    }
    fs::File::open(root)?.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn swap_current(_: &Path, _: &str) -> Result<()> {
    Err("release activation is supported on Unix only".into())
}

fn journal(root: &Path, entry: &Value) -> Result<PathBuf> {
    let path = root.join(JOURNAL);
    let mut options = fs::OpenOptions::new();
    options.append(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&path)?;
    let mut line = serde_json::to_vec(entry)?;
    line.push(b'\n');
    file.write_all(&line)?;
    file.sync_all()?;
    Ok(path)
}

fn record(
    root: &Path,
    action: &'static str,
    previous: Option<String>,
    name: &str,
    manifest: String,
) -> Result<Activation> {
    let path = journal(
        root,
        &json!({"schema_version":1,"at":chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            "action":action,"from":previous,"to":name,"manifest_sha256":manifest}),
    )?;
    Ok(Activation {
        action,
        releases_root: root.to_owned(),
        previous,
        current: name.to_owned(),
        manifest_sha256: manifest,
        journal: path,
        services_restarted: false,
    })
}

/// Point `<root>/current` at `<root>/<version>` atomically.
pub fn activate(root: &Path, version: &str) -> Result<Activation> {
    check_root(root)?;
    let manifest = check_version(root, version)?;
    let previous = current_target(root)?;
    if previous.as_deref() == Some(version) {
        return Err(format!("{version} is already current").into());
    }
    swap_current(root, version)?;
    record(root, "activate", previous, version, manifest)
}

/// Undo the latest switch: restore the version that was current before the
/// activation (or rollback) that produced the current link.
pub fn rollback(root: &Path) -> Result<Activation> {
    check_root(root)?;
    let current = current_target(root)?.ok_or("nothing is activated")?;
    let text = fs::read_to_string(root.join(JOURNAL)).map_err(|_| "no activation journal")?;
    let previous = text
        .lines()
        .rev()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|entry| entry["to"] == current.as_str())
        .and_then(|entry| entry["from"].as_str().map(str::to_owned))
        .ok_or("no earlier version recorded for the current activation")?;
    let manifest = check_version(root, &previous)?;
    swap_current(root, &previous)?;
    record(root, "rollback", Some(current), &previous, manifest)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn bundle(root: &Path, name: &str, manifest: &str) {
        let bin = root.join(name).join("bin");
        fs::create_dir_all(&bin).expect("bin");
        fs::write(root.join(name).join("MANIFEST.json"), manifest).expect("manifest");
        fs::write(bin.join("admiraltyd"), b"binary").expect("binary");
    }

    fn private_root() -> tempfile::TempDir {
        let root = tempfile::tempdir().expect("tempdir");
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).expect("chmod");
        root
    }

    #[test]
    fn activation_is_atomic_journaled_and_reversible() {
        let root = private_root();
        bundle(root.path(), "0.1.0", "{\"v\":1}\n");
        bundle(root.path(), "0.2.0", "{\"v\":2}\n");
        let first = activate(root.path(), "0.1.0").expect("first");
        assert_eq!(first.previous, None);
        assert_eq!(
            fs::read_link(root.path().join(CURRENT)).expect("link"),
            Path::new("0.1.0")
        );
        let second = activate(root.path(), "0.2.0").expect("second");
        assert_eq!(second.previous.as_deref(), Some("0.1.0"));
        assert!(!second.services_restarted);
        assert!(activate(root.path(), "0.2.0").is_err(), "already current");
        let back = rollback(root.path()).expect("rollback");
        assert_eq!(
            (back.previous.as_deref(), back.current.as_str()),
            (Some("0.2.0"), "0.1.0")
        );
        assert_eq!(
            fs::read_link(root.path().join(CURRENT)).expect("link"),
            Path::new("0.1.0")
        );
        let journal = fs::read_to_string(root.path().join(JOURNAL)).expect("journal");
        assert_eq!(journal.lines().count(), 3);
        assert!(
            journal
                .lines()
                .last()
                .expect("line")
                .contains("\"action\":\"rollback\"")
        );
        assert_eq!(
            fs::metadata(root.path().join(JOURNAL))
                .expect("meta")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        // Rollback undoes the latest switch, so a second rollback re-applies it.
        let redo = rollback(root.path()).expect("undo the rollback");
        assert_eq!(redo.current, "0.2.0");
        // A first activation has no predecessor to restore.
        let fresh = private_root();
        bundle(fresh.path(), "only", "{}\n");
        activate(fresh.path(), "only").expect("only");
        assert!(rollback(fresh.path()).is_err());
        assert!(
            fs::read_dir(root.path()).expect("list").all(|e| !e
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .starts_with(".current-")),
            "no temporary links left behind"
        );
    }

    #[test]
    fn activation_refuses_unsafe_roots_versions_and_real_current_paths() {
        let root = private_root();
        bundle(root.path(), "good", "{}\n");
        for name in ["", "../escape", "a/b", ".hidden", "current", "missing"] {
            assert!(activate(root.path(), name).is_err(), "{name:?}");
        }
        fs::create_dir(root.path().join("empty")).expect("dir");
        assert!(activate(root.path(), "empty").is_err(), "no manifest");
        std::os::unix::fs::symlink(root.path().join("good"), root.path().join("alias"))
            .expect("link");
        assert!(activate(root.path(), "alias").is_err(), "symlinked version");
        fs::create_dir(root.path().join(CURRENT)).expect("real current");
        assert!(
            activate(root.path(), "good").is_err(),
            "never replaces a real directory"
        );
        fs::remove_dir(root.path().join(CURRENT)).expect("cleanup");
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o777)).expect("chmod");
        assert!(
            activate(root.path(), "good").is_err(),
            "world-writable root"
        );
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).expect("chmod");
        assert!(activate(Path::new("relative"), "good").is_err());
        activate(root.path(), "good").expect("valid activation still works");
    }

    #[test]
    fn qualification_inputs_reject_symlinks_and_short_commits() {
        let root = private_root();
        let file = root.path().join("file");
        fs::write(&file, b"x").expect("file");
        regular_input(&file).expect("regular");
        std::os::unix::fs::symlink(&file, root.path().join("link")).expect("link");
        assert!(regular_input(&root.path().join("link")).is_err());
        assert!(regular_input(root.path()).is_err());
        assert!(full_commit(&"a".repeat(40)));
        assert!(full_commit(&"0".repeat(64)));
        assert!(!full_commit(&"A".repeat(40)));
        assert!(!full_commit("abc123"));
    }
}
