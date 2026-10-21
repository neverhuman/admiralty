//! `cargo xtask check`: the complete local CI matrix (formerly
//! `scripts/check/all.sh`).

use crate::{
    Error,
    process::{self, quoted_command_line},
};
use clap::Args;
use rustix::{fs::Mode, process::umask};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    ffi::{OsStr, OsString},
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::{Path, PathBuf},
    process::Stdio,
};

/// Number of stages in the local CI matrix.
pub const STAGE_COUNT: u8 = 16;
/// The real-browser journey stage, skipped by `--skip-browser`.
pub const BROWSER_STAGE: u8 = 16;
/// Repository-relative path of the pinned axe bundle.
pub const AXE_BUNDLE: &str = "node_modules/axe-core/axe.min.js";
/// SHA-256 of the pinned `axe-core` 4.14.0 `axe.min.js`.
pub const AXE_SHA256: &str = "20c09fe157a8a34a30e241aaa1fcdade657734f08ab379ecfbeb7d45cc46e878";
/// Node major version CI uses (`.github/workflows/ci.yml`).
pub const NODE_MAJOR: u32 = 24;
/// pnpm version pinned by `package.json`.
pub const PNPM_VERSION: &str = "11.15.1";
/// Number of log lines echoed to stderr for a failing stage.
pub const FAILURE_TAIL_LINES: usize = 80;

#[derive(Args, Debug, Clone, Default)]
pub struct CheckArgs {
    /// Print the numbered stage list and exit.
    #[arg(long)]
    pub list: bool,
    /// Start at stage N (earlier stages are not run).
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u8).range(1..=i64::from(STAGE_COUNT)), conflicts_with = "only")]
    pub from: Option<u8>,
    /// Run only stage N (repeatable).
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u8).range(1..=i64::from(STAGE_COUNT)))]
    pub only: Vec<u8>,
    /// Skip the real-browser journeys (stage 16) on hosts without Chrome/Chromium.
    #[arg(long)]
    pub skip_browser: bool,
}

/// What a stage executes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StageAction {
    /// An external command (wrapped in `rtk proxy` when available).
    Command(Vec<OsString>),
    /// In-process SHA-256 verification of a repository file.
    VerifySha256 { path: PathBuf, expected: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stage {
    pub number: u8,
    pub action: StageAction,
    /// Stage 16 needs the browser-journey exports and a real browser.
    pub browser: bool,
}

impl Stage {
    /// Arguments as printed (and appended to `commands.log`).
    pub fn display_argv(&self) -> Vec<OsString> {
        match &self.action {
            StageAction::Command(argv) => argv.clone(),
            StageAction::VerifySha256 { path, expected } => vec![
                OsString::from("xtask"),
                OsString::from("verify-sha256"),
                path.as_os_str().to_owned(),
                OsString::from(expected),
            ],
        }
    }
}

fn argv<I, S>(items: I) -> Vec<OsString>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    items
        .into_iter()
        .map(|item| item.as_ref().to_owned())
        .collect()
}

/// The ordered local CI matrix for a target directory and private scratch.
pub fn plan(target: &Path, scratch: &Path) -> Vec<Stage> {
    let debug = target.join("debug");
    let commands: Vec<Vec<OsString>> = vec![
        argv(["pnpm", "install", "--frozen-lockfile", "--ignore-scripts"]),
        argv(["cargo", "fmt", "--all", "--", "--check"]),
        argv([
            "cargo",
            "clippy",
            "--workspace",
            "--all-targets",
            "--locked",
            "--",
            "-D",
            "warnings",
        ]),
        argv(["cargo", "test", "--workspace", "--locked"]),
        argv([
            "cargo",
            "build",
            "-p",
            "admiralty-server",
            "-p",
            "admiralty-cli",
            "-p",
            "admiralty-observe",
            "-p",
            "admiralty-qualify",
            "--locked",
        ]),
        argv([
            debug.join("admiralty-canary").as_os_str(),
            OsStr::new("--admiraltyd-binary"),
            debug.join("admiraltyd").as_os_str(),
            OsStr::new("--admiralty-binary"),
            debug.join("admiralty").as_os_str(),
            OsStr::new("--scratch-root"),
            scratch.join("canary").as_os_str(),
        ]),
        argv([
            "cargo",
            "run",
            "-p",
            "admiralty-cli",
            "--locked",
            "--",
            "contracts",
            "check",
        ]),
        argv(["pnpm", "contracts:check"]),
        argv(["pnpm", "--dir", "packages/contracts", "test"]),
        argv(["pnpm", "scripts:check"]),
        argv(["pnpm", "--dir", "apps/dashboard", "typecheck"]),
        argv(["pnpm", "--dir", "apps/dashboard", "test", "--run"]),
        argv([
            "pnpm",
            "--dir",
            "apps/dashboard",
            "exec",
            "tsc",
            "-p",
            "e2e/tsconfig.json",
            "--noEmit",
        ]),
        argv([
            "env",
            "NODE_ENV=production",
            "pnpm",
            "--dir",
            "apps/dashboard",
            "build",
        ]),
    ];
    let mut stages: Vec<Stage> = commands
        .into_iter()
        .zip(1u8..)
        .map(|(command, number)| Stage {
            number,
            action: StageAction::Command(command),
            browser: false,
        })
        .collect();
    stages.push(Stage {
        number: 15,
        action: StageAction::VerifySha256 {
            path: PathBuf::from(AXE_BUNDLE),
            expected: AXE_SHA256.to_owned(),
        },
        browser: false,
    });
    stages.push(Stage {
        number: BROWSER_STAGE,
        action: StageAction::Command(argv([
            "pnpm",
            "--dir",
            "apps/dashboard",
            "test",
            "--run",
            "--config",
            "e2e/vitest.config.ts",
        ])),
        browser: true,
    });
    stages
}

/// Which stages a run executes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    pub stages: BTreeSet<u8>,
    pub skip_browser: bool,
}

impl Selection {
    pub fn from_args(args: &CheckArgs) -> Self {
        let stages: BTreeSet<u8> = if !args.only.is_empty() {
            args.only.iter().copied().collect()
        } else {
            (args.from.unwrap_or(1)..=STAGE_COUNT).collect()
        };
        Self {
            stages,
            skip_browser: args.skip_browser,
        }
    }

    /// Whether stage `number` was requested (before `--skip-browser`).
    pub fn requested(&self, number: u8) -> bool {
        self.stages.contains(&number)
    }

    /// Whether stage `number` actually runs.
    pub fn runs(&self, number: u8) -> bool {
        self.requested(number) && !(self.skip_browser && number == BROWSER_STAGE)
    }

    pub fn is_complete(&self) -> bool {
        !self.skip_browser && (1..=STAGE_COUNT).all(|n| self.stages.contains(&n))
    }

    /// The final success line.
    pub fn pass_line(&self, scratch: &Path) -> String {
        if self.is_complete() {
            return format!(
                "PASS: all {STAGE_COUNT} local CI stages; logs retained in {}/logs",
                scratch.display()
            );
        }
        let ran: Vec<u8> = (1..=STAGE_COUNT).filter(|n| self.runs(*n)).collect();
        let not_run: Vec<u8> = (1..=STAGE_COUNT).filter(|n| !self.runs(*n)).collect();
        let browser_note = if self.skip_browser && self.requested(BROWSER_STAGE) {
            "; browser stage 16 skipped by --skip-browser"
        } else {
            ""
        };
        format!(
            "PASS: {} of {STAGE_COUNT} local CI stages (ran {}; not run {}{browser_note}); NOT a complete local CI matrix; logs retained in {}/logs",
            ran.len(),
            stage_ranges(&ran),
            stage_ranges(&not_run),
            scratch.display()
        )
    }
}

/// Compact sorted stage numbers: `[1, 2, 3, 5]` -> `01-03,05`.
pub fn stage_ranges(numbers: &[u8]) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut index = 0;
    while index < numbers.len() {
        let start = numbers[index];
        let mut end = start;
        while index + 1 < numbers.len() && numbers[index + 1] == end + 1 {
            index += 1;
            end = numbers[index];
        }
        parts.push(if start == end {
            format!("{start:02}")
        } else {
            format!("{start:02}-{end:02}")
        });
        index += 1;
    }
    parts.join(",")
}

/// Print the stage list with symbolic paths (`--list`).
pub fn list(out: &mut dyn Write) -> io::Result<()> {
    let stages = plan(Path::new("$CARGO_TARGET_DIR"), Path::new("$SCRATCH"));
    for stage in stages {
        let line = stage
            .display_argv()
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" ");
        let note = match (&stage.action, stage.browser) {
            (StageAction::VerifySha256 { .. }, _) => "  [in-process SHA-256 pin of the axe bundle]",
            (_, true) => "  [real browser; skipped by --skip-browser]",
            _ => "",
        };
        writeln!(out, "{:02} {line}{note}", stage.number)?;
    }
    Ok(())
}

/// The fixed environment every stage receives (as `all.sh` exported it).
pub fn pinned_env(target: &Path) -> Vec<(OsString, OsString)> {
    vec![
        (
            OsString::from("CARGO_TARGET_DIR"),
            target.as_os_str().to_owned(),
        ),
        (OsString::from("CARGO_BUILD_JOBS"), OsString::from("2")),
        (
            OsString::from("CARGO_PROFILE_DEV_DEBUG"),
            OsString::from("0"),
        ),
        (
            OsString::from("CARGO_PROFILE_TEST_DEBUG"),
            OsString::from("0"),
        ),
        (OsString::from("RUST_TEST_THREADS"), OsString::from("1")),
    ]
}

/// `${ADMIRALTY_TARGET_DIR:-$root/target}`, which must be absolute.
pub fn target_dir(root: &Path, configured: Option<OsString>) -> Result<PathBuf, &'static str> {
    let target = match configured {
        Some(value) if !value.is_empty() => PathBuf::from(value),
        _ => root.join("target"),
    };
    if target.is_absolute() {
        Ok(target)
    } else {
        Err("ADMIRALTY_TARGET_DIR must be absolute")
    }
}

/// Required Node major: `ADMIRALTY_ALLOW_NODE_MAJOR` (local override) or 24.
pub fn required_node_major(configured: Option<OsString>) -> Result<(u32, bool), String> {
    match configured {
        Some(value) if !value.is_empty() => value
            .to_str()
            .and_then(|text| text.parse::<u32>().ok())
            .filter(|major| *major > 0)
            .map(|major| (major, major != NODE_MAJOR))
            .ok_or_else(|| "ADMIRALTY_ALLOW_NODE_MAJOR must be a positive integer".to_owned()),
        _ => Ok((NODE_MAJOR, false)),
    }
}

/// Whether `node -p 'process.versions.node.split(".")[0]'` output matches.
pub fn node_major_matches(output: &str, required: u32) -> bool {
    output.trim_end_matches('\n') == required.to_string()
}

/// Whether `pnpm --version` output matches the pinned version.
pub fn pnpm_version_matches(output: &str) -> bool {
    output.trim_end_matches('\n') == PNPM_VERSION
}

/// Restores the caller's umask when dropped.
pub struct UmaskGuard {
    caller: Mode,
}

impl UmaskGuard {
    /// Set `mask` and remember the caller's umask.
    pub fn set(mask: Mode) -> Self {
        Self {
            caller: umask(mask),
        }
    }

    pub fn caller(&self) -> Mode {
        self.caller
    }
}

impl Drop for UmaskGuard {
    fn drop(&mut self) {
        umask(self.caller);
    }
}

/// `/tmp`, or `/var/tmp` when `/tmp/.git` exists (shared hosts) and
/// `/var/tmp` is a directory outside Git.
pub fn scratch_base(tmp: &Path, var_tmp: &Path) -> PathBuf {
    if tmp.join(".git").exists() && var_tmp.is_dir() && !var_tmp.join(".git").exists() {
        var_tmp.to_path_buf()
    } else {
        tmp.to_path_buf()
    }
}

/// Create canonical private scratch `adm.XXXXXX` (0700) with `logs/` and
/// `canary/`, under umask 077, then restore the caller's umask. Returns the
/// canonical scratch path and the caller's umask bits.
pub fn create_scratch(base: &Path) -> io::Result<(PathBuf, u64)> {
    let guard = UmaskGuard::set(Mode::from_raw_mode(0o077));
    let scratch = tempfile::Builder::new()
        .prefix("adm.")
        .rand_bytes(6)
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in(base)?
        .keep();
    let scratch = scratch.canonicalize()?;
    let mut builder = fs::DirBuilder::new();
    builder.mode(0o700);
    builder.create(scratch.join("logs"))?;
    builder.create(scratch.join("canary"))?;
    let caller = u64::from(guard.caller().bits());
    drop(guard);
    Ok((scratch, caller))
}

/// Last `lines` lines of `bytes`, like `tail -n`.
pub fn tail(bytes: &[u8], lines: usize) -> &[u8] {
    if lines == 0 {
        return &bytes[bytes.len()..];
    }
    let body = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    let mut seen = 0;
    for (index, byte) in body.iter().enumerate().rev() {
        if *byte == b'\n' {
            seen += 1;
            if seen == lines {
                return &bytes[index + 1..];
            }
        }
    }
    bytes
}

/// SHA-256 hex digest of a file.
pub fn sha256_file(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex(&hasher.finalize()))
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut text, byte| {
        text.push_str(&format!("{byte:02x}"));
        text
    })
}

/// Executes stages with the `all.sh` logging contract.
pub struct Runner<'a> {
    pub root: &'a Path,
    pub logs: PathBuf,
    pub env: Vec<(OsString, OsString)>,
    pub rtk: Option<PathBuf>,
    pub out: &'a mut dyn Write,
    pub err: &'a mut dyn Write,
}

impl Runner<'_> {
    pub fn log_path(&self, number: u8) -> PathBuf {
        self.logs.join(format!("{number:02}.log"))
    }

    /// Run one stage. Returns `Ok(0)` on success, otherwise the exit code
    /// after printing the FAIL line and the log tail to stderr.
    pub fn run_stage(
        &mut self,
        stage: &Stage,
        extra_env: &[(OsString, OsString)],
    ) -> io::Result<i32> {
        let log_path = self.log_path(stage.number);
        let line = quoted_command_line(&stage.display_argv());
        writeln!(self.out, "{:02} {line}", stage.number)?;
        self.out.flush()?;
        let mut commands = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.logs.join("commands.log"))?;
        writeln!(commands, "{line}")?;
        drop(commands);
        let mut log = File::create(&log_path)?;
        let code = match &stage.action {
            StageAction::Command(argv) => {
                let executed = process::wrap_rtk(self.rtk.as_deref(), argv);
                let mut env = self.env.clone();
                env.extend(extra_env.iter().cloned());
                let mut command = process::command(&executed, &env)?;
                command
                    .current_dir(self.root)
                    .stdin(Stdio::null())
                    .stdout(log.try_clone()?)
                    .stderr(log.try_clone()?);
                match process::status(&mut command) {
                    Ok(status) => process::exit_code(status),
                    Err(error) => {
                        writeln!(
                            log,
                            "xtask: failed to start {}: {error}",
                            executed
                                .first()
                                .map(|p| p.to_string_lossy().into_owned())
                                .unwrap_or_default()
                        )?;
                        if error.kind() == io::ErrorKind::NotFound {
                            127
                        } else {
                            126
                        }
                    }
                }
            }
            StageAction::VerifySha256 { path, expected } => {
                match sha256_file(&self.root.join(path)) {
                    Ok(actual) => {
                        writeln!(log, "file={}", path.display())?;
                        writeln!(log, "expected_sha256={expected}")?;
                        writeln!(log, "actual_sha256={actual}")?;
                        if actual == *expected {
                            writeln!(log, "OK")?;
                            0
                        } else {
                            writeln!(log, "MISMATCH")?;
                            1
                        }
                    }
                    Err(error) => {
                        writeln!(log, "cannot read {}: {error}", path.display())?;
                        1
                    }
                }
            }
        };
        drop(log);
        if code == 0 {
            writeln!(self.out, "{:02} PASS {}", stage.number, log_path.display())?;
            self.out.flush()?;
            return Ok(0);
        }
        writeln!(
            self.err,
            "{:02} FAIL exit={code} log={}",
            stage.number,
            log_path.display()
        )?;
        let bytes = fs::read(&log_path)?;
        self.err.write_all(tail(&bytes, FAILURE_TAIL_LINES))?;
        self.err.flush()?;
        Ok(code)
    }

    /// Run the selected stages in order; stops at the first failure.
    pub fn run_selected(
        &mut self,
        stages: &[Stage],
        selection: &Selection,
        browser_env: &mut dyn FnMut() -> Result<Vec<(OsString, OsString)>, Error>,
    ) -> Result<i32, Error> {
        for stage in stages {
            if !selection.requested(stage.number) {
                continue;
            }
            if !selection.runs(stage.number) {
                writeln!(
                    self.out,
                    "{:02} SKIPPED (--skip-browser; browser journeys need Chrome or Chromium) {}",
                    stage.number,
                    quoted_command_line(&stage.display_argv())
                )?;
                continue;
            }
            let extra = if stage.browser {
                browser_env()?
            } else {
                Vec::new()
            };
            let code = self.run_stage(stage, &extra)?;
            if code != 0 {
                return Ok(code);
            }
        }
        Ok(0)
    }
}

/// Exports for the browser journeys (stage 16), with the daemon hash
/// computed in Rust.
pub fn browser_env(target: &Path) -> Result<Vec<(OsString, OsString)>, Error> {
    let daemon = target.join("debug/admiraltyd");
    let digest = sha256_file(&daemon)
        .map_err(|error| format!("cannot hash {}: {error}", daemon.display()))?;
    Ok(vec![
        (
            OsString::from("ADMIRALTY_E2E_ADMIRALTYD_BIN"),
            daemon.as_os_str().to_owned(),
        ),
        (
            OsString::from("ADMIRALTYD_BINARY"),
            daemon.as_os_str().to_owned(),
        ),
        (
            OsString::from("ADMIRALTY_E2E_OBSERVER_BIN"),
            target.join("debug/admiralty-observe").into_os_string(),
        ),
        (
            OsString::from("ADMIRALTY_E2E_ADMIRALTYD_SHA256"),
            OsString::from(digest),
        ),
    ])
}

fn probe(
    root: &Path,
    env: &[(OsString, OsString)],
    argv: &[&str],
    stdout: Stdio,
) -> Result<std::process::Output, Error> {
    let rendered = argv.join(" ");
    let argv: Vec<OsString> = argv.iter().map(OsString::from).collect();
    let mut command = process::command(&argv, env)?;
    command
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(Stdio::inherit());
    match process::spawn(&mut command) {
        Ok(child) => Ok(child.wait_with_output()?),
        Err(error) => Err(format!("cannot run {rendered}: {error}").into()),
    }
}

fn version_output(root: &Path, env: &[(OsString, OsString)], argv: &[&str]) -> Option<String> {
    probe(root, env, argv, Stdio::piped())
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// `cargo xtask check`.
pub fn run(root: &Path, args: &CheckArgs) -> Result<i32, Error> {
    let stdout = io::stdout();
    let stderr = io::stderr();
    let mut out = stdout.lock();
    let mut err = stderr.lock();
    if args.list {
        list(&mut out)?;
        return Ok(0);
    }
    let selection = Selection::from_args(args);
    if !(1..=STAGE_COUNT).any(|n| selection.runs(n)) {
        writeln!(
            err,
            "No local CI stage selected (only the skipped browser stage was requested)."
        )?;
        return Ok(2);
    }
    let target = match target_dir(root, std::env::var_os("ADMIRALTY_TARGET_DIR")) {
        Ok(target) => target,
        Err(message) => {
            writeln!(err, "{message}")?;
            return Ok(1);
        }
    };
    let mut env = pinned_env(&target);
    let (node_major, overridden) =
        match required_node_major(std::env::var_os("ADMIRALTY_ALLOW_NODE_MAJOR")) {
            Ok(value) => value,
            Err(message) => {
                writeln!(err, "{message}")?;
                return Ok(1);
            }
        };
    let node = version_output(
        root,
        &env,
        &["node", "-p", "process.versions.node.split(\".\")[0]"],
    );
    if !node
        .as_deref()
        .is_some_and(|text| node_major_matches(text, node_major))
    {
        if overridden {
            writeln!(
                err,
                "Local CI requires Node {node_major} (ADMIRALTY_ALLOW_NODE_MAJOR); CI uses Node {NODE_MAJOR}."
            )?;
        } else {
            writeln!(
                err,
                "Local CI requires Node 24, matching .github/workflows/ci.yml."
            )?;
        }
        return Ok(1);
    }
    if overridden {
        writeln!(
            err,
            "Node major override ADMIRALTY_ALLOW_NODE_MAJOR={node_major} is in effect; CI uses Node {NODE_MAJOR}, so this run is not CI-equivalent."
        )?;
    }
    let pnpm = version_output(root, &env, &["pnpm", "--version"]);
    if !pnpm.as_deref().is_some_and(pnpm_version_matches) {
        writeln!(
            err,
            "Local CI requires pnpm 11.15.1, matching package.json."
        )?;
        return Ok(1);
    }

    let base = scratch_base(Path::new("/tmp"), Path::new("/var/tmp"));
    let (scratch, caller_umask) = create_scratch(&base)?;
    env.push((OsString::from("TMPDIR"), scratch.as_os_str().to_owned()));
    let logs = scratch.join("logs");
    writeln!(out, "Local CI logs: {}", logs.display())?;
    out.flush()?;
    let probe_code = write_environment_log(
        root,
        &env,
        &logs,
        &target,
        caller_umask,
        node_major,
        overridden,
    )?;
    if probe_code != 0 {
        return Ok(probe_code);
    }

    let stages = plan(&target, &scratch);
    let mut runner = Runner {
        root,
        logs,
        env,
        rtk: process::find_rtk(),
        out: &mut out,
        err: &mut err,
    };
    let mut browser = || browser_env(&target);
    let code = runner.run_selected(&stages, &selection, &mut browser)?;
    if code != 0 {
        return Ok(code);
    }
    writeln!(out, "{}", selection.pass_line(&scratch))?;
    Ok(0)
}

fn write_environment_log(
    root: &Path,
    env: &[(OsString, OsString)],
    logs: &Path,
    target: &Path,
    caller_umask: u64,
    node_major: u32,
    overridden: bool,
) -> Result<i32, Error> {
    let mut log = File::create(logs.join("environment.log"))?;
    writeln!(
        log,
        "started_at_utc={}",
        chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ")
    )?;
    for argv in [
        &["git", "rev-parse", "HEAD"][..],
        &["git", "status", "--short"],
        &["rustc", "--version"],
        &["node", "--version"],
        &["pnpm", "--version"],
    ] {
        let output = probe(root, env, argv, Stdio::piped())?;
        log.write_all(&output.stdout)?;
        if !output.status.success() {
            // `set -e` ended all.sh with the probe's own status.
            let code = process::exit_code(output.status);
            eprintln!(
                "environment probe `{}` failed (exit={code})",
                argv.join(" ")
            );
            return Ok(code);
        }
    }
    writeln!(
        log,
        "target={} build_jobs=2 test_threads=1 caller_umask={caller_umask:04o}",
        target.display()
    )?;
    if overridden {
        writeln!(log, "node_major_override={node_major}")?;
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn args() -> CheckArgs {
        CheckArgs::default()
    }

    #[test]
    fn plan_has_sixteen_stages_in_the_documented_order() {
        let stages = plan(Path::new("/t"), Path::new("/s"));
        assert_eq!(stages.len(), usize::from(STAGE_COUNT));
        let numbers: Vec<u8> = stages.iter().map(|s| s.number).collect();
        assert_eq!(numbers, (1..=STAGE_COUNT).collect::<Vec<_>>());
        let lines: Vec<String> = stages
            .iter()
            .map(|s| quoted_command_line(&s.display_argv()))
            .collect();
        assert_eq!(lines[0], "pnpm install --frozen-lockfile --ignore-scripts ");
        assert_eq!(lines[1], "cargo fmt --all -- --check ");
        assert_eq!(
            lines[2],
            "cargo clippy --workspace --all-targets --locked -- -D warnings "
        );
        assert_eq!(lines[3], "cargo test --workspace --locked ");
        assert_eq!(
            lines[4],
            "cargo build -p admiralty-server -p admiralty-cli -p admiralty-observe -p admiralty-qualify --locked "
        );
        assert_eq!(
            lines[5],
            "/t/debug/admiralty-canary --admiraltyd-binary /t/debug/admiraltyd --admiralty-binary /t/debug/admiralty --scratch-root /s/canary "
        );
        assert_eq!(
            lines[6],
            "cargo run -p admiralty-cli --locked -- contracts check "
        );
        assert_eq!(lines[7], "pnpm contracts:check ");
        assert_eq!(lines[8], "pnpm --dir packages/contracts test ");
        assert_eq!(lines[9], "pnpm scripts:check ");
        assert_eq!(lines[10], "pnpm --dir apps/dashboard typecheck ");
        assert_eq!(lines[11], "pnpm --dir apps/dashboard test --run ");
        assert_eq!(
            lines[12],
            "pnpm --dir apps/dashboard exec tsc -p e2e/tsconfig.json --noEmit "
        );
        assert_eq!(
            lines[13],
            "env NODE_ENV=production pnpm --dir apps/dashboard build "
        );
        assert_eq!(
            lines[14],
            format!("xtask verify-sha256 {AXE_BUNDLE} {AXE_SHA256} ")
        );
        assert_eq!(
            lines[15],
            "pnpm --dir apps/dashboard test --run --config e2e/vitest.config.ts "
        );
        assert!(stages[15].browser);
        assert!(stages.iter().filter(|s| s.browser).count() == 1);
    }

    #[test]
    fn list_prints_every_stage_with_symbolic_paths() {
        let mut out = Vec::new();
        list(&mut out).expect("list");
        let text = String::from_utf8(out).expect("utf8");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 16);
        assert_eq!(
            lines[0],
            "01 pnpm install --frozen-lockfile --ignore-scripts"
        );
        assert!(lines[5].starts_with("06 $CARGO_TARGET_DIR/debug/admiralty-canary "));
        assert!(lines[5].ends_with("--scratch-root $SCRATCH/canary"));
        assert!(lines[14].contains("in-process SHA-256"));
        assert!(lines[15].contains("skipped by --skip-browser"));
    }

    #[test]
    fn selection_covers_from_only_and_skip_browser() {
        let full = Selection::from_args(&args());
        assert!(full.is_complete());
        assert!((1..=16).all(|n| full.runs(n)));
        assert_eq!(
            full.pass_line(Path::new("/tmp/adm.x")),
            "PASS: all 16 local CI stages; logs retained in /tmp/adm.x/logs"
        );

        let from = Selection::from_args(&CheckArgs {
            from: Some(14),
            ..args()
        });
        assert!(!from.runs(13) && from.runs(14) && from.runs(16));
        assert!(!from.is_complete());
        let line = from.pass_line(Path::new("/s"));
        assert_eq!(
            line,
            "PASS: 3 of 16 local CI stages (ran 14-16; not run 01-13); NOT a complete local CI matrix; logs retained in /s/logs"
        );
        assert!(line.contains("NOT a complete local CI matrix"));
        assert!(line.ends_with("logs retained in /s/logs"));

        let only = Selection::from_args(&CheckArgs {
            only: vec![6, 2],
            ..args()
        });
        assert_eq!(only.stages.iter().copied().collect::<Vec<_>>(), vec![2, 6]);

        let skip = Selection::from_args(&CheckArgs {
            skip_browser: true,
            ..args()
        });
        assert!(skip.requested(16) && !skip.runs(16) && skip.runs(15));
        assert!(!skip.is_complete());
        let line = skip.pass_line(Path::new("/s"));
        assert!(line.starts_with("PASS: 15 of 16 local CI stages"));
        assert!(
            line.contains("(ran 01-15; not run 16; browser stage 16 skipped by --skip-browser)")
        );
        assert_eq!(stage_ranges(&[]), "");
        assert_eq!(stage_ranges(&[2, 6]), "02,06");
        assert_eq!(stage_ranges(&[1, 2, 3, 5, 7, 8]), "01-03,05,07-08");
        assert!(line.contains("NOT a complete local CI matrix"));
    }

    #[test]
    fn target_dir_defaults_and_must_be_absolute() {
        let root = Path::new("/repo");
        assert_eq!(target_dir(root, None), Ok(PathBuf::from("/repo/target")));
        assert_eq!(
            target_dir(root, Some(OsString::new())),
            Ok(PathBuf::from("/repo/target"))
        );
        assert_eq!(
            target_dir(root, Some(OsString::from("/warm/target"))),
            Ok(PathBuf::from("/warm/target"))
        );
        assert_eq!(
            target_dir(root, Some(OsString::from("rel/target"))),
            Err("ADMIRALTY_TARGET_DIR must be absolute")
        );
    }

    #[test]
    fn pinned_env_matches_all_sh() {
        let env = pinned_env(Path::new("/t"));
        let rendered: Vec<String> = env
            .iter()
            .map(|(k, v)| format!("{}={}", k.to_string_lossy(), v.to_string_lossy()))
            .collect();
        assert_eq!(
            rendered,
            vec![
                "CARGO_TARGET_DIR=/t",
                "CARGO_BUILD_JOBS=2",
                "CARGO_PROFILE_DEV_DEBUG=0",
                "CARGO_PROFILE_TEST_DEBUG=0",
                "RUST_TEST_THREADS=1"
            ]
        );
    }

    #[test]
    fn node_and_pnpm_version_checks_are_strict_by_default() {
        assert_eq!(required_node_major(None), Ok((24, false)));
        assert_eq!(required_node_major(Some(OsString::new())), Ok((24, false)));
        assert_eq!(
            required_node_major(Some(OsString::from("22"))),
            Ok((22, true))
        );
        assert_eq!(
            required_node_major(Some(OsString::from("24"))),
            Ok((24, false))
        );
        assert!(required_node_major(Some(OsString::from("v22"))).is_err());
        assert!(required_node_major(Some(OsString::from("0"))).is_err());
        assert!(node_major_matches("24\n", 24));
        assert!(!node_major_matches("22\n", 24));
        assert!(!node_major_matches("240\n", 24));
        assert!(!node_major_matches(" 24\n", 24));
        assert!(pnpm_version_matches("11.15.1\n"));
        assert!(!pnpm_version_matches("11.15.10\n"));
        assert!(!pnpm_version_matches("10.0.0\n"));
    }

    #[test]
    fn scratch_base_prefers_var_tmp_only_when_tmp_is_inside_git() {
        let dir = tempfile::tempdir().expect("tempdir");
        let tmp = dir.path().join("tmp");
        let var_tmp = dir.path().join("var_tmp");
        fs::create_dir_all(&tmp).expect("mkdir tmp");
        fs::create_dir_all(&var_tmp).expect("mkdir var_tmp");
        assert_eq!(scratch_base(&tmp, &var_tmp), tmp);
        fs::create_dir(tmp.join(".git")).expect("mkdir .git");
        assert_eq!(scratch_base(&tmp, &var_tmp), var_tmp);
        fs::create_dir(var_tmp.join(".git")).expect("mkdir var .git");
        assert_eq!(scratch_base(&tmp, &var_tmp), tmp);
        assert_eq!(scratch_base(&tmp, &dir.path().join("missing")), tmp);
    }

    #[test]
    fn tail_matches_tail_n() {
        assert_eq!(tail(b"a\nb\nc\n", 2), b"b\nc\n");
        assert_eq!(tail(b"a\nb\nc", 2), b"b\nc");
        assert_eq!(tail(b"a\nb\n", 5), b"a\nb\n");
        assert_eq!(tail(b"", 80), b"");
        assert_eq!(tail(b"\n\n\n", 2), b"\n\n");
        let many: String = (1..=100).map(|n| format!("line{n}\n")).collect();
        let tailed = String::from_utf8(tail(many.as_bytes(), 80).to_vec()).expect("utf8");
        assert_eq!(tailed.lines().count(), 80);
        assert!(tailed.starts_with("line21\n"));
        assert!(tailed.ends_with("line100\n"));
    }

    #[test]
    fn sha256_and_hex_are_lowercase_sha256sum_compatible() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("abc");
        fs::write(&file, b"abc").expect("write");
        assert_eq!(
            sha256_file(&file).expect("hash"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    fn sh(script: &str) -> Vec<OsString> {
        argv(["/bin/sh", "-c", script])
    }

    struct Harness {
        dir: tempfile::TempDir,
        out: Vec<u8>,
        err: Vec<u8>,
    }

    impl Harness {
        fn new() -> Self {
            let dir = tempfile::tempdir().expect("tempdir");
            fs::create_dir(dir.path().join("logs")).expect("mkdir logs");
            Self {
                dir,
                out: Vec::new(),
                err: Vec::new(),
            }
        }

        fn run(&mut self, stages: &[Stage], selection: &Selection, rtk: Option<PathBuf>) -> i32 {
            let root = self.dir.path().to_path_buf();
            let mut runner = Runner {
                root: &root,
                logs: root.join("logs"),
                env: vec![(OsString::from("XTASK_TEST_PIN"), OsString::from("pinned"))],
                rtk,
                out: &mut self.out,
                err: &mut self.err,
            };
            let mut browser = || -> Result<Vec<(OsString, OsString)>, Error> {
                Ok(vec![(
                    OsString::from("XTASK_BROWSER"),
                    OsString::from("exported"),
                )])
            };
            runner
                .run_selected(stages, selection, &mut browser)
                .expect("run stages")
        }

        fn log(&self, name: &str) -> String {
            fs::read_to_string(self.dir.path().join("logs").join(name)).expect("read log")
        }
    }

    fn stage(number: u8, argv: Vec<OsString>, browser: bool) -> Stage {
        Stage {
            number,
            action: StageAction::Command(argv),
            browser,
        }
    }

    #[test]
    fn runner_logs_pass_lines_commands_and_stage_output() {
        let mut harness = Harness::new();
        let stages = vec![
            stage(
                1,
                sh("echo out-$XTASK_TEST_PIN; echo err >&2; pwd -P"),
                false,
            ),
            stage(2, sh("echo browser=$XTASK_BROWSER"), true),
        ];
        let selection = Selection {
            stages: [1, 2].into_iter().collect(),
            skip_browser: false,
        };
        assert_eq!(harness.run(&stages, &selection, None), 0);
        let logs = harness.dir.path().join("logs");
        let out = String::from_utf8(harness.out.clone()).expect("utf8");
        let expected = format!(
            "01 /bin/sh -c echo\\ out-\\$XTASK_TEST_PIN\\;\\ echo\\ err\\ \\>\\&2\\;\\ pwd\\ -P \n01 PASS {}\n02 /bin/sh -c echo\\ browser=\\$XTASK_BROWSER \n02 PASS {}\n",
            logs.join("01.log").display(),
            logs.join("02.log").display()
        );
        assert_eq!(out, expected);
        assert!(harness.err.is_empty());
        let first = harness.log("01.log");
        assert!(first.contains("out-pinned\n"));
        assert!(first.contains("err\n"));
        let canonical = harness.dir.path().canonicalize().expect("canonical root");
        assert!(first.contains(&format!("{}\n", canonical.display())));
        assert_eq!(harness.log("02.log"), "browser=exported\n");
        assert_eq!(
            harness.log("commands.log"),
            "/bin/sh -c echo\\ out-\\$XTASK_TEST_PIN\\;\\ echo\\ err\\ \\>\\&2\\;\\ pwd\\ -P \n/bin/sh -c echo\\ browser=\\$XTASK_BROWSER \n"
        );
    }

    #[test]
    fn runner_stops_on_failure_with_exit_code_and_last_eighty_lines() {
        let mut harness = Harness::new();
        let stages = vec![
            stage(
                1,
                sh("for i in $(seq 1 100); do echo line$i; done; exit 7"),
                false,
            ),
            stage(2, sh("echo never"), false),
        ];
        let selection = Selection {
            stages: [1, 2].into_iter().collect(),
            skip_browser: false,
        };
        assert_eq!(harness.run(&stages, &selection, None), 7);
        let logs = harness.dir.path().join("logs");
        let err = String::from_utf8(harness.err.clone()).expect("utf8");
        let mut lines = err.lines();
        assert_eq!(
            lines.next(),
            Some(format!("01 FAIL exit=7 log={}", logs.join("01.log").display()).as_str())
        );
        let rest: Vec<&str> = lines.collect();
        assert_eq!(rest.len(), 80);
        assert_eq!(rest.first(), Some(&"line21"));
        assert_eq!(rest.last(), Some(&"line100"));
        assert!(!logs.join("02.log").exists());
        let out = String::from_utf8(harness.out.clone()).expect("utf8");
        assert!(!out.contains("PASS"));
        assert!(!out.contains("02 "));
    }

    #[test]
    fn runner_reports_signals_and_missing_programs_like_bash() {
        let mut harness = Harness::new();
        let selection = Selection {
            stages: [1].into_iter().collect(),
            skip_browser: false,
        };
        let killed = vec![stage(1, sh("kill -TERM $$"), false)];
        assert_eq!(harness.run(&killed, &selection, None), 143);
        let mut harness = Harness::new();
        let missing = vec![stage(1, argv(["/nonexistent/xtask-missing-tool"]), false)];
        assert_eq!(harness.run(&missing, &selection, None), 127);
        assert!(harness.log("01.log").contains("failed to start"));
    }

    #[test]
    fn runner_keeps_fixed_numbers_for_partial_runs_and_prints_skipped_browser() {
        let mut harness = Harness::new();
        let stages = vec![
            stage(1, sh("exit 1"), false),
            stage(2, sh("echo two"), false),
            stage(16, sh("exit 1"), true),
        ];
        let selection = Selection {
            stages: [2, 16].into_iter().collect(),
            skip_browser: true,
        };
        assert_eq!(harness.run(&stages, &selection, None), 0);
        let out = String::from_utf8(harness.out.clone()).expect("utf8");
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].starts_with("02 /bin/sh"));
        assert!(lines[1].starts_with("02 PASS "));
        assert!(
            lines[2].starts_with(
                "16 SKIPPED (--skip-browser; browser journeys need Chrome or Chromium) "
            )
        );
        assert!(!harness.dir.path().join("logs/01.log").exists());
        assert!(!harness.dir.path().join("logs/16.log").exists());
    }

    #[test]
    fn runner_wraps_commands_in_rtk_proxy_without_logging_the_wrapper() {
        let mut harness = Harness::new();
        let bin = harness.dir.path().join("bin");
        fs::create_dir(&bin).expect("mkdir bin");
        let rtk = bin.join("rtk");
        fs::write(&rtk, "#!/bin/sh\necho \"rtk:$1\"\nshift\nexec \"$@\"\n").expect("write rtk");
        fs::set_permissions(&rtk, fs::Permissions::from_mode(0o755)).expect("chmod rtk");
        let stages = vec![stage(1, sh("echo wrapped"), false)];
        let selection = Selection {
            stages: [1].into_iter().collect(),
            skip_browser: false,
        };
        assert_eq!(harness.run(&stages, &selection, Some(rtk)), 0);
        assert_eq!(harness.log("01.log"), "rtk:proxy\nwrapped\n");
        assert_eq!(harness.log("commands.log"), "/bin/sh -c echo\\ wrapped \n");
    }

    #[test]
    fn runner_verifies_axe_hash_in_process() {
        let mut harness = Harness::new();
        let axe = harness.dir.path().join(AXE_BUNDLE);
        fs::create_dir_all(axe.parent().expect("parent")).expect("mkdir axe");
        fs::write(&axe, b"abc").expect("write axe");
        let pinned = |expected: &str| Stage {
            number: 15,
            action: StageAction::VerifySha256 {
                path: PathBuf::from(AXE_BUNDLE),
                expected: expected.to_owned(),
            },
            browser: false,
        };
        let selection = Selection {
            stages: [15].into_iter().collect(),
            skip_browser: false,
        };
        assert_eq!(harness.run(&[pinned(AXE_SHA256)], &selection, None), 1);
        let err = String::from_utf8(harness.err.clone()).expect("utf8");
        assert!(err.starts_with("15 FAIL exit=1 log="));
        assert!(err.contains("MISMATCH"));
        let mut harness_ok = Harness::new();
        let axe_ok = harness_ok.dir.path().join(AXE_BUNDLE);
        fs::create_dir_all(axe_ok.parent().expect("parent")).expect("mkdir axe");
        fs::write(&axe_ok, b"abc").expect("write axe");
        let abc = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert_eq!(harness_ok.run(&[pinned(abc)], &selection, None), 0);
        assert!(harness_ok.log("15.log").ends_with("OK\n"));
    }

    #[test]
    fn browser_env_hashes_the_daemon_in_rust() {
        let dir = tempfile::tempdir().expect("tempdir");
        let debug = dir.path().join("debug");
        fs::create_dir(&debug).expect("mkdir debug");
        fs::write(debug.join("admiraltyd"), b"abc").expect("write daemon");
        let env = browser_env(dir.path()).expect("browser env");
        let lookup = |key: &str| {
            env.iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.to_string_lossy().into_owned())
        };
        let daemon = debug.join("admiraltyd").display().to_string();
        assert_eq!(lookup("ADMIRALTY_E2E_ADMIRALTYD_BIN"), Some(daemon.clone()));
        assert_eq!(lookup("ADMIRALTYD_BINARY"), Some(daemon));
        assert_eq!(
            lookup("ADMIRALTY_E2E_OBSERVER_BIN"),
            Some(debug.join("admiralty-observe").display().to_string())
        );
        assert_eq!(
            lookup("ADMIRALTY_E2E_ADMIRALTYD_SHA256").as_deref(),
            Some("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
        );
        assert!(browser_env(&dir.path().join("missing")).is_err());
    }
}
