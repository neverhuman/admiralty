//! Qualify a private native archive with real CLI and dashboard payloads
//! offline (port of the retired `scripts/release/qualify_native.py`).
//!
//! Drives the supplied `admiralty` executable through `release stage`, two
//! byte-identical `release archive` runs, `release install`, the installed
//! `doctor` and `demo --snapshot-only`, and two refused installs, with an
//! empty `PATH` and no inherited account or provider configuration. Every
//! input, capture and report is retained in the new output directory.

use std::fs::{self, File};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::process::{returncode, wait_timeout};
use crate::py::{self, Dump, PyDict, PyValue};
use crate::py_dict;

const COMMAND_TIMEOUT: Duration = Duration::from_secs(60);
const GIT_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_CAPTURE: u64 = 4 * 1024 * 1024;

/// `qualify_native.py` arguments.
#[derive(Debug, Clone, clap::Args)]
pub struct QualifyArgs {
    /// The `admiralty` executable to stage, archive, install and run.
    #[arg(long)]
    pub binary: PathBuf,
    /// Built dashboard directory (must contain `index.html`).
    #[arg(long)]
    pub assets: PathBuf,
    /// New evidence directory beneath protected ancestors.
    #[arg(long)]
    pub output: PathBuf,
    /// Full lowercase 40- or 64-character commit that exists locally.
    #[arg(long = "source-commit")]
    pub source_commit: String,
    /// Target label.
    #[arg(long)]
    pub target: String,
    /// Source checkout providing schemas, validators, docs and the commit
    /// (default: the checkout this harness was built from).
    #[arg(long = "repo-root")]
    pub repo_root: Option<PathBuf>,
}

type Outcome<T> = Result<T, String>;

fn io_error(context: impl std::fmt::Display) -> impl Fn(io::Error) -> String {
    move |error| format!("{context}: {error}")
}

/// The checkout this crate was built from (`Path(__file__).parents[2]`).
pub fn default_repo_root() -> PathBuf {
    let raw = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    crate::source_package::abspath(&raw).unwrap_or(raw)
}

fn absolute(path: &Path) -> Outcome<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()
            .map_err(io_error("current directory"))?
            .join(path))
    }
}

fn is_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink())
}

/// `regular(path)`: no symlink in the path or any ancestor, and a regular file.
fn regular(path: &Path) -> Outcome<PathBuf> {
    for ancestor in path.ancestors() {
        if is_symlink(ancestor) {
            return Err(format!("ValueError: symlink input: {}", ancestor.display()));
        }
    }
    let metadata = fs::metadata(path).map_err(io_error(path.display()))?;
    if !metadata.file_type().is_file() {
        return Err(format!("ValueError: nonregular input: {}", path.display()));
    }
    Ok(path.to_path_buf())
}

fn sha256(path: &Path) -> Outcome<String> {
    let mut file = File::open(path).map_err(io_error(path.display()))?;
    let mut digest = Sha256::new();
    io::copy(&mut file, &mut digest).map_err(io_error(path.display()))?;
    Ok(format!("{:x}", digest.finalize()))
}

/// `write_json(path, value)`: exclusive create, `0600`, sorted indent-2 JSON.
fn write_json(path: &Path, value: &PyValue) -> Outcome<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(io_error(path.display()))?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(io_error(path.display()))?;
    let mut text = py::dumps(value, Dump::INDENT2_SORTED);
    text.push('\n');
    file.write_all(text.as_bytes())
        .map_err(io_error(path.display()))
}

fn mkdir_private(path: &Path) -> Outcome<()> {
    fs::DirBuilder::new()
        .mode(0o700)
        .create(path)
        .map_err(io_error(path.display()))
}

/// `Path.mkdir(mode=0o700, parents=True, exist_ok=True)`: missing ancestors
/// get default permissions and only the leaf gets `mode`.
fn mkdir_parents(path: &Path) -> Outcome<()> {
    if path.is_dir() {
        return Ok(());
    }
    if let Some(parent) = path.parent()
        && !parent.exists()
    {
        mkdir_parents_default(parent)?;
    }
    match fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists && path.is_dir() => Ok(()),
        Err(error) => Err(format!("{}: {error}", path.display())),
    }
}

fn mkdir_parents_default(path: &Path) -> Outcome<()> {
    fs::create_dir_all(path).map_err(io_error(path.display()))
}

fn run_with_timeout(command: &mut Command, timeout: Duration, label: &str) -> Outcome<i32> {
    let mut child = command.spawn().map_err(io_error(label))?;
    match wait_timeout(&mut child, timeout).map_err(io_error(label))? {
        Some(status) => Ok(returncode(status)),
        None => {
            let _ = child.kill();
            let _ = child.wait();
            Err(format!(
                "TimeoutExpired: Command {label} timed out after {} seconds",
                timeout.as_secs()
            ))
        }
    }
}

fn git_resolve(repo: &Path, commit: &str) -> Outcome<String> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "--verify", &format!("{commit}^{{commit}}")])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(io_error("git"))?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let reader = std::thread::spawn(move || {
        let mut out = Vec::new();
        if let Some(mut stdout) = stdout {
            let _ = io::Read::read_to_end(&mut stdout, &mut out);
        }
        out
    });
    let error_reader = std::thread::spawn(move || {
        let mut err = Vec::new();
        if let Some(mut stderr) = stderr {
            let _ = io::Read::read_to_end(&mut stderr, &mut err);
        }
        err
    });
    let status = match wait_timeout(&mut child, GIT_TIMEOUT).map_err(io_error("git"))? {
        Some(status) => status,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            return Err("TimeoutExpired: git rev-parse timed out after 10 seconds".into());
        }
    };
    let out = reader.join().unwrap_or_default();
    let err = error_reader.join().unwrap_or_default();
    if !status.success() {
        return Err(format!(
            "CalledProcessError: git rev-parse returned non-zero exit status {}: {}",
            returncode(status),
            String::from_utf8_lossy(&err).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out).trim().to_owned())
}

/// Python `platform.platform()` on Linux (`Linux-<release>-<machine>-with-glibc<ver>`).
pub fn platform_string() -> String {
    let read = |path: &str| {
        fs::read_to_string(path)
            .map(|text| text.trim().to_owned())
            .unwrap_or_default()
    };
    let system = read("/proc/sys/kernel/ostype");
    let release = read("/proc/sys/kernel/osrelease");
    let machine = std::env::consts::ARCH.to_owned();
    let libc = Command::new("getconf")
        .arg("GNU_LIBC_VERSION")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| {
            String::from_utf8_lossy(&output.stdout)
                .trim()
                .replacen(' ', "", 1)
        })
        .unwrap_or_default();
    let parts: Vec<&str> = [
        system.as_str(),
        release.as_str(),
        machine.as_str(),
        "with",
        libc.as_str(),
    ]
    .into_iter()
    .filter(|part| !part.is_empty())
    .collect();
    let parts = if libc.is_empty() {
        parts.into_iter().filter(|part| *part != "with").collect()
    } else {
        parts
    };
    let mut text = parts
        .iter()
        .map(|part| part.trim())
        .collect::<Vec<_>>()
        .join("-");
    text = text.replace(' ', "_");
    for symbol in ['/', '\\', ':', ';', '"', '(', ')'] {
        text = text.replace(symbol, "-");
    }
    text = text.replace("unknown", "");
    while text.contains("--") {
        text = text.replace("--", "-");
    }
    while text.ends_with('-') {
        text.pop();
    }
    text
}

struct Qualification {
    output: PathBuf,
    environment: Vec<(String, String)>,
    commands: Vec<PyValue>,
    inputs: PathBuf,
    allowlist: Vec<PyValue>,
}

impl Qualification {
    fn invoke(
        &mut self,
        program: &Path,
        args: &[String],
        label: &str,
        succeeds: bool,
    ) -> Outcome<Option<PyValue>> {
        let stdout_path = self.output.join(format!("{label}.stdout"));
        let stderr_path = self.output.join(format!("{label}.stderr"));
        let create = |path: &Path| {
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
                .map_err(io_error(path.display()))
        };
        let stdout = create(&stdout_path)?;
        let stderr = create(&stderr_path)?;
        let exit_code = run_with_timeout(
            Command::new(program)
                .args(args)
                .current_dir(&self.output)
                .env_clear()
                .envs(
                    self.environment
                        .iter()
                        .map(|(k, v)| (k.as_str(), v.as_str())),
                )
                .stdin(Stdio::null())
                .stdout(Stdio::from(stdout))
                .stderr(Stdio::from(stderr)),
            COMMAND_TIMEOUT,
            label,
        )?;
        self.commands.push(py_dict! {
            "label" => label,
            "exit_code" => exit_code,
            "stdout_sha256" => sha256(&stdout_path)?,
            "stderr_sha256" => sha256(&stderr_path)?,
        });
        if (exit_code == 0) != succeeds {
            return Err(format!(
                "ValueError: {label}: unexpected exit {exit_code}; inspect retained captures"
            ));
        }
        let size = |path: &Path| fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        if size(&stdout_path) > MAX_CAPTURE || size(&stderr_path) > MAX_CAPTURE {
            return Err(format!("ValueError: {label}: capture exceeds 4 MiB"));
        }
        if !succeeds {
            return Ok(None);
        }
        let text = fs::read(&stdout_path).map_err(io_error(stdout_path.display()))?;
        let text =
            String::from_utf8(text).map_err(|error| format!("UnicodeDecodeError: {error}"))?;
        py::loads_str(&text, false)
            .map(Some)
            .map_err(|error| format!("JSONDecodeError: {label}: {error}"))
    }

    fn add(&mut self, source: &Path, relative: &str, mode: u32) -> Outcome<()> {
        regular(source)?;
        let destination = self.inputs.join(relative);
        if let Some(parent) = destination.parent() {
            mkdir_parents(parent)?;
        }
        let mut input = File::open(source).map_err(io_error(source.display()))?;
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&destination)
            .map_err(io_error(destination.display()))?;
        io::copy(&mut input, &mut output).map_err(io_error(destination.display()))?;
        drop(output);
        fs::set_permissions(&destination, fs::Permissions::from_mode(mode))
            .map_err(io_error(destination.display()))?;
        self.allowlist.push(py_dict! {
            "source" => relative,
            "expected_sha256" => sha256(&destination)?,
            "mode" => mode,
        });
        Ok(())
    }
}

fn arg(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn get<'a>(value: &'a PyValue, key: &str) -> Outcome<&'a PyValue> {
    value.index(key)
}

/// `assets.rglob("*")` (no descent into symlinked directories), sorted.
fn walk_assets(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                pending.push(path.clone());
            }
            found.push(path);
        }
    }
    found.sort();
    found
}

fn glob_validators(directory: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = fs::read_dir(directory)
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| {
                    let name = entry.file_name();
                    let name = name.to_string_lossy();
                    name.starts_with("validators") && name.ends_with(".mjs") && name.len() >= 14
                })
                .map(|entry| entry.path())
                .collect()
        })
        .unwrap_or_default();
    found.sort();
    found
}

fn check_output_ancestors(output: &Path) -> Outcome<()> {
    let euid = rustix::process::geteuid().as_raw();
    let Some(parent) = output.parent() else {
        return Ok(());
    };
    for ancestor in parent.ancestors() {
        if is_symlink(ancestor) {
            return Err(format!(
                "ValueError: symlink output ancestor: {}",
                ancestor.display()
            ));
        }
        let metadata = fs::metadata(ancestor).map_err(io_error(ancestor.display()))?;
        let mode = metadata.mode();
        if !metadata.is_dir()
            || !(metadata.uid() == 0 || metadata.uid() == euid)
            || (mode & 0o022 != 0 && mode & 0o1000 == 0)
        {
            return Err(format!(
                "ValueError: unprotected output ancestor: {}",
                ancestor.display()
            ));
        }
    }
    Ok(())
}

/// Run the qualification and return the report (also written to
/// `<output>/qualification.json`).
pub fn qualify(args: &QualifyArgs) -> Outcome<PyValue> {
    let repo = args.repo_root.clone().unwrap_or_else(default_repo_root);
    let binary = regular(&absolute(&args.binary)?)?;
    let assets = absolute(&args.assets)?;
    regular(&assets.join("index.html"))?;
    let commit = args.source_commit.as_str();
    let hex = commit
        .bytes()
        .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    if !(hex && (commit.len() == 40 || commit.len() == 64)) {
        return Err("ValueError: source commit must be a complete lowercase Git ID".into());
    }
    if git_resolve(&repo, commit)? != commit {
        return Err("ValueError: source commit must identify an existing local commit".into());
    }
    // Keep this evidence directory. Never clear or reuse a previous result.
    let output = absolute(&args.output)?;
    check_output_ancestors(&output)?;
    mkdir_private(&output)?;
    let inputs = output.join("inputs");
    mkdir_private(&inputs)?;
    let scratch = output.join("scratch");
    mkdir_private(&scratch)?;
    let empty_path = output.join("empty-path");
    mkdir_private(&empty_path)?;
    // No account credentials, provider settings or tool executables are inherited.
    let mut run = Qualification {
        output: output.clone(),
        environment: vec![
            ("PATH".into(), arg(&empty_path)),
            ("TMPDIR".into(), arg(&scratch)),
            ("LANG".into(), "C".into()),
        ],
        commands: Vec::new(),
        inputs,
        allowlist: Vec::new(),
    };

    run.add(&binary, "target/release/admiralty", 0o755)?;
    for asset in walk_assets(&assets) {
        if is_symlink(&asset) {
            return Err(format!("ValueError: symlink asset: {}", asset.display()));
        }
        if asset.is_file() {
            let relative = asset
                .strip_prefix(&assets)
                .map_err(|error| error.to_string())?
                .to_string_lossy()
                .into_owned();
            run.add(&asset, &format!("apps/dashboard/dist/{relative}"), 0o644)?;
        }
    }
    run.add(
        &repo.join("schemas/domain-event-v2.schema.json"),
        "schemas/domain-event-v2.schema.json",
        0o644,
    )?;
    for guard in glob_validators(&repo.join("packages/contracts/generated")) {
        let name = guard
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        run.add(
            &guard,
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
        run.add(
            &repo.join("docs/operators").join(name),
            &format!("docs/operators/{name}"),
            0o644,
        )?;
    }
    for name in ["LICENSE", "CONTRIBUTING.md", "SECURITY.md"] {
        run.add(&repo.join(name), &format!("docs/{name}"), 0o644)?;
    }
    let allowlist_file = output.join("allowlist.json");
    write_json(&allowlist_file, &PyValue::List(run.allowlist.clone()))?;
    let stage = output.join("stage");
    let stage_args: Vec<String> = vec![
        "release".into(),
        "stage".into(),
        "--input-root".into(),
        arg(&run.inputs),
        "--output".into(),
        arg(&stage),
        "--allowlist".into(),
        arg(&allowlist_file),
        "--version".into(),
        "2.0.0-private-qualification".into(),
        "--target".into(),
        args.target.clone(),
        "--source-commit".into(),
        commit.to_owned(),
    ];
    let staged = run
        .invoke(&binary, &stage_args, "stage", true)?
        .unwrap_or(PyValue::Null);
    let expected = output.join("expected-manifest.json");
    write_json(&expected, get(get(&staged, "receipt")?, "manifest")?)?;
    let manifest_hash = sha256(&stage.join("MANIFEST.json"))?;
    let mut archive_hashes = Vec::new();
    for number in 0..2 {
        let archive = output.join(format!("native-{number}.tar.gz"));
        let archive_args: Vec<String> = vec![
            "release".into(),
            "archive".into(),
            "--staged-dir".into(),
            arg(&stage),
            "--manifest".into(),
            arg(&expected),
            "--expected-manifest-sha256".into(),
            manifest_hash.clone(),
            "--output".into(),
            arg(&archive),
        ];
        let archived = run
            .invoke(&binary, &archive_args, &format!("archive-{number}"), true)?
            .unwrap_or(PyValue::Null);
        let actual_hash = sha256(&archive)?;
        let receipt_hash = get(get(&archived, "receipt")?, "archive_sha256")?;
        if !py::py_eq(&PyValue::from(actual_hash.as_str()), receipt_hash) {
            return Err("ValueError: archive receipt differs from actual bytes".into());
        }
        archive_hashes.push(actual_hash);
    }
    if archive_hashes[0] != archive_hashes[1] {
        return Err("ValueError: repeated archive build differs".into());
    }
    let destination = output.join("installed");
    let install_args: Vec<String> = vec![
        "release".into(),
        "install".into(),
        "--archive".into(),
        arg(&output.join("native-0.tar.gz")),
        "--expected-archive-sha256".into(),
        archive_hashes[0].clone(),
        "--expected-manifest-sha256".into(),
        manifest_hash.clone(),
        "--destination".into(),
        arg(&destination),
    ];
    let installed = run
        .invoke(&binary, &install_args, "install", true)?
        .unwrap_or(PyValue::Null);
    let executable = destination.join("bin/admiralty");
    if sha256(&executable)? != sha256(&binary)? {
        return Err("ValueError: installed executable differs from input binary".into());
    }
    // Exercise the installed executable with no tools on PATH and no account.
    let doctor = run
        .invoke(
            &executable,
            &["doctor".to_owned()],
            "installed-doctor",
            true,
        )?
        .unwrap_or(PyValue::Null);
    let demo_args = vec![
        "demo".to_owned(),
        "--snapshot-only".to_owned(),
        "--assets".to_owned(),
        arg(&destination.join("share/admiralty/dashboard")),
    ];
    let demo = run
        .invoke(&executable, &demo_args, "installed-demo", true)?
        .unwrap_or(PyValue::Null);
    if !get(get(&doctor, "capabilities")?, "isolated_offline_demo")?.truthy()
        || get(&doctor, "native_execution_qualified")?.truthy()
    {
        return Err("ValueError: unexpected doctor capability boundary".into());
    }
    let tasks = get(get(&demo, "projection")?, "tasks")?;
    if !tasks.truthy() {
        return Err("ValueError: demo did not seed useful task state".into());
    }
    let PyValue::Dict(task_map) = tasks else {
        return Err(format!(
            "AttributeError: '{}' object has no attribute 'values'",
            tasks.type_name()
        ));
    };
    for task in task_map.values() {
        if !get(get(task, "contract")?, "simulated")?.truthy() {
            return Err("ValueError: demo contains a nonsimulated task".into());
        }
    }
    run.invoke(
        &binary,
        &install_args,
        "existing-destination-refused",
        false,
    )?;
    if sha256(&executable)? != sha256(&binary)? {
        return Err("ValueError: refused reinstall changed installed binary".into());
    }
    let mut bad_args = install_args.clone();
    if let Some(position) = bad_args
        .iter()
        .position(|value| value == "--expected-archive-sha256")
    {
        bad_args[position + 1] = "0".repeat(64);
    }
    let bad_install = output.join("bad-install");
    if let Some(last) = bad_args.last_mut() {
        *last = arg(&bad_install);
    }
    run.invoke(&binary, &bad_args, "wrong-archive-pin-refused", false)?;
    if bad_install.exists() {
        return Err("ValueError: bad archive pin published a destination".into());
    }
    if fs::read_dir(&scratch)
        .map_err(io_error(scratch.display()))?
        .next()
        .is_some()
    {
        return Err("ValueError: demo left temporary runtime state".into());
    }
    let script = std::env::current_exe().map_err(io_error("current executable"))?;
    let installed_count = get(get(&installed, "receipt")?, "installed_file_count")?.clone();
    let mut report = PyDict::new();
    report.insert("schema_version", 1);
    report.insert("status", "passed");
    report.insert("scope", "private_current_host_archive_install_demo");
    report.insert("platform", platform_string());
    report.insert("source_commit_label", commit);
    report.insert("target_label", args.target.as_str());
    report.insert("qualification_script_sha256", sha256(&script)?);
    report.insert("allowlist_sha256", sha256(&allowlist_file)?);
    report.insert("binary_sha256", sha256(&binary)?);
    report.insert("manifest_raw_sha256", manifest_hash);
    report.insert("archive_sha256", archive_hashes[0].as_str());
    report.insert("reproducible_archive", true);
    report.insert("installed_file_count", installed_count);
    report.insert("demo_task_count", task_map.len());
    report.insert("commands", run.commands.clone());
    report.insert("account_required", false);
    report.insert("tools_on_path", false);
    report.insert("node_server_required", false);
    report.insert("source_commit_verified", false);
    report.insert("signed", false);
    report.insert("published", false);
    report.insert("service_activation_qualified", false);
    report.insert("full_packet_acceptance", false);
    let report = PyValue::Dict(report);
    write_json(&output.join("qualification.json"), &report)?;
    Ok(report)
}

/// Run the qualification: print the report (indent 2, exit 0) or the
/// failure on stderr (exit 1).
pub fn run_qualify(args: QualifyArgs) -> crate::RunResult {
    match qualify(&args) {
        Ok(report) => {
            println!("{}", py::dumps(&report, Dump::INDENT2));
            Ok(0)
        }
        Err(error) => {
            eprintln!("native qualification failed: {error}");
            Ok(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_string_matches_python_layout() {
        let text = platform_string();
        assert!(text.starts_with("Linux-") || text.is_empty() || !cfg!(target_os = "linux"));
        assert!(!text.contains("--") && !text.ends_with('-'));
    }

    #[test]
    fn regular_refuses_symlink_ancestors_and_directories() {
        let temporary = tempfile::tempdir().expect("tempdir");
        let root = temporary.path().canonicalize().expect("resolve");
        fs::create_dir(root.join("real")).expect("mkdir");
        fs::write(root.join("real/file"), b"x").expect("write");
        std::os::unix::fs::symlink(root.join("real"), root.join("alias")).expect("symlink");
        assert!(regular(&root.join("real/file")).is_ok());
        assert!(regular(&root.join("alias/file")).is_err());
        assert!(regular(&root.join("real")).is_err());
    }

    #[test]
    fn asset_walk_sorts_by_components_and_lists_directories() {
        let temporary = tempfile::tempdir().expect("tempdir");
        let root = temporary.path().to_path_buf();
        fs::create_dir(root.join("a")).expect("mkdir a");
        fs::create_dir(root.join("a-b")).expect("mkdir a-b");
        fs::write(root.join("a/b"), b"x").expect("write");
        fs::write(root.join("a-b/x"), b"x").expect("write");
        fs::write(root.join(".hidden"), b"x").expect("write");
        let names: Vec<String> = walk_assets(&root)
            .iter()
            .map(|path| {
                path.strip_prefix(&root)
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default()
            })
            .collect();
        assert_eq!(names, [".hidden", "a", "a/b", "a-b", "a-b/x"]);
    }
}
