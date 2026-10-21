//! `cargo xtask security`: fail-closed supply-chain checks (formerly
//! `scripts/check/security-supply-chain.sh` via `tools/security-lane.sh`).
//!
//! Enforced scope: Cargo.lock/pnpm-lock.yaml integrity, `cargo metadata
//! --locked`, workflow permissions and checkout credential persistence,
//! actionlint (`.github/actionlint.yaml`), gitleaks git range/merge-diff scan,
//! and a SHA-256 provenance digest receipt in a fresh 0700 private directory.
//! `cargo audit`, `pnpm audit`, `pip-audit` and `grype` are NOT claimed here and
//! remain open in `receipts/integration/jankurai-triage-20261019.json` until
//! provisioned on all LAN runners.

use crate::{Error, check::sha256_file, process};
use rustix::{fs::Mode, process::umask};
use std::{
    ffi::{OsStr, OsString},
    fs::{self, OpenOptions},
    io::{self, Write},
    os::unix::{
        ffi::OsStrExt,
        fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    path::{Path, PathBuf},
    process::Stdio,
};

const PREFIX: &str = "[security-supply-chain]";
/// Tools that must be on `PATH`; there is no missing-tool skip.
pub const REQUIRED_TOOLS: [&str; 4] = ["cargo", "git", "actionlint", "gitleaks"];
/// Name of the digest receipt written into the private output directory.
pub const RECEIPT_NAME: &str = "supply-chain-digests.sha256";

/// Inputs the shell script read from its environment.
#[derive(Debug, Clone)]
pub struct SecurityEnv {
    /// Repository root (the script `cd`'d here).
    pub root: PathBuf,
    /// Effective `PATH` for tool lookup and children.
    pub path: OsString,
    /// `${TMPDIR:-/tmp}`.
    pub scratch_parent: PathBuf,
    /// `ADMIRALTY_SECURITY_SCAN_BASE` when non-empty.
    pub scan_base: Option<String>,
}

impl SecurityEnv {
    /// Read the environment exactly as the script did: unless
    /// `ADMIRALTY_SECURITY_NO_DEFAULT_PATH=1`, `$HOME/.cargo/bin` and
    /// `$HOME/.local/bin` are prepended to `PATH`.
    pub fn from_env(root: PathBuf) -> Self {
        let no_default_path = std::env::var_os("ADMIRALTY_SECURITY_NO_DEFAULT_PATH")
            .is_some_and(|value| value == "1");
        let inherited = std::env::var_os("PATH").unwrap_or_default();
        let home = std::env::var_os("HOME");
        let path = effective_path(no_default_path, home.as_deref(), &inherited);
        let scratch_parent = match std::env::var_os("TMPDIR") {
            Some(value) if !value.is_empty() => PathBuf::from(value),
            _ => PathBuf::from("/tmp"),
        };
        let scan_base = std::env::var_os("ADMIRALTY_SECURITY_SCAN_BASE")
            .filter(|value| !value.is_empty())
            .map(|value| value.to_string_lossy().into_owned());
        Self {
            root,
            path,
            scratch_parent,
            scan_base,
        }
    }
}

/// `PATH="${HOME}/.cargo/bin:${HOME}/.local/bin:${PATH}"` unless disabled.
pub fn effective_path(no_default_path: bool, home: Option<&OsStr>, inherited: &OsStr) -> OsString {
    match (no_default_path, home) {
        (false, Some(home)) => {
            let mut path = OsString::from(home);
            path.push("/.cargo/bin:");
            path.push(home);
            path.push("/.local/bin:");
            path.push(inherited);
            path
        }
        _ => inherited.to_owned(),
    }
}

/// How gitleaks walks history (`--log-opts`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScanRange {
    /// No event base: the latest commit including merge diffs.
    Latest,
    /// All-zero base (first push of a ref): complete reachable history.
    Full,
    /// `<base>..HEAD` including merge-parent diffs; the base must be verified.
    Since(String),
}

impl ScanRange {
    pub fn log_opts(&self) -> String {
        match self {
            Self::Latest => "-1 -m".to_owned(),
            Self::Full => "HEAD -m".to_owned(),
            Self::Since(base) => format!("{base}..HEAD -m"),
        }
    }
}

/// Validate `ADMIRALTY_SECURITY_SCAN_BASE` syntax: a complete 40- or 64-digit
/// lowercase hexadecimal Git object ID (`^([0-9a-f]{40}|[0-9a-f]{64})$`).
pub fn parse_scan_base(scan_base: Option<&str>) -> Result<ScanRange, &'static str> {
    let Some(base) = scan_base else {
        return Ok(ScanRange::Latest);
    };
    let complete = matches!(base.len(), 40 | 64)
        && base.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    if !complete {
        return Err("scan base must be a complete Git object ID");
    }
    if base.bytes().all(|b| b == b'0') {
        Ok(ScanRange::Full)
    } else {
        Ok(ScanRange::Since(base.to_owned()))
    }
}

/// `.github/workflows/*.yml` in glob (byte) order, excluding dot files.
pub fn workflow_files(root: &Path) -> io::Result<Vec<PathBuf>> {
    let mut names: Vec<OsString> = fs::read_dir(root.join(".github/workflows"))?
        .filter_map(Result::ok)
        .map(|entry| entry.file_name())
        .filter(|name| {
            let bytes = name.as_bytes();
            !bytes.starts_with(b".") && bytes.ends_with(b".yml")
        })
        .collect();
    names.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
    Ok(names
        .into_iter()
        .map(|name| Path::new(".github/workflows").join(name))
        .collect())
}

/// Every workflow must declare `permissions:` and disable checkout credential
/// persistence. Returns the first violation as a FAIL message.
pub fn check_workflow_text(name: &Path, text: &str) -> Result<(), String> {
    for needle in ["permissions:", "persist-credentials: false"] {
        if !text.contains(needle) {
            return Err(format!(
                "{PREFIX} FAIL: workflow '{}' does not contain '{needle}'",
                name.display()
            ));
        }
    }
    Ok(())
}

/// One `sha256sum`-format line (`<hex>  <path>`), including GNU escaping of
/// names that contain a backslash or newline.
pub fn sha256sum_line(hex: &str, name: &Path) -> String {
    let raw = name.to_string_lossy();
    if raw.contains('\\') || raw.contains('\n') {
        let escaped = raw.replace('\\', "\\\\").replace('\n', "\\n");
        format!("\\{hex}  {escaped}\n")
    } else {
        format!("{hex}  {raw}\n")
    }
}

/// Create the private `admiralty-sec-supply-chain.XXXXXX` output directory
/// and verify it is a 0700 non-symlink directory.
pub fn create_output_dir(scratch_parent: &Path) -> Result<PathBuf, String> {
    let out_dir = tempfile::Builder::new()
        .prefix("admiralty-sec-supply-chain.")
        .rand_bytes(6)
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in(scratch_parent)
        .map_err(|error| {
            format!(
                "{PREFIX} FAIL: cannot create output directory under '{}': {error}",
                scratch_parent.display()
            )
        })?
        .keep();
    let private = fs::symlink_metadata(&out_dir)
        .map(|meta| meta.is_dir() && meta.mode() & 0o7777 == 0o700)
        .unwrap_or(false);
    if !private {
        return Err(format!(
            "{PREFIX} FAIL: output directory '{}' is not a private 0700 non-symlink directory",
            out_dir.display()
        ));
    }
    Ok(out_dir)
}

struct Lane<'a> {
    env: &'a SecurityEnv,
    out: &'a mut dyn Write,
    err: &'a mut dyn Write,
}

impl Lane<'_> {
    fn fail(&mut self, message: &str) -> Result<i32, Error> {
        writeln!(self.err, "{message}")?;
        Ok(1)
    }

    fn tool(&self, name: &str) -> PathBuf {
        process::find_in_path(name, Some(&self.env.path)).unwrap_or_else(|| PathBuf::from(name))
    }

    /// Run a tool from the effective PATH in the repository root; stdin is
    /// closed, stderr is inherited, stdout optionally discarded.
    fn run(&mut self, tool: &str, args: &[&OsStr], quiet: bool) -> Result<i32, Error> {
        self.out.flush()?;
        let mut argv = vec![self.tool(tool).into_os_string()];
        argv.extend(args.iter().map(|arg| arg.to_os_string()));
        let mut command =
            process::command(&argv, &[(OsString::from("PATH"), self.env.path.clone())])?;
        command
            .current_dir(&self.env.root)
            .stdin(Stdio::null())
            .stdout(if quiet {
                Stdio::null()
            } else {
                Stdio::inherit()
            })
            .stderr(Stdio::inherit());
        match process::status(&mut command) {
            Ok(status) => Ok(process::exit_code(status)),
            Err(error) => {
                writeln!(self.err, "{PREFIX} FAIL: cannot run '{tool}': {error}")?;
                Ok(127)
            }
        }
    }
}

/// The lane body after the process umask is set; output goes to `out`/`err`
/// and child processes inherit the real stdout/stderr.
pub fn execute(env: &SecurityEnv, out: &mut dyn Write, err: &mut dyn Write) -> Result<i32, Error> {
    let mut lane = Lane { env, out, err };
    let root = &env.root;

    // Validate scratch before querying dependencies; this denial is independently testable.
    if fs::symlink_metadata(&env.scratch_parent).is_ok_and(|meta| meta.file_type().is_symlink()) {
        return lane.fail(&format!(
            "{PREFIX} FAIL: scratch parent '{}' must not be a symlink",
            env.scratch_parent.display()
        ));
    }
    for tool in REQUIRED_TOOLS {
        if process::find_in_path(tool, Some(&env.path)).is_none() {
            return lane.fail(&format!(
                "{PREFIX} FAIL: required tool '{tool}' is missing from PATH (no missing-tool skip allowed)"
            ));
        }
    }

    writeln!(
        lane.out,
        "{PREFIX} 1. Checking Cargo.lock and pnpm-lock.yaml integrity..."
    )?;
    for lockfile in ["Cargo.lock", "pnpm-lock.yaml"] {
        if !fs::metadata(root.join(lockfile)).is_ok_and(|meta| meta.len() > 0) {
            return lane.fail(&format!("{PREFIX} FAIL: '{lockfile}' is missing or empty"));
        }
    }
    let code = lane.run(
        "cargo",
        &[
            OsStr::new("metadata"),
            OsStr::new("--locked"),
            OsStr::new("--format-version"),
            OsStr::new("1"),
        ],
        true,
    )?;
    if code != 0 {
        writeln!(
            lane.err,
            "{PREFIX} FAIL: cargo metadata --locked failed (exit={code})"
        )?;
        return Ok(code);
    }

    writeln!(
        lane.out,
        "{PREFIX} 2. Checking GitHub Actions workflow permissions, checkout credential persistence, actionlint, and gitleaks..."
    )?;
    let workflows = match workflow_files(root) {
        Ok(workflows) if !workflows.is_empty() => workflows,
        Ok(_) | Err(_) => {
            return lane.fail(&format!(
                "{PREFIX} FAIL: no .github/workflows/*.yml files to check"
            ));
        }
    };
    for workflow in &workflows {
        let text = match fs::read(root.join(workflow)) {
            Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            Err(error) => {
                return lane.fail(&format!(
                    "{PREFIX} FAIL: cannot read workflow '{}': {error}",
                    workflow.display()
                ));
            }
        };
        if let Err(message) = check_workflow_text(workflow, &text) {
            return lane.fail(&message);
        }
    }
    let code = lane.run(
        "actionlint",
        &[
            OsStr::new("-config-file"),
            OsStr::new(".github/actionlint.yaml"),
        ],
        false,
    )?;
    if code != 0 {
        writeln!(lane.err, "{PREFIX} FAIL: actionlint failed (exit={code})")?;
        return Ok(code);
    }
    // GitHub test merges have no patch under plain `git log -1 -p`. Scan the
    // complete trusted event range, including merge-parent diffs. Without an
    // event base (for example local/manual runs), inspect the latest commit's
    // merge diffs.
    let range = match parse_scan_base(env.scan_base.as_deref()) {
        Ok(range) => range,
        Err(message) => return lane.fail(&format!("{PREFIX} FAIL: {message}")),
    };
    if let ScanRange::Since(base) = &range {
        let commit = format!("{base}^{{commit}}");
        let available = lane.run(
            "git",
            &[
                OsStr::new("cat-file"),
                OsStr::new("-e"),
                OsStr::new(&commit),
            ],
            false,
        )? == 0
            && lane.run(
                "git",
                &[
                    OsStr::new("merge-base"),
                    OsStr::new("--is-ancestor"),
                    OsStr::new(base),
                    OsStr::new("HEAD"),
                ],
                false,
            )? == 0;
        if !available {
            return lane.fail(&format!(
                "{PREFIX} FAIL: scan base must be an available ancestor commit"
            ));
        }
    }
    let log_opts = format!("--log-opts={}", range.log_opts());
    let code = lane.run(
        "gitleaks",
        &[
            OsStr::new("git"),
            OsStr::new("--no-banner"),
            OsStr::new("--redact"),
            OsStr::new(&log_opts),
            OsStr::new("."),
        ],
        false,
    )?;
    if code != 0 {
        writeln!(lane.err, "{PREFIX} FAIL: gitleaks failed (exit={code})")?;
        return Ok(code);
    }

    writeln!(
        lane.out,
        "{PREFIX} 3. Emitting deterministic dependency & provenance digest receipt..."
    )?;
    let out_dir = match create_output_dir(&env.scratch_parent) {
        Ok(out_dir) => out_dir,
        Err(message) => return lane.fail(&message),
    };
    let mut receipt = String::new();
    let inputs = [PathBuf::from("Cargo.lock"), PathBuf::from("pnpm-lock.yaml")]
        .into_iter()
        .chain(workflows);
    for input in inputs {
        let digest = match sha256_file(&root.join(&input)) {
            Ok(digest) => digest,
            Err(error) => {
                return lane.fail(&format!(
                    "{PREFIX} FAIL: cannot digest '{}': {error}",
                    input.display()
                ));
            }
        };
        receipt.push_str(&sha256sum_line(&digest, &input));
    }
    let receipt_path = out_dir.join(RECEIPT_NAME);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&receipt_path)?;
    file.write_all(receipt.as_bytes())?;
    file.sync_all()?;
    writeln!(lane.out, "{PREFIX} PASS ({})", receipt_path.display())?;
    Ok(0)
}

/// `cargo xtask security`: umask 077 for the lane and every child.
pub fn run(root: PathBuf) -> Result<i32, Error> {
    umask(Mode::from_raw_mode(0o077));
    let env = SecurityEnv::from_env(root);
    let stdout = io::stdout();
    let stderr = io::stderr();
    execute(&env, &mut stdout.lock(), &mut stderr.lock())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn effective_path_prepends_home_tools_unless_disabled() {
        let home = OsStr::new("/home/u");
        assert_eq!(
            effective_path(false, Some(home), OsStr::new("/usr/bin")),
            OsString::from("/home/u/.cargo/bin:/home/u/.local/bin:/usr/bin")
        );
        assert_eq!(
            effective_path(true, Some(home), OsStr::new("/usr/bin")),
            OsString::from("/usr/bin")
        );
        assert_eq!(
            effective_path(false, None, OsStr::new("/usr/bin")),
            OsString::from("/usr/bin")
        );
    }

    #[test]
    fn scan_base_requires_complete_lowercase_object_ids() {
        assert_eq!(parse_scan_base(None), Ok(ScanRange::Latest));
        assert_eq!(ScanRange::Latest.log_opts(), "-1 -m");
        let zero40 = "0".repeat(40);
        let zero64 = "0".repeat(64);
        assert_eq!(parse_scan_base(Some(&zero40)), Ok(ScanRange::Full));
        assert_eq!(parse_scan_base(Some(&zero64)), Ok(ScanRange::Full));
        assert_eq!(ScanRange::Full.log_opts(), "HEAD -m");
        let sha1 = "0123456789abcdef0123456789abcdef01234567";
        assert_eq!(
            parse_scan_base(Some(sha1)),
            Ok(ScanRange::Since(sha1.to_owned()))
        );
        assert_eq!(
            ScanRange::Since(sha1.to_owned()).log_opts(),
            format!("{sha1}..HEAD -m")
        );
        let sha256 = "a".repeat(64);
        assert!(parse_scan_base(Some(&sha256)).is_ok());
        for bad in [
            "",
            "abc",
            &"a".repeat(39),
            &"a".repeat(41),
            &"a".repeat(63),
            &"A".repeat(40),
            &format!("{}g", "a".repeat(39)),
            &format!("{} ", "a".repeat(40)),
            &format!("{}\n", "a".repeat(40)),
            "HEAD~1",
        ] {
            assert_eq!(
                parse_scan_base(Some(bad)),
                Err("scan base must be a complete Git object ID"),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn workflow_text_requires_permissions_and_no_persisted_credentials() {
        let name = Path::new(".github/workflows/x.yml");
        assert!(
            check_workflow_text(
                name,
                "permissions:\n  contents: read\n  persist-credentials: false\n"
            )
            .is_ok()
        );
        let missing = check_workflow_text(name, "persist-credentials: false\n");
        assert_eq!(
            missing,
            Err(
                "[security-supply-chain] FAIL: workflow '.github/workflows/x.yml' does not contain 'permissions:'"
                    .to_owned()
            )
        );
        assert!(check_workflow_text(name, "permissions:\npersist-credentials: true\n").is_err());
    }

    #[test]
    fn sha256sum_lines_match_coreutils_format() {
        assert_eq!(
            sha256sum_line("ab", Path::new("Cargo.lock")),
            "ab  Cargo.lock\n"
        );
        assert_eq!(sha256sum_line("ab", Path::new("a\\b")), "\\ab  a\\\\b\n");
    }

    #[test]
    fn output_dir_is_private_and_refused_when_not() {
        let dir = tempfile::tempdir().expect("tempdir");
        let out_dir = create_output_dir(dir.path()).expect("output dir");
        let meta = fs::symlink_metadata(&out_dir).expect("stat");
        assert!(meta.is_dir());
        assert_eq!(meta.mode() & 0o7777, 0o700);
        let name = out_dir
            .file_name()
            .expect("name")
            .to_string_lossy()
            .into_owned();
        assert!(name.starts_with("admiralty-sec-supply-chain."));
        assert_eq!(name.len(), "admiralty-sec-supply-chain.".len() + 6);
        assert!(create_output_dir(&dir.path().join("missing")).is_err());
    }

    /// A hermetic repository root plus fake tools that record their argv.
    struct Fixture {
        dir: tempfile::TempDir,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().expect("tempdir");
            let root = dir.path().join("repo");
            fs::create_dir_all(root.join(".github/workflows")).expect("mkdir workflows");
            fs::write(root.join("Cargo.lock"), "lock\n").expect("Cargo.lock");
            fs::write(root.join("pnpm-lock.yaml"), "pnpm\n").expect("pnpm-lock");
            for name in ["b.yml", "a.yml"] {
                fs::write(
                    root.join(".github/workflows").join(name),
                    format!(
                        "name: {name}\npermissions:\n  contents: read\npersist-credentials: false\n"
                    ),
                )
                .expect("workflow");
            }
            fs::write(root.join(".github/workflows/notes.yaml"), "ignored\n").expect("yaml");
            fs::create_dir(dir.path().join("bin")).expect("mkdir bin");
            fs::create_dir(dir.path().join("scratch")).expect("mkdir scratch");
            let fixture = Self { dir };
            for tool in REQUIRED_TOOLS {
                fixture.tool(tool, 0);
            }
            fixture
        }

        fn root(&self) -> PathBuf {
            self.dir.path().join("repo")
        }

        fn calls(&self) -> String {
            fs::read_to_string(self.dir.path().join("calls.log")).unwrap_or_default()
        }

        fn tool(&self, name: &str, code: i32) {
            let path = self.dir.path().join("bin").join(name);
            let log = self.dir.path().join("calls.log");
            let _ = fs::remove_file(&path);
            fs::write(
                &path,
                format!(
                    "#!/bin/sh\nprintf '%s|%s\\n' \"{name}\" \"$*\" >> '{}'\nexit {code}\n",
                    log.display()
                ),
            )
            .expect("write fake tool");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod tool");
        }

        fn env(&self, scan_base: Option<&str>) -> SecurityEnv {
            SecurityEnv {
                root: self.root(),
                path: self.dir.path().join("bin").into_os_string(),
                scratch_parent: self.dir.path().join("scratch"),
                scan_base: scan_base.map(str::to_owned),
            }
        }

        fn execute(&self, env: &SecurityEnv) -> (i32, String, String) {
            let mut out = Vec::new();
            let mut err = Vec::new();
            let code = execute(env, &mut out, &mut err).expect("execute lane");
            (
                code,
                String::from_utf8(out).expect("utf8 out"),
                String::from_utf8(err).expect("utf8 err"),
            )
        }
    }

    #[test]
    fn lane_passes_and_writes_a_private_sha256sum_receipt() {
        let fixture = Fixture::new();
        let (code, out, err) = fixture.execute(&fixture.env(None));
        assert_eq!(code, 0, "stderr: {err}");
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 4);
        assert_eq!(
            lines[0],
            "[security-supply-chain] 1. Checking Cargo.lock and pnpm-lock.yaml integrity..."
        );
        assert!(lines[1].starts_with("[security-supply-chain] 2. Checking GitHub Actions"));
        assert!(lines[2].starts_with("[security-supply-chain] 3. Emitting deterministic"));
        let receipt = lines[3]
            .strip_prefix("[security-supply-chain] PASS (")
            .and_then(|rest| rest.strip_suffix(')'))
            .map(PathBuf::from)
            .expect("PASS line names the receipt");
        assert_eq!(receipt.file_name(), Some(OsStr::new(RECEIPT_NAME)));
        let parent = receipt.parent().expect("receipt dir");
        assert!(parent.starts_with(fixture.dir.path().join("scratch")));
        assert_eq!(
            fs::symlink_metadata(parent).expect("stat dir").mode() & 0o7777,
            0o700
        );
        assert_eq!(
            fs::metadata(&receipt).expect("stat receipt").mode() & 0o777,
            0o600
        );
        let root = fixture.root();
        let expected: String = [
            "Cargo.lock",
            "pnpm-lock.yaml",
            ".github/workflows/a.yml",
            ".github/workflows/b.yml",
        ]
        .iter()
        .map(|name| {
            format!(
                "{}  {name}\n",
                sha256_file(&root.join(name)).expect("digest")
            )
        })
        .collect();
        assert_eq!(fs::read_to_string(&receipt).expect("receipt"), expected);
        assert_eq!(
            fixture.calls(),
            "cargo|metadata --locked --format-version 1\nactionlint|-config-file .github/actionlint.yaml\ngitleaks|git --no-banner --redact --log-opts=-1 -m .\n"
        );
    }

    #[test]
    fn lane_scans_full_history_for_zero_base_and_verifies_real_bases() {
        let fixture = Fixture::new();
        let zero = "0".repeat(40);
        let (code, _, err) = fixture.execute(&fixture.env(Some(&zero)));
        assert_eq!(code, 0, "stderr: {err}");
        assert!(
            fixture
                .calls()
                .ends_with("gitleaks|git --no-banner --redact --log-opts=HEAD -m .\n")
        );
        assert!(!fixture.calls().contains("git|"));

        let fixture = Fixture::new();
        let base = "0123456789abcdef0123456789abcdef01234567";
        let (code, _, err) = fixture.execute(&fixture.env(Some(base)));
        assert_eq!(code, 0, "stderr: {err}");
        let calls = fixture.calls();
        assert!(calls.contains(&format!("git|cat-file -e {base}^{{commit}}\n")));
        assert!(calls.contains(&format!("git|merge-base --is-ancestor {base} HEAD\n")));
        assert!(calls.ends_with(&format!(
            "gitleaks|git --no-banner --redact --log-opts={base}..HEAD -m .\n"
        )));
    }

    #[test]
    fn lane_rejects_malformed_or_unavailable_scan_bases_before_gitleaks() {
        let fixture = Fixture::new();
        let (code, _, err) = fixture.execute(&fixture.env(Some("deadbeef")));
        assert_eq!(code, 1);
        assert_eq!(
            err,
            "[security-supply-chain] FAIL: scan base must be a complete Git object ID\n"
        );
        assert!(!fixture.calls().contains("gitleaks|"));

        let fixture = Fixture::new();
        fixture.tool("git", 1);
        let base = "0123456789abcdef0123456789abcdef01234567";
        let (code, _, err) = fixture.execute(&fixture.env(Some(base)));
        assert_eq!(code, 1);
        assert_eq!(
            err,
            "[security-supply-chain] FAIL: scan base must be an available ancestor commit\n"
        );
        assert!(!fixture.calls().contains("gitleaks|"));
    }

    #[test]
    fn lane_fails_closed_on_lockfiles_workflows_and_tool_failures() {
        let fixture = Fixture::new();
        fs::write(fixture.root().join("pnpm-lock.yaml"), "").expect("truncate");
        let (code, _, err) = fixture.execute(&fixture.env(None));
        assert_eq!(code, 1);
        assert!(err.contains("'pnpm-lock.yaml' is missing or empty"));
        assert_eq!(fixture.calls(), "");

        let fixture = Fixture::new();
        fixture.tool("cargo", 101);
        let (code, _, err) = fixture.execute(&fixture.env(None));
        assert_eq!(code, 101);
        assert!(err.contains("cargo metadata --locked failed"));

        let fixture = Fixture::new();
        fs::write(
            fixture.root().join(".github/workflows/b.yml"),
            "permissions: {}\n",
        )
        .expect("rewrite workflow");
        let (code, _, err) = fixture.execute(&fixture.env(None));
        assert_eq!(code, 1);
        assert!(
            err.contains("'.github/workflows/b.yml' does not contain 'persist-credentials: false'")
        );
        assert!(!fixture.calls().contains("actionlint|"));

        let fixture = Fixture::new();
        fixture.tool("actionlint", 3);
        let (code, _, _) = fixture.execute(&fixture.env(None));
        assert_eq!(code, 3);
        assert!(!fixture.calls().contains("gitleaks|"));

        let fixture = Fixture::new();
        fixture.tool("gitleaks", 1);
        let (code, out, _) = fixture.execute(&fixture.env(None));
        assert_eq!(code, 1);
        assert!(!out.contains("PASS"));
        assert_eq!(
            fs::read_dir(fixture.dir.path().join("scratch"))
                .expect("scratch")
                .count(),
            0
        );
    }

    #[test]
    fn lane_refuses_symlinked_scratch_before_tool_lookup() {
        let fixture = Fixture::new();
        let link = fixture.dir.path().join("scratch-link");
        symlink(fixture.dir.path().join("scratch"), &link).expect("symlink");
        let mut env = fixture.env(None);
        env.scratch_parent = link.clone();
        env.path = OsString::new();
        let (code, out, err) = fixture.execute(&env);
        assert_eq!(code, 1);
        assert!(out.is_empty());
        assert_eq!(
            err,
            format!(
                "[security-supply-chain] FAIL: scratch parent '{}' must not be a symlink\n",
                link.display()
            )
        );
    }

    #[test]
    fn lane_names_each_missing_tool_without_skipping() {
        for missing in REQUIRED_TOOLS {
            let fixture = Fixture::new();
            fs::remove_file(fixture.dir.path().join("bin").join(missing)).expect("remove tool");
            let (code, out, err) = fixture.execute(&fixture.env(None));
            assert_eq!(code, 1);
            assert!(out.is_empty());
            assert_eq!(
                err,
                format!(
                    "[security-supply-chain] FAIL: required tool '{missing}' is missing from PATH (no missing-tool skip allowed)\n"
                )
            );
            assert_eq!(fixture.calls(), "");
        }
    }
}
