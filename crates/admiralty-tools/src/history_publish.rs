//! Hourly history publisher (cron entry point), port of the retired
//! `tools/history_export_cron.sh`.
//!
//! Runs in its OWN clone so it never touches a live checkout or any service.
//! Commits ONLY explicit allowlisted files under `data/history/` and pushes
//! them straight to `main` as the configured data identity (documented
//! exception). Custody rules, in order:
//!
//! 1. Linux only; required tools present — both refused before any lock,
//!    log or checkout effect.
//! 2. Non-blocking exclusive `flock` on the lock file; a contended lock is a
//!    logged, successful no-op.
//! 3. Pre-existing staged index entries, dirty paths outside
//!    `data/history`, or dirty/untracked paths under `data/history` are
//!    refused without modifying the index or worktree.
//! 4. Fast-forward only; the exporter runs in-process; on failure tracked
//!    `data/history` is restored to `HEAD` and `*.tmp` scratch is deleted.
//! 5. Every changed path must match the exact allowlist; only allowlisted
//!    files are staged; the push retries three times with a rebase.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::LazyLock;

use chrono::Utc;
use regex::Regex;

use crate::error::{ToolError, ToolResult};
use crate::history_export::{self, ExportOptions};
use crate::proc;
use crate::pyjson::{self, LoadOptions};

/// Default clone (`ADMIRALTY_HISTORY_CLONE`).
pub const DEFAULT_CLONE: &str = "/home/ubuntu/.admiralty-history";
/// Default log (`ADMIRALTY_HISTORY_LOG`).
pub const DEFAULT_LOG: &str = "/home/ubuntu/xbabe2_kicker/captains-dashboard/history_export.log";
/// Default lock (`ADMIRALTY_HISTORY_LOCK`).
pub const DEFAULT_LOCK: &str = "/tmp/admiralty_history_export.lock";
/// Default child `HOME`.
pub const DEFAULT_HOME: &str = "/home/ubuntu";
/// Default child `PATH`.
pub const DEFAULT_PATH: &str = "/home/ubuntu/.local/bin:/usr/local/bin:/usr/bin:/bin";
/// Default clone URL.
pub const DEFAULT_REPOSITORY: &str = "https://github.com/neverhuman/admiralty.git";
/// Default data-commit identity name.
pub const DEFAULT_USER_NAME: &str = "jepsontaylor";
/// Default data-commit identity email.
pub const DEFAULT_USER_EMAIL: &str = "130782313+jepsontaylor@users.noreply.github.com";
/// Second paragraph of every data commit. It names the program that made
/// the commit; the shell publisher named the retired scripts instead.
pub const COMMIT_BODY: &str = "Automated: admiralty history export via admiralty history publish (exact allowlisted data/history paths only; see data/history/README.md).";

static ALLOWED: LazyLock<Regex> = LazyLock::new(|| {
    let date = r"[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]";
    Regex::new(&format!(
        r"^data/history/(?:README\.md|rates_index\.json|status\.jsonl|summary\.json|quota/codex_quota\.csv|resources/disk_used\.csv|resources/host_snapshots\.csv|tokens/codex_sessions\.csv|tokens/claude_hourly\.csv|timeseries/history\.csv|timeseries/history_{date}\.jsonl|ledger/model_usage\.csv|ledger/model_usage_{date}\.jsonl|token_yield/token_yield_{date}\.jsonl|snapshots/projects_{date}\.jsonl)\z"
    ))
    .expect("static allowlist pattern")
});

/// `is_allowed_export_path`: the exact `case` allowlist of the shell publisher.
pub fn is_allowed_export_path(path: &str) -> bool {
    ALLOWED.is_match(path)
}

/// Publisher configuration; `Default` reproduces the shell constants.
#[derive(Clone, Debug)]
pub struct PublishConfig {
    /// Dedicated publishing clone.
    pub clone: PathBuf,
    /// Append-only log receiving all output after the preflight checks.
    pub log: PathBuf,
    /// Lock file (`flock -n`).
    pub lock: PathBuf,
    /// `HOME` for Git children and the exporter.
    pub home: String,
    /// `PATH` for Git children and the tool check.
    pub path: String,
    /// Clone URL used when the clone is missing.
    pub repository_url: String,
    /// Commit identity name.
    pub user_name: String,
    /// Commit identity email.
    pub user_email: String,
    /// Host operating system (`std::env::consts::OS`).
    pub host_os: String,
    /// Commands that must be on `PATH`.
    pub required_tools: Vec<String>,
    /// Lower CPU (`nice 15`) and I/O (`ionice -c3`) priority before exporting.
    pub lower_priority: bool,
    /// Dashboard runtime override (`CDASH_RUNTIME`).
    pub dash: Option<PathBuf>,
}

impl Default for PublishConfig {
    fn default() -> Self {
        Self {
            clone: PathBuf::from(DEFAULT_CLONE),
            log: PathBuf::from(DEFAULT_LOG),
            lock: PathBuf::from(DEFAULT_LOCK),
            home: DEFAULT_HOME.to_owned(),
            path: DEFAULT_PATH.to_owned(),
            repository_url: DEFAULT_REPOSITORY.to_owned(),
            user_name: DEFAULT_USER_NAME.to_owned(),
            user_email: DEFAULT_USER_EMAIL.to_owned(),
            host_os: std::env::consts::OS.to_owned(),
            required_tools: vec!["git".to_owned(), "ionice".to_owned()],
            lower_priority: true,
            dash: None,
        }
    }
}

impl PublishConfig {
    /// Apply `ADMIRALTY_HISTORY_{CLONE,LOG,LOCK}` and `CDASH_RUNTIME` from a
    /// lookup (`${VAR:-default}`: empty values keep the default).
    pub fn from_lookup(lookup: &dyn Fn(&str) -> Option<String>) -> Self {
        let get = |key: &str| lookup(key).filter(|value| !value.is_empty());
        let mut config = Self::default();
        if let Some(clone) = get("ADMIRALTY_HISTORY_CLONE") {
            config.clone = PathBuf::from(clone);
        }
        if let Some(log) = get("ADMIRALTY_HISTORY_LOG") {
            config.log = PathBuf::from(log);
        }
        if let Some(lock) = get("ADMIRALTY_HISTORY_LOCK") {
            config.lock = PathBuf::from(lock);
        }
        config.dash = lookup("CDASH_RUNTIME").map(PathBuf::from);
        config
    }

    /// [`Self::from_lookup`] over the process environment.
    pub fn from_env() -> Self {
        Self::from_lookup(&|key| std::env::var(key).ok())
    }

    /// Apply optional JSON overrides (`home`, `path`, `repository_url`,
    /// `user_name`, `user_email`, `clone`, `log`, `lock`).
    pub fn apply_json(&mut self, text: &str) -> ToolResult<()> {
        let value = pyjson::loads(text, LoadOptions::default())
            .map_err(|error| ToolError::invalid(format!("invalid config JSON: {error}")))?;
        let map = value
            .as_object()
            .ok_or_else(|| ToolError::invalid("config must be a JSON object"))?;
        for (key, item) in map.iter() {
            let text = item
                .as_str()
                .ok_or_else(|| ToolError::invalid(format!("config {key} must be a string")))?
                .to_owned();
            match key.as_str() {
                "home" => self.home = text,
                "path" => self.path = text,
                "repository_url" => self.repository_url = text,
                "user_name" => self.user_name = text,
                "user_email" => self.user_email = text,
                "clone" => self.clone = PathBuf::from(text),
                "log" => self.log = PathBuf::from(text),
                "lock" => self.lock = PathBuf::from(text),
                other => return Err(ToolError::invalid(format!("unknown config key {other}"))),
            }
        }
        Ok(())
    }

    /// Exporter options for this clone (the shell ran the clone's exporter
    /// with `--out "$CLONE/data/history"` under `HOME`).
    pub fn export_options(&self) -> ExportOptions {
        let mut options = ExportOptions::defaults(self.clone.clone(), self.home.clone());
        options.out = self.clone.join("data").join("history");
        options.dash = self.dash.clone().unwrap_or_else(|| {
            PathBuf::from(format!("{}/xbabe2_kicker/captains-dashboard", self.home))
        });
        options
    }
}

/// In-process exporter: receives options and the log writer; returns the
/// exporter exit code (non-zero or `Err` means failure).
pub type Exporter<'a> = dyn FnMut(&ExportOptions, &mut dyn Write) -> ToolResult<i32> + 'a;

fn command_available(tool: &str, path: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.split(':').any(|directory| {
        let directory = if directory.is_empty() { "." } else { directory };
        fs::metadata(Path::new(directory).join(tool))
            .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
    })
}

fn stamp() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

struct Session<'a> {
    config: &'a PublishConfig,
    cwd: Option<PathBuf>,
    log: File,
}

impl Session<'_> {
    fn say(&mut self, line: &str) -> ToolResult<()> {
        writeln!(self.log, "{line}")?;
        Ok(())
    }

    fn git(&self, args: &[&str]) -> Command {
        let mut command = Command::new("git");
        command
            .args(args)
            .env("PATH", &self.config.path)
            .env("HOME", &self.config.home)
            .stdin(Stdio::null());
        if let Some(cwd) = &self.cwd {
            command.current_dir(cwd);
        }
        command
    }

    /// Run with stdout and stderr appended to the log.
    fn run(&mut self, args: &[&str]) -> ToolResult<i32> {
        let status = self
            .git(args)
            .stdout(self.log.try_clone()?)
            .stderr(self.log.try_clone()?)
            .status()?;
        Ok(proc::exit_code(&status))
    }

    /// Run with stdout to the log and stderr discarded (`2>/dev/null`).
    fn run_quiet(&mut self, args: &[&str]) -> ToolResult<i32> {
        let status = self
            .git(args)
            .stdout(self.log.try_clone()?)
            .stderr(Stdio::null())
            .status()?;
        Ok(proc::exit_code(&status))
    }

    /// `$(git ...)`: stdout captured (trailing newlines stripped), stderr to the log.
    fn capture(&mut self, args: &[&str]) -> ToolResult<String> {
        let output = self
            .git(args)
            .stdout(Stdio::piped())
            .stderr(self.log.try_clone()?)
            .output()?;
        let text = String::from_utf8_lossy(&output.stdout).into_owned();
        Ok(text.trim_end_matches('\n').to_owned())
    }

    fn refuse(&mut self, message: &str) -> ToolResult<i32> {
        self.say(message)?;
        self.run(&["status", "--short"])?;
        Ok(1)
    }
}

fn remove_scratch(directory: &Path) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            remove_scratch(&path);
        } else if kind.is_file()
            && path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().ends_with(".tmp"))
        {
            let _ = fs::remove_file(&path);
        }
    }
}

fn lower_priority() -> ToolResult<()> {
    rustix::process::nice(15)?;
    let pid = rustix::process::getpid().as_raw_nonzero().to_string();
    let status = Command::new("ionice")
        .args(["-c3", "-p", &pid])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(ToolError::invalid("ionice -c3 failed"))
    }
}

/// Run one publication. Messages before the log is opened go to `stderr`.
#[allow(clippy::too_many_lines)]
pub fn publish(
    config: &PublishConfig,
    exporter: &mut Exporter<'_>,
    stderr: &mut dyn Write,
) -> ToolResult<i32> {
    // flock/ionice are part of this Linux cron job's custody boundary. A
    // missing locking facility must never be reported as a contended skip.
    if config.host_os != "linux" {
        writeln!(
            stderr,
            "history export requires Linux with flock and ionice"
        )?;
        return Ok(1);
    }
    for tool in &config.required_tools {
        if !command_available(tool, &config.path) {
            writeln!(stderr, "history export missing required tool: {tool}")?;
            return Ok(1);
        }
    }
    let log = match OpenOptions::new()
        .append(true)
        .create(true)
        .open(&config.log)
    {
        Ok(log) => log,
        Err(error) => {
            writeln!(
                stderr,
                "history export cannot open log {}: {error}",
                config.log.display()
            )?;
            return Ok(1);
        }
    };
    let mut session = Session {
        config,
        cwd: None,
        log,
    };
    let lock = match OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&config.lock)
    {
        Ok(lock) => lock,
        Err(error) => {
            session.say(&format!(
                "history export cannot open lock {}: {error}",
                config.lock.display()
            ))?;
            return Ok(1);
        }
    };
    match rustix::fs::flock(&lock, rustix::fs::FlockOperation::NonBlockingLockExclusive) {
        Ok(()) => {}
        Err(error) if error == rustix::io::Errno::WOULDBLOCK => {
            session.say(&format!("{} skip: previous run still active", stamp()))?;
            return Ok(0);
        }
        Err(error) => {
            session.say(&format!(
                "history export could not establish lock custody (exit 1): {error}"
            ))?;
            return Ok(1);
        }
    }
    session.say(&format!("{} start", stamp()))?;
    if !config.clone.join(".git").is_dir() {
        let clone = config.clone.to_string_lossy().into_owned();
        if session.run(&["clone", "-q", &config.repository_url, &clone])? != 0 {
            return Ok(1);
        }
    }
    if !config.clone.is_dir() {
        session.say(&format!("cannot enter {}", config.clone.display()))?;
        return Ok(1);
    }
    session.cwd = Some(config.clone.clone());
    for (key, value) in [
        ("user.name", config.user_name.as_str()),
        ("user.email", config.user_email.as_str()),
    ] {
        let code = session.run(&["config", key, value])?;
        if code != 0 {
            return Ok(code);
        }
    }

    // Require full index and worktree custody BEFORE fetch/merge/export.
    // Never reset or overwrite pre-existing staged or dirty work.
    if session.run(&["diff", "--cached", "--quiet"])? != 0 {
        return session.refuse(
            "pre-existing staged index entries; refusing without modifying index or worktree",
        );
    }
    let outside = session.capture(&[
        "status",
        "--porcelain",
        "--untracked-files=all",
        "--",
        ".",
        ":!data/history",
    ])?;
    if !outside.is_empty() {
        return session
            .refuse("dirty outside data/history; refusing without modifying index or worktree");
    }
    let tracked_dirty = session.run(&["diff", "--quiet", "--", "data/history"])? != 0;
    if tracked_dirty
        || !session
            .capture(&[
                "status",
                "--porcelain",
                "--untracked-files=all",
                "--",
                "data/history",
            ])?
            .is_empty()
    {
        return session.refuse(
            "pre-existing dirty or untracked files under data/history; refusing without modifying index or worktree",
        );
    }

    if session.run(&["fetch", "-q", "origin", "main"])? != 0 {
        return Ok(1);
    }
    if session.run(&["merge", "-q", "--ff-only", "origin/main"])? != 0 {
        session.say("cannot fast-forward")?;
        return Ok(1);
    }

    let options = config.export_options();
    let exported = if config.lower_priority {
        lower_priority()
    } else {
        Ok(())
    }
    .and_then(|()| exporter(&options, &mut session.log));
    let succeeded = match exported {
        Ok(code) => code == 0,
        Err(error) => {
            session.say(&format!("history_export: {error}"))?;
            false
        }
    };
    if !succeeded {
        session
            .say("export failed or scan hit - restoring clean HEAD data/history tracked files")?;
        let _ = session.run_quiet(&["checkout", "-q", "HEAD", "--", "data/history"]);
        remove_scratch(&config.clone.join("data").join("history"));
        return Ok(1);
    }

    let status = session.capture(&["status", "--porcelain", "--untracked-files=all"])?;
    let mut candidates = Vec::new();
    for line in status.split('\n') {
        if line.is_empty() {
            continue;
        }
        let path = line.get(3..).unwrap_or("");
        if !is_allowed_export_path(path) {
            session.say(&format!(
                "refusing: exporter modified or created non-allowlisted path: {path}"
            ))?;
            return Ok(1);
        }
        candidates.push(path.to_owned());
    }
    for path in &candidates {
        if config.clone.join(path).is_file() {
            let code = session.run(&["add", "--", path])?;
            if code != 0 {
                return Ok(code);
            }
        }
    }
    if session.run(&["diff", "--cached", "--quiet"])? == 0 {
        session.say("no data change")?;
        return Ok(0);
    }
    for staged in session
        .capture(&["diff", "--cached", "--name-only"])?
        .split('\n')
    {
        if !staged.is_empty() && !is_allowed_export_path(staged) {
            session.say(&format!(
                "refusing: staged path outside exact export allowlist: {staged}"
            ))?;
            return Ok(1);
        }
    }
    let subject = format!(
        "data(history): hourly export {}",
        Utc::now().format("%Y-%m-%dT%H:%MZ")
    );
    let code = session.run(&["commit", "-q", "-m", &subject, "-m", COMMIT_BODY])?;
    if code != 0 {
        return Ok(code);
    }
    for _ in 0..3 {
        if session.run(&["push", "-q", "origin", "HEAD:main"])? == 0 {
            let head = session.capture(&["rev-parse", "--short", "HEAD"])?;
            session.say(&format!("pushed {head}"))?;
            return Ok(0);
        }
        let refreshed = session.run(&["fetch", "-q", "origin", "main"])? == 0
            && session.run(&["rebase", "-q", "origin/main"])? == 0;
        if !refreshed {
            let abort = session.run(&["rebase", "--abort"])?;
            if abort != 0 {
                return Ok(abort);
            }
            session.say("rebase failed")?;
            return Ok(1);
        }
    }
    session.say("push failed after retries")?;
    Ok(1)
}

/// The production exporter: [`history_export::export`] with both streams
/// appended to the publisher log.
pub fn default_exporter(options: &ExportOptions, log: &mut dyn Write) -> ToolResult<i32> {
    let mut errors = Vec::new();
    let code = history_export::export(options, log, &mut errors)?;
    log.write_all(&errors)?;
    Ok(code)
}

/// `admiralty history publish` arguments.
#[derive(clap::Args, Debug, Clone, Default)]
pub struct PublishArgs {
    /// Optional JSON overrides for host specifics (`home`, `path`,
    /// `repository_url`, `user_name`, `user_email`, `clone`, `log`, `lock`).
    #[arg(long)]
    pub config: Option<PathBuf>,
}

/// Run `history publish`; returns the exit code.
pub fn run(args: &PublishArgs) -> Result<i32, crate::BoxError> {
    let mut config = PublishConfig::from_env();
    if let Some(path) = &args.config {
        config.apply_json(&fs::read_to_string(path)?)?;
    }
    let mut stderr = io::stderr();
    let mut exporter = default_exporter;
    match publish(&config, &mut exporter, &mut stderr) {
        Ok(code) => Ok(code),
        Err(error) => {
            eprintln!("history publish: {error}");
            Ok(1)
        }
    }
}
