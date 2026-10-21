//! Generic replacement for the one-off delivery scripts
//! (`tools/send_add1.sh`, `send_add2.sh`, `send_add2b.sh`, `send_compute.sh`,
//! `send_notice2.sh`, `send_sc.sh`): a JSON plan describes the targets, the
//! idle gate, the retry budget, sent markers and the optional
//! coordination-log pointer (appended under `flock` with a bounded wait).
//!
//! Execution order (matching the scripts): `pre_log` → `prepare` →
//! `reset_sent_markers` → delivery passes → `fallback` for unsent targets →
//! `unsent_echo` for unsent targets → `list_sent_markers` → `done_echo`.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

use crate::Ctx;
use crate::compat::{command_substitution, io_error_text};
use crate::system::{denver_hms, utc_hms, utc_iso};

fn default_true() -> bool {
    true
}
fn default_paste_settle_ms() -> u64 {
    1000
}
fn default_lock_wait_secs() -> u64 {
    60
}
fn default_lock_poll_ms() -> u64 {
    100
}

/// The idle gate run before every paste attempt.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdleCheck {
    /// Hardcopy path = prefix + screen id (e.g. `/tmp/hc_a2_`).
    pub hardcopy_prefix: String,
    /// Require the Claude Code `? for shortcuts` footer (all historical
    /// scripts did); `false` selects the prompt-only heuristic.
    #[serde(default = "default_true")]
    pub require_shortcuts_footer: bool,
    /// `rm -f` the hardcopy first and require it to be non-empty afterwards
    /// (`send_compute.sh` / `send_notice2.sh`); `false` reuses a possibly
    /// stale file like `send_add1.sh` / `send_add2*.sh`.
    #[serde(default)]
    pub fresh_hardcopy: bool,
    /// Sleep between the hardcopy request and reading the file.
    pub settle_ms: u64,
}

/// One part of a prepared message file.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum PreparePart {
    /// Literal text (`printf "%s" TEXT`).
    Text(String),
    /// Raw contents of a file (`cat FILE`).
    File(String),
}

/// A file written before delivery starts (`printf ... > f`, `cat a > b`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrepareFile {
    pub path: String,
    pub parts: Vec<PreparePart>,
}

/// A coordination-log append step (`pre_log` or `fallback`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogStep {
    /// Appended text (a newline is added, like `printf '%s\n'`).
    pub template: String,
    /// Line printed after appending.
    pub echo: String,
    /// Source of `{message}` (default: the target's message file).
    #[serde(default)]
    pub message_file: Option<String>,
    /// Marker touched after appending (`/tmp/logged_add2_{key}`).
    #[serde(default)]
    pub logged_marker: Option<String>,
    /// `flock -w` seconds.
    #[serde(default = "default_lock_wait_secs")]
    pub lock_wait_secs: u64,
    /// Poll interval while waiting for the lock.
    #[serde(default = "default_lock_poll_ms")]
    pub lock_poll_ms: u64,
}

/// One delivery target. Missing `screen`, `coord_log` and `lock` default to
/// the config's `captains[key]` entry.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanTarget {
    pub key: String,
    #[serde(default)]
    pub screen: Option<String>,
    #[serde(default)]
    pub message_file: Option<String>,
    #[serde(default)]
    pub sent_marker: Option<String>,
    #[serde(default)]
    pub coord_log: Option<String>,
    /// Lock file for `coord_log`; `""` means append without a lock.
    #[serde(default)]
    pub lock: Option<String>,
}

/// A delivery plan.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliverPlan {
    #[serde(default)]
    pub description: Option<String>,
    /// Maximum number of passes over the targets (`seq 1 N`).
    pub passes: u32,
    /// Sleep after every pass that still had undelivered targets.
    pub interval_secs: u64,
    pub idle: IdleCheck,
    /// Sleep between `paste .` and the final CR.
    #[serde(default = "default_paste_settle_ms")]
    pub paste_settle_ms: u64,
    /// Default message file template (`{key}`, `{screen}`).
    #[serde(default)]
    pub message_file: Option<String>,
    /// Default sent-marker template (`{key}`, `{screen}`).
    #[serde(default)]
    pub sent_marker: Option<String>,
    /// Line printed after each successful paste.
    pub sent_echo: String,
    #[serde(default)]
    pub prepare: Vec<PrepareFile>,
    #[serde(default)]
    pub pre_log: Option<LogStep>,
    /// Remove every target's sent marker before the first pass.
    #[serde(default)]
    pub reset_sent_markers: bool,
    #[serde(default)]
    pub fallback: Option<LogStep>,
    /// Line printed for every target still unsent after the last pass
    /// (`send_sc.sh`: `SC busy 20 min; not pasted`).
    #[serde(default)]
    pub unsent_echo: Option<String>,
    /// Print the existing sent markers at the end (`ls /tmp/sent_*`).
    #[serde(default)]
    pub list_sent_markers: bool,
    #[serde(default)]
    pub done_echo: Option<String>,
    pub targets: Vec<PlanTarget>,
}

/// A target with every path resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTarget {
    pub key: String,
    pub screen: String,
    pub message_file: String,
    pub sent_marker: PathBuf,
    pub coord_log: Option<String>,
    pub lock: Option<String>,
}

const PATH_VARS: &[&str] = &["key", "screen"];
const ECHO_VARS: &[&str] = &[
    "key",
    "screen",
    "message_file",
    "message_basename",
    "coord_log",
    "utc_iso",
    "utc_hms",
    "denver_hms",
];
const LOG_VARS: &[&str] = &[
    "key",
    "screen",
    "message_file",
    "message_basename",
    "coord_log",
    "utc_iso",
    "utc_hms",
    "denver_hms",
    "message",
];

/// Render `{name}` placeholders (`{{` / `}}` escape braces).
pub fn render(
    template: &str,
    lookup: &mut dyn FnMut(&str) -> Option<Vec<u8>>,
) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(template.len());
    let mut rest = template;
    while let Some(pos) = rest.find(['{', '}']) {
        out.extend_from_slice(&rest.as_bytes()[..pos]);
        let tail = &rest[pos..];
        if let Some(after) = tail.strip_prefix("{{") {
            out.push(b'{');
            rest = after;
        } else if let Some(after) = tail.strip_prefix("}}") {
            out.push(b'}');
            rest = after;
        } else if tail.starts_with('{') {
            let Some(end) = tail.find('}') else {
                return Err(format!(
                    "unterminated placeholder in template: {template:?}"
                ));
            };
            let name = &tail[1..end];
            match lookup(name) {
                Some(v) => out.extend_from_slice(&v),
                None => {
                    return Err(format!(
                        "unknown placeholder {{{name}}} in template: {template:?}"
                    ));
                }
            }
            rest = &tail[end + 1..];
        } else {
            return Err(format!("unbalanced '}}' in template: {template:?}"));
        }
    }
    out.extend_from_slice(rest.as_bytes());
    Ok(out)
}

fn check_template(template: &str, allowed: &[&str]) -> Result<(), String> {
    render(template, &mut |name| allowed.contains(&name).then(Vec::new)).map(|_| ())
}

fn render_path(template: &str, key: &str, screen: &str) -> Result<String, String> {
    let bytes = render(template, &mut |name| match name {
        "key" => Some(key.as_bytes().to_vec()),
        "screen" => Some(screen.as_bytes().to_vec()),
        _ => None,
    })?;
    String::from_utf8(bytes).map_err(|e| e.to_string())
}

impl DeliverPlan {
    /// Parse and validate a plan file.
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read plan {}: {e}", path.display()))?;
        let plan: DeliverPlan = serde_json::from_str(&text)
            .map_err(|e| format!("invalid plan {}: {e}", path.display()))?;
        plan.validate()?;
        Ok(plan)
    }

    /// Static checks (placeholders, non-empty targets).
    pub fn validate(&self) -> Result<(), String> {
        if self.targets.is_empty() {
            return Err("plan has no targets".to_string());
        }
        let mut keys = std::collections::BTreeSet::new();
        for t in &self.targets {
            if !keys.insert(t.key.as_str()) {
                return Err(format!("duplicate target key {:?}", t.key));
            }
            for p in [&t.message_file, &t.sent_marker].into_iter().flatten() {
                check_template(p, PATH_VARS)?;
            }
        }
        for p in [&self.message_file, &self.sent_marker]
            .into_iter()
            .flatten()
        {
            check_template(p, PATH_VARS)?;
        }
        check_template(&self.sent_echo, ECHO_VARS)?;
        if let Some(unsent) = &self.unsent_echo {
            check_template(unsent, ECHO_VARS)?;
        }
        if let Some(done) = &self.done_echo {
            check_template(done, &[])?;
        }
        for step in [&self.pre_log, &self.fallback].into_iter().flatten() {
            check_template(&step.template, LOG_VARS)?;
            check_template(&step.echo, ECHO_VARS)?;
            for p in [&step.message_file, &step.logged_marker]
                .into_iter()
                .flatten()
            {
                check_template(p, PATH_VARS)?;
            }
        }
        Ok(())
    }

    /// Resolve every target against the config's captain table.
    pub fn resolve(
        &self,
        captains: &BTreeMap<String, crate::config::CaptainTarget>,
    ) -> Result<Vec<ResolvedTarget>, String> {
        let needs_log = self.pre_log.is_some() || self.fallback.is_some();
        self.targets
            .iter()
            .map(|t| {
                let cap = captains.get(&t.key);
                let screen = t
                    .screen
                    .clone()
                    .or_else(|| cap.map(|c| c.screen.clone()))
                    .ok_or_else(|| format!("target {:?}: no screen id", t.key))?;
                let message_tpl = t
                    .message_file
                    .as_ref()
                    .or(self.message_file.as_ref())
                    .ok_or_else(|| format!("target {:?}: no message_file", t.key))?;
                let marker_tpl = t
                    .sent_marker
                    .as_ref()
                    .or(self.sent_marker.as_ref())
                    .ok_or_else(|| format!("target {:?}: no sent_marker", t.key))?;
                let coord_log = t
                    .coord_log
                    .clone()
                    .or_else(|| cap.map(|c| c.coord_log.clone()))
                    .filter(|s| !s.is_empty());
                if needs_log && coord_log.is_none() {
                    return Err(format!("target {:?}: no coord_log", t.key));
                }
                let lock = t
                    .lock
                    .clone()
                    .or_else(|| cap.map(|c| c.lock.clone()))
                    .filter(|s| !s.is_empty());
                Ok(ResolvedTarget {
                    key: t.key.clone(),
                    message_file: render_path(message_tpl, &t.key, &screen)?,
                    sent_marker: PathBuf::from(render_path(marker_tpl, &t.key, &screen)?),
                    screen,
                    coord_log,
                    lock,
                })
            })
            .collect()
    }
}

/// The scripts' idle heuristic over a hardcopy:
/// `grep -aq "? for shortcuts"` (optional) `&& ! grep -aq "esc to cancel"
/// && [ "$(grep -a "^>" f | tail -1 | tr -d " ")" = ">" ]`.
pub fn prompt_is_idle(hardcopy: &[u8], require_shortcuts_footer: bool) -> bool {
    fn contains(hay: &[u8], needle: &[u8]) -> bool {
        hay.windows(needle.len()).any(|w| w == needle)
    }
    if require_shortcuts_footer && !contains(hardcopy, b"? for shortcuts") {
        return false;
    }
    if contains(hardcopy, b"esc to cancel") {
        return false;
    }
    let Some(last) = hardcopy
        .split(|b| *b == b'\n')
        .rfind(|l| l.starts_with(b">"))
    else {
        return false;
    };
    let squeezed: Vec<u8> = last
        .iter()
        .copied()
        .filter(|b| *b != b' ' && *b != 0)
        .collect();
    squeezed == b">"
}

/// Outcome of a locked append.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppendOutcome {
    Appended,
    LockTimeout,
}

/// Append `bytes` to `log`, holding an exclusive `flock` on `lock` (created
/// if missing, like `flock(1)`) and waiting at most `wait` for it.
pub fn append_locked(
    clock: &dyn crate::system::Clock,
    log: &Path,
    lock: Option<&Path>,
    wait: Duration,
    poll: Duration,
    bytes: &[u8],
) -> io::Result<AppendOutcome> {
    let _guard = match lock {
        None => None,
        Some(lock) => {
            let file = match File::open(lock) {
                Ok(f) => f,
                Err(e) if e.kind() == io::ErrorKind::NotFound => OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(false)
                    .open(lock)?,
                Err(e) => return Err(e),
            };
            let mut waited = Duration::ZERO;
            loop {
                match rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive)
                {
                    Ok(()) => break,
                    Err(e)
                        if e == rustix::io::Errno::WOULDBLOCK || e == rustix::io::Errno::INTR =>
                    {
                        if waited >= wait {
                            return Ok(AppendOutcome::LockTimeout);
                        }
                        let step = poll.max(Duration::from_millis(1));
                        clock.sleep(step);
                        waited += step;
                    }
                    Err(e) => return Err(io::Error::from(e)),
                }
            }
            Some(file)
        }
    };
    let mut f = OpenOptions::new().create(true).append(true).open(log)?;
    f.write_all(bytes)?;
    Ok(AppendOutcome::Appended)
}

fn touch(path: &Path) -> io::Result<()> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map(|_| ())
}

fn basename(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rsplit_once('/') {
        Some((_, b)) => b.to_string(),
        None => trimmed.to_string(),
    }
}

struct Deliverer<'c, 'a> {
    ctx: &'c mut Ctx<'a>,
    plan: &'c DeliverPlan,
}

impl Deliverer<'_, '_> {
    fn vars(&self, t: &ResolvedTarget, name: &str, message: Option<&[u8]>) -> Option<Vec<u8>> {
        let now = self.ctx.clock.now();
        let s = match name {
            "key" => t.key.clone(),
            "screen" => t.screen.clone(),
            "message_file" => t.message_file.clone(),
            "message_basename" => basename(&t.message_file),
            "coord_log" => t.coord_log.clone().unwrap_or_default(),
            "utc_iso" => utc_iso(now),
            "utc_hms" => utc_hms(now),
            "denver_hms" => denver_hms(now),
            "message" => return message.map(<[u8]>::to_vec),
            _ => return None,
        };
        Some(s.into_bytes())
    }

    fn echo(&mut self, template: &str, t: &ResolvedTarget) -> io::Result<()> {
        let line = render(template, &mut |n| self.vars(t, n, None)).map_err(io::Error::other)?;
        self.ctx.out.write_all(&line)?;
        self.ctx.out.write_all(b"\n")
    }

    fn read_message(&mut self, path: &str) -> io::Result<Vec<u8>> {
        match std::fs::read(path) {
            Ok(bytes) => Ok(command_substitution(&bytes)),
            Err(e) => {
                writeln!(self.ctx.err, "cat: {path}: {}", io_error_text(&e))?;
                Ok(Vec::new())
            }
        }
    }

    fn log_step(&mut self, step: &LogStep, t: &ResolvedTarget) -> io::Result<()> {
        let source = match &step.message_file {
            Some(tpl) => render_path(tpl, &t.key, &t.screen).map_err(io::Error::other)?,
            None => t.message_file.clone(),
        };
        let message = self.read_message(&source)?;
        let mut text = render(&step.template, &mut |n| self.vars(t, n, Some(&message)))
            .map_err(io::Error::other)?;
        text.push(b'\n');
        let log = t.coord_log.clone().unwrap_or_default();
        let lock = t.lock.as_deref().map(Path::new);
        let wait = Duration::from_secs(step.lock_wait_secs);
        let poll = Duration::from_millis(step.lock_poll_ms);
        match append_locked(self.ctx.clock, Path::new(&log), lock, wait, poll, &text) {
            Ok(AppendOutcome::Appended) => {}
            Ok(AppendOutcome::LockTimeout) => writeln!(
                self.ctx.err,
                "warning: could not lock {} within {}s; pointer NOT appended to {log}",
                t.lock.as_deref().unwrap_or_default(),
                step.lock_wait_secs
            )?,
            Err(e) => writeln!(self.ctx.err, "{log}: {}", io_error_text(&e))?,
        }
        if let Some(tpl) = &step.logged_marker {
            let marker = render_path(tpl, &t.key, &t.screen).map_err(io::Error::other)?;
            if let Err(e) = touch(Path::new(&marker)) {
                writeln!(
                    self.ctx.err,
                    "touch: cannot touch '{marker}': {}",
                    io_error_text(&e)
                )?;
            }
        }
        self.echo(&step.echo, t)
    }

    fn prepare(&mut self) -> io::Result<()> {
        for file in &self.plan.prepare {
            let mut bytes = Vec::new();
            for part in &file.parts {
                match part {
                    PreparePart::Text(t) => bytes.extend_from_slice(t.as_bytes()),
                    PreparePart::File(p) => match std::fs::read(p) {
                        Ok(b) => bytes.extend_from_slice(&b),
                        Err(e) => writeln!(self.ctx.err, "cat: {p}: {}", io_error_text(&e))?,
                    },
                }
            }
            if let Err(e) = std::fs::write(&file.path, &bytes) {
                writeln!(self.ctx.err, "{}: {}", file.path, io_error_text(&e))?;
            }
        }
        Ok(())
    }

    fn emit(&mut self, out: &crate::system::CmdOutput) -> io::Result<()> {
        self.ctx.out.write_all(&out.stdout)?;
        self.ctx.err.write_all(&out.stderr)
    }

    fn is_idle(&mut self, t: &ResolvedTarget) -> io::Result<bool> {
        let idle = &self.plan.idle;
        let path = PathBuf::from(format!("{}{}", idle.hardcopy_prefix, t.screen));
        if idle.fresh_hardcopy {
            let _ = std::fs::remove_file(&path);
        }
        let out = self.ctx.screen().hardcopy(&t.screen, None, false, &path);
        self.emit(&out)?;
        self.ctx.clock.sleep(Duration::from_millis(idle.settle_ms));
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) => {
                if !idle.fresh_hardcopy {
                    writeln!(
                        self.ctx.err,
                        "grep: {}: {}",
                        path.display(),
                        io_error_text(&e)
                    )?;
                }
                return Ok(false);
            }
        };
        if idle.fresh_hardcopy && bytes.is_empty() {
            return Ok(false);
        }
        Ok(prompt_is_idle(&bytes, idle.require_shortcuts_footer))
    }

    fn paste(&mut self, t: &ResolvedTarget) -> io::Result<()> {
        let screen = self.ctx.screen();
        let a = screen.readbuf(&t.screen, OsStr::new(&t.message_file));
        let b = screen.paste_dot(&t.screen);
        self.emit(&a)?;
        self.emit(&b)?;
        self.ctx
            .clock
            .sleep(Duration::from_millis(self.plan.paste_settle_ms));
        let c = self.ctx.screen().command(
            &t.screen,
            None,
            vec![
                std::ffi::OsString::from("stuff"),
                std::ffi::OsString::from("\r"),
            ],
        );
        self.emit(&c)
    }

    fn run(&mut self, targets: &[ResolvedTarget]) -> io::Result<()> {
        if let Some(step) = self.plan.pre_log.clone() {
            for t in targets {
                self.log_step(&step, t)?;
            }
        }
        self.prepare()?;
        if self.plan.reset_sent_markers {
            for t in targets {
                let _ = std::fs::remove_file(&t.sent_marker);
            }
        }
        for _ in 0..self.plan.passes {
            let mut left = false;
            for t in targets {
                if t.sent_marker.exists() {
                    continue;
                }
                left = true;
                if self.is_idle(t)? {
                    self.paste(t)?;
                    if let Err(e) = touch(&t.sent_marker) {
                        writeln!(
                            self.ctx.err,
                            "touch: cannot touch '{}': {}",
                            t.sent_marker.display(),
                            io_error_text(&e)
                        )?;
                    }
                    let tpl = self.plan.sent_echo.clone();
                    self.echo(&tpl, t)?;
                }
            }
            if !left {
                break;
            }
            self.ctx
                .clock
                .sleep(Duration::from_secs(self.plan.interval_secs));
        }
        if let Some(step) = self.plan.fallback.clone() {
            for t in targets {
                if t.sent_marker.exists() {
                    continue;
                }
                self.log_step(&step, t)?;
            }
        }
        if let Some(tpl) = self.plan.unsent_echo.clone() {
            for t in targets {
                if !t.sent_marker.exists() {
                    self.echo(&tpl, t)?;
                }
            }
        }
        if self.plan.list_sent_markers {
            let mut existing: Vec<&PathBuf> = targets
                .iter()
                .map(|t| &t.sent_marker)
                .filter(|p| p.exists())
                .collect();
            existing.sort();
            for p in existing {
                writeln!(self.ctx.out, "{}", p.display())?;
            }
        }
        if let Some(done) = &self.plan.done_echo {
            let line = render(done, &mut |_| None).map_err(io::Error::other)?;
            self.ctx.out.write_all(&line)?;
            self.ctx.out.write_all(b"\n")?;
        }
        Ok(())
    }
}

/// Execute a plan; returns the exit code (2 for an unresolvable plan).
pub fn deliver(ctx: &mut Ctx<'_>, plan: &DeliverPlan) -> io::Result<i32> {
    if let Err(e) = plan.validate() {
        writeln!(ctx.err, "error: {e}")?;
        return Ok(2);
    }
    let targets = match plan.resolve(&ctx.cfg.captains) {
        Ok(t) => t,
        Err(e) => {
            writeln!(ctx.err, "error: {e}")?;
            return Ok(2);
        }
    };
    Deliverer { ctx, plan }.run(&targets)?;
    Ok(0)
}

#[cfg(test)]
mod tests;
