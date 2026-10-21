//! Port of `tools/bin/kick`: one pass over the monitored sessions that
//! approves pending prompts, nudges PR-blocked / grant-waiting agents (with a
//! 15-minute cooldown), records per-session state files and appends to
//! `logs/kick.log`.

use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;
use serde::Serialize;
use serde_json::Value;

use crate::Ctx;
use crate::classify::{Classification, classify_file};
use crate::compat::{JsonLayout, py_float_repr, py_json, take_chars};
use crate::send::{SendOutcome, send_keys, send_text};
use crate::snap::snap;
use crate::system::local_stamp;

pub const PR_NUDGE: &str = "Continue. If anything is blocked on an open PR (CI, review, merge conflict), fix it and drive it to merge. Do not abandon, close, or reject the PR. Full permission — always approve. Keep going.";

pub const CLAUDE_GRANT_NUDGE: &str = "Keep going. Ping Codex again for the CPU grant / ledger slot if there is still no reply. Do not wait idle — re-check AGENT_CHAT, re-arm watches, and proceed on any unblocked work. Full permission / bypass — always approve.";

pub const CODEX_INPUT_YES: &str = "Yes — full permission / YOLO. Always approve. Proceed and keep going; if a PR is blocked, fix and merge it — do not abandon.";

pub const USAGE: &str = "usage: admiralty fleet kick [--dry-run] [--session NAME] [--once] [--inject 'text']\n       admiralty fleet kick --inject SESSION 'free text'\n";

/// Exit code when `state/DISK_HOLD` blocks live kicks.
pub const DISK_HOLD_EXIT: i32 = 75;

/// Parsed `kick` arguments (the script's hand-rolled parser, verbatim).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KickOpts {
    pub dry_run: bool,
    pub session_filter: Option<String>,
    pub inject_text: Option<String>,
    pub inject_session: Option<String>,
    pub inject_only: bool,
}

/// Why argument parsing printed usage (exit 2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KickUsage {
    pub unknown: Option<String>,
}

/// Parse like the script: `--inject X Y` means SESSION TEXT when another
/// argument follows and `X` has no space and does not start with
/// `Continue`; otherwise `X` is the text.
pub fn parse_kick_args(args: &[String]) -> Result<KickOpts, KickUsage> {
    let usage = Err(KickUsage { unknown: None });
    let get = |i: usize| args.get(i).cloned().unwrap_or_default();
    let mut o = KickOpts::default();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--dry-run" => o.dry_run = true,
            "--once" => {}
            "--session" => {
                i += 1;
                let v = get(i);
                if v.is_empty() {
                    return usage;
                }
                o.session_filter = Some(v);
            }
            "--inject" => {
                i += 1;
                let next = get(i);
                if next.is_empty() {
                    return usage;
                }
                let peek = get(i + 1);
                if !peek.is_empty() && !next.contains(' ') && !next.starts_with("Continue") {
                    o.inject_session = Some(next);
                    i += 1;
                    o.inject_text = Some(get(i));
                    o.inject_only = true;
                } else {
                    o.inject_text = Some(next);
                }
            }
            "-h" | "--help" => return usage,
            other => {
                return Err(KickUsage {
                    unknown: Some(other.to_string()),
                });
            }
        }
        i += 1;
    }
    Ok(o)
}

/// `classification` member of a state file.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum StateClassification {
    Full(Classification),
    /// `{"state":"error","summary":"snap failed"}`.
    SnapFailed {
        state: &'static str,
        summary: &'static str,
    },
    /// `{}` (classification unavailable after an inject).
    Empty {},
}

impl StateClassification {
    pub fn snap_failed() -> Self {
        Self::SnapFailed {
            state: "error",
            summary: "snap failed",
        }
    }
}

#[derive(Serialize)]
struct StateFile<'a> {
    session: &'a str,
    updated_epoch: i64,
    updated_at: String,
    last_action: &'a str,
    last_pr_nudge_epoch: i64,
    classification: &'a StateClassification,
}

/// Python `int(x or 0)` with every failure mapped to 0 (the script's
/// `|| echo 0`).
fn py_int_or_zero(v: Option<&Value>) -> i64 {
    match v {
        None | Some(Value::Null) => 0,
        Some(Value::Bool(b)) => i64::from(*b),
        Some(Value::Number(n)) => n
            .as_i64()
            .or_else(|| n.as_f64().map(|f| f.trunc() as i64))
            .unwrap_or(0),
        Some(Value::String(s)) => s.trim().parse::<i64>().unwrap_or(0),
        Some(_) => 0,
    }
}

/// bash `${#s}`: characters in a UTF-8 locale, bytes otherwise.
pub fn shell_len(s: &str, utf8: bool) -> usize {
    if utf8 { s.chars().count() } else { s.len() }
}

/// bash `${s:0:n}`: first `n` characters in a UTF-8 locale, bytes otherwise.
pub fn shell_prefix(s: &str, n: usize, utf8: bool) -> &[u8] {
    if utf8 {
        take_chars(s, n).as_bytes()
    } else {
        &s.as_bytes()[..s.len().min(n)]
    }
}

static INPUT_YES_CUE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"allow|approv|permission|yolo|proceed|continue|yes\b").expect("static regex")
});
static GRANT_CUE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)grant|ping codex|waiting on").expect("static regex"));

struct Kicker<'c, 'a> {
    ctx: &'c mut Ctx<'a>,
    opts: KickOpts,
    state_dir: PathBuf,
    log: PathBuf,
}

impl Kicker<'_, '_> {
    fn log_line(&mut self, msg: &str) {
        let line = format!(
            "[{}] {msg}\n",
            local_stamp(self.ctx.clock, self.ctx.clock.now())
        );
        let res = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log)
            .and_then(|mut f| f.write_all(line.as_bytes()));
        if let Err(e) = res {
            let _ = writeln!(self.ctx.err, "{}: {e}", self.log.display());
        }
    }

    fn state_path(&self, sess: &str) -> PathBuf {
        self.state_dir.join(format!("{sess}.json"))
    }

    fn last_kick_epoch(&self, sess: &str) -> i64 {
        let Ok(text) = std::fs::read_to_string(self.state_path(sess)) else {
            return 0;
        };
        match serde_json::from_str::<Value>(&text) {
            Ok(Value::Object(map)) => py_int_or_zero(map.get("last_pr_nudge_epoch")),
            _ => 0,
        }
    }

    fn write_state(
        &mut self,
        sess: &str,
        classification: &StateClassification,
        action: &str,
        pr_epoch: i64,
    ) {
        let now = self.ctx.clock.now();
        let doc = StateFile {
            session: sess,
            updated_epoch: now.timestamp(),
            updated_at: local_stamp(self.ctx.clock, now),
            last_action: action,
            last_pr_nudge_epoch: pr_epoch,
            classification,
        };
        let path = self.state_path(sess);
        let res = py_json(&doc, JsonLayout::Indent(2), true)
            .and_then(|json| std::fs::write(&path, format!("{json}\n")));
        if let Err(e) = res {
            let _ = writeln!(self.ctx.err, "cannot write {}: {e}", path.display());
        }
    }

    fn passthrough_err(&mut self, o: &SendOutcome) {
        let _ = self.ctx.err.write_all(&o.stderr);
    }

    fn keys(&mut self, sess: &str, keys: &str) {
        let o = send_keys(self.ctx, sess, keys);
        self.passthrough_err(&o);
    }

    fn text(&mut self, sess: &str, text: &str) {
        let o = send_text(self.ctx, sess, text);
        self.passthrough_err(&o);
    }

    fn pause(&self, ms: u64) {
        self.ctx.clock.sleep(Duration::from_millis(ms));
    }

    fn send_approve(&mut self, sess: &str, kind: &str, hint: &str) -> io::Result<()> {
        if self.opts.dry_run {
            writeln!(
                self.ctx.out,
                "  [dry-run] would approve kind={kind} hint={hint}"
            )?;
            return Ok(());
        }
        let desc = match kind {
            "claude" => {
                if hint == "2" || hint == "always" {
                    self.keys(sess, "2");
                    self.pause(200);
                    self.keys(sess, "enter");
                    "claude_approve:2"
                } else if hint == "bypass" {
                    self.keys(sess, "shifttab");
                    "claude_approve:shifttab"
                } else {
                    self.keys(sess, "2");
                    self.pause(150);
                    self.text(sess, "always allow");
                    "claude_approve:2+always"
                }
            }
            "grok" => {
                if hint == "a" {
                    self.keys(sess, "a");
                    "grok_approve:a"
                } else {
                    self.keys(sess, "a");
                    self.pause(200);
                    self.keys(sess, "y");
                    "grok_approve:a+y"
                }
            }
            "codex" => {
                self.keys(sess, "y");
                self.pause(100);
                self.text(sess, "always");
                "codex_approve:y+always"
            }
            _ => {
                self.keys(sess, "y");
                "unknown_approve:y"
            }
        };
        writeln!(self.ctx.out, "  action: {desc}")?;
        self.log_line(&format!("{sess} ACTION {desc}"));
        Ok(())
    }

    fn send_nudge(&mut self, sess: &str, msg: &str, label: &str) -> io::Result<()> {
        if self.opts.dry_run {
            write!(self.ctx.out, "  [dry-run] would inject ({label}): ")?;
            let prefix = shell_prefix(msg, 80, self.ctx.utf8_locale);
            self.ctx.out.write_all(prefix)?;
            writeln!(self.ctx.out, "...")?;
            return Ok(());
        }
        self.text(sess, msg);
        writeln!(
            self.ctx.out,
            "  action: {label} ({} chars)",
            shell_len(msg, self.ctx.utf8_locale)
        )?;
        self.log_line(&format!("{sess} ACTION {label}"));
        Ok(())
    }

    fn process_session(&mut self, sess: &str) -> io::Result<()> {
        writeln!(self.ctx.out, "======== {sess} ========")?;
        let outcome = snap(self.ctx, sess, None);
        let combined = outcome.combined();
        let path = match outcome.result {
            Ok(path) => path,
            Err(_) => {
                writeln!(self.ctx.out, "  ERROR: snap failed: {combined}")?;
                self.log_line(&format!("{sess} ERROR snap_failed: {combined}"));
                let epoch = self.last_kick_epoch(sess);
                self.write_state(
                    sess,
                    &StateClassification::snap_failed(),
                    "snap_failed",
                    epoch,
                );
                return Ok(());
            }
        };
        writeln!(self.ctx.out, "  snap: {combined}")?;

        let c = match classify_file(&path, sess) {
            Ok(c) => c,
            Err(e) => {
                writeln!(self.ctx.out, "  ERROR: classify failed: {e}")?;
                self.log_line(&format!("{sess} ERROR classify_failed"));
                return Ok(());
            }
        };
        let kind = c.kind.clone();
        let state = c.state.clone();
        let hint = c.approval_hint.clone().unwrap_or_default();
        let confidence = py_float_repr(c.confidence);
        let pr_refs = c.pr_refs.join(", ");
        writeln!(
            self.ctx.out,
            "  kind={kind}  state={state}  conf={confidence}  hint={}",
            if hint.is_empty() { "none" } else { &hint }
        )?;
        writeln!(
            self.ctx.out,
            "  pr_refs: {}",
            if pr_refs.is_empty() { "none" } else { &pr_refs }
        )?;
        writeln!(self.ctx.out, "  summary: {}", c.summary)?;

        let mut action = "none".to_string();
        let now = self.ctx.clock.now().timestamp();
        let mut pr_last = self.last_kick_epoch(sess);
        let cooldown = self.ctx.cfg.pr_cooldown_secs;
        let full = StateClassification::Full(c.clone());

        if let (Some(text), false) = (self.opts.inject_text.clone(), self.opts.inject_only) {
            self.send_nudge(sess, &text, "manual_inject")?;
            action = "manual_inject".to_string();
            self.write_state(sess, &full, &action, pr_last);
            self.log_line(&format!("{sess} state={state} action={action}"));
            writeln!(self.ctx.out)?;
            return Ok(());
        }

        match state.as_str() {
            "needs_approval" => {
                self.send_approve(sess, &kind, &hint)?;
                action = format!("approve:{}", if hint.is_empty() { "auto" } else { &hint });
            }
            "pr_blocked" => {
                if now - pr_last >= cooldown {
                    self.send_nudge(sess, PR_NUDGE, "pr_nudge")?;
                    action = "pr_nudge".to_string();
                    pr_last = now;
                } else {
                    let left = cooldown - (now - pr_last);
                    writeln!(self.ctx.out, "  skip pr_nudge (cooldown {left}s remaining)")?;
                    action = "pr_nudge_cooldown".to_string();
                }
            }
            "needs_input" => {
                let tail_low = c.tail.to_lowercase();
                if INPUT_YES_CUE.is_match(&tail_low) {
                    if kind == "codex" {
                        self.send_nudge(sess, CODEX_INPUT_YES, "input_yes")?;
                    } else {
                        let h = if hint.is_empty() { "y" } else { &hint };
                        let h = h.to_string();
                        self.send_approve(sess, &kind, &h)?;
                    }
                    action = "input_yes".to_string();
                } else {
                    writeln!(
                        self.ctx.out,
                        "  needs_input but no clear yes/approve cue — not auto-clicking"
                    )?;
                    action = "needs_input_ambiguous".to_string();
                }
            }
            "idle" => {
                let haystack = format!("{} {}", c.summary, c.to_pretty_json()?);
                if kind == "claude" && GRANT_CUE.is_match(&haystack) {
                    if now - pr_last >= cooldown {
                        self.send_nudge(sess, CLAUDE_GRANT_NUDGE, "claude_grant_nudge")?;
                        action = "claude_grant_nudge".to_string();
                        pr_last = now;
                    } else {
                        writeln!(self.ctx.out, "  skip grant nudge (cooldown)")?;
                        action = "grant_nudge_cooldown".to_string();
                    }
                } else {
                    writeln!(self.ctx.out, "  idle — no action")?;
                }
            }
            "working" | "error" | "unknown" => {
                writeln!(self.ctx.out, "  no auto-action for state={state}")?;
            }
            _ => {}
        }

        self.write_state(sess, &full, &action, pr_last);
        self.log_line(&format!(
            "{sess} state={state} kind={kind} action={action} conf={confidence}"
        ));
        writeln!(self.ctx.out, "  recorded action: {action}")?;
        writeln!(self.ctx.out)?;
        Ok(())
    }

    fn inject_only(&mut self, sess: &str, text: &str) -> io::Result<i32> {
        writeln!(self.ctx.out, "======== inject → {sess} ========")?;
        if self.opts.dry_run {
            write!(self.ctx.out, "  [dry-run] ")?;
            let prefix = shell_prefix(text, 100, self.ctx.utf8_locale);
            self.ctx.out.write_all(prefix)?;
            writeln!(self.ctx.out)?;
            return Ok(0);
        }
        let o = send_text(self.ctx, sess, text);
        self.ctx.out.write_all(&o.stdout)?;
        self.ctx.err.write_all(&o.stderr)?;
        if o.code != 0 {
            return Ok(o.code);
        }
        writeln!(self.ctx.out, "sent to {sess} (text)")?;
        self.log_line(&format!("{sess} ACTION manual_inject_cli"));
        let outcome = snap(self.ctx, sess, None);
        match outcome.result {
            Ok(path) if path.is_file() => {
                let cls = match classify_file(&path, sess) {
                    Ok(c) => StateClassification::Full(c),
                    Err(_) => StateClassification::Empty {},
                };
                let epoch = self.last_kick_epoch(sess);
                self.write_state(sess, &cls, "manual_inject_cli", epoch);
            }
            Ok(_) => {}
            Err(msg) => writeln!(self.ctx.err, "{msg}")?,
        }
        Ok(0)
    }
}

/// Run `kick` with raw arguments; returns the exit code.
pub fn kick(ctx: &mut Ctx<'_>, args: &[String]) -> io::Result<i32> {
    let opts = match parse_kick_args(args) {
        Ok(o) => o,
        Err(usage) => {
            if let Some(arg) = usage.unknown {
                writeln!(ctx.err, "unknown arg: {arg}")?;
            }
            ctx.err.write_all(USAGE.as_bytes())?;
            return Ok(2);
        }
    };
    let state_dir = ctx.cfg.state_dir();
    let log = ctx.cfg.kick_log();
    for dir in [
        state_dir.clone(),
        ctx.cfg.kicker_base.join("logs"),
        ctx.cfg.snaps_dir(),
    ] {
        if let Err(e) = std::fs::create_dir_all(&dir) {
            writeln!(
                ctx.err,
                "mkdir: cannot create directory '{}': {e}",
                dir.display()
            )?;
            return Ok(1);
        }
    }
    let mut k = Kicker {
        ctx,
        opts,
        state_dir,
        log,
    };

    // jope-disk-guard writes this while root free space is under 500 GiB.
    let hold = k.state_dir.join("DISK_HOLD");
    if hold.is_file() && !k.opts.dry_run {
        writeln!(
            k.ctx.err,
            "DISK_HOLD is set ({}). Refusing to kick or inject until root free space stays above 600 GiB and the guard removes this file.",
            hold.display()
        )?;
        if let Ok(bytes) = std::fs::read(&hold) {
            let mut taken = 0;
            let mut end = bytes.len();
            for (i, b) in bytes.iter().enumerate() {
                if *b == b'\n' {
                    taken += 1;
                    if taken == 5 {
                        end = i + 1;
                        break;
                    }
                }
            }
            k.ctx.err.write_all(&bytes[..end])?;
        }
        k.log_line("REFUSED disk hold");
        return Ok(DISK_HOLD_EXIT);
    }

    if k.opts.inject_only {
        let sess = k.opts.inject_session.clone().unwrap_or_default();
        let text = k.opts.inject_text.clone().unwrap_or_default();
        return k.inject_only(&sess, &text);
    }

    let sessions: Vec<String> = match &k.opts.session_filter {
        Some(s) => vec![s.clone()],
        None => k.ctx.cfg.monitored_sessions.clone(),
    };
    writeln!(
        k.ctx.out,
        "kick starting (dry_run={}) sessions: {}",
        u8::from(k.opts.dry_run),
        sessions.join(" ")
    )?;
    writeln!(k.ctx.out)?;
    for s in &sessions {
        k.process_session(s)?;
    }
    writeln!(k.ctx.out, "kick done.")?;
    Ok(0)
}

#[cfg(test)]
mod tests;
