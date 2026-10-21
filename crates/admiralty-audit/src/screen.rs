//! Terminal-screen classification (`screen_state`, `tail_busy`, `tail_hash`)
//! and the screen/process helpers (`screens`, `descendants`, `pty_last_input`,
//! `remote_screens`, `remote_hash`).
//!
//! These are hints only: they never authorize input or transport.

use std::collections::{BTreeMap, HashMap};
use std::sync::LazyLock;

use regex::Regex;

use crate::ctx::{Audit, sha256_hex};
use crate::py::{self, Dt, J};

/// `screens()` result: screen name -> pid.
pub type Screens = BTreeMap<String, i64>;

fn re(p: &str) -> Regex {
    Regex::new(p).expect("valid built-in regex")
}

/// SURVEY without its look-behind alternative (handled by [`survey_scale`]).
static SURVEY_BASE: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"(?i)(feedback survey|how (would|likely|satisfied)|rate (your|this|us)|take a (quick )?survey|\bNPS\b|on a scale of|\[1\] .*\[2\]|thumbs (up|down)\?)",
    )
});
pub static QUOTA: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"(?i)(quota (exceeded|exhausted)|rate.?limit(ed)?|RESOURCE_EXHAUSTED|\b429\b|out of credits|usage limit)",
    )
});
pub static APPROVAL: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"(?i)(Allow (this|once)|Do you want to (proceed|run|allow)|\(y/n\)|\[Y/n\]|Approve\?|Press enter to (confirm|continue))",
    )
});
pub static BUSY_RE: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)esc to (cancel|interrupt)"));
const CLAUDE_STATUS_PREFIX: &str = r"[ \t]*[A-Za-z]+(?:[ \t]+[A-Za-z]+){0,2}[ \t]+for[ \t]+[0-9]+(?:\.[0-9]+)?[hms](?:[ \t]*[0-9]+(?:\.[0-9]+)?[hms])*[ \t]*·[ \t]*done[ \t]+[0-9]{1,2}:[0-9]{2}(?:[ \t]*[AP]M)?";
/// `BACKGROUND_STATUS_RE.fullmatch`.
pub static BACKGROUND_STATUS_RE: LazyLock<Regex> = LazyLock::new(|| {
    re(&format!(
        r"(?i)\A(?:{CLAUDE_STATUS_PREFIX}(?:[ \t]*·[^\n]*)?[ \t]*)\z"
    ))
});
/// `BACKGROUND_BUSY_RE.fullmatch`.
pub static BACKGROUND_BUSY_RE: LazyLock<Regex> = LazyLock::new(|| {
    re(&format!(
        r"(?i)\A(?:{CLAUDE_STATUS_PREFIX}[ \t]*·[ \t]*[1-9][0-9]*[ \t]+(?:monitors?|background tasks?|shells?)[ \t]+(?:still[ \t]+)?running[ \t]*)\z"
    ))
});
static ZERO_FOOTER: LazyLock<Regex> = LazyLock::new(|| {
    re(r"(?i)\bauto mode on\b[^\n]*·[ \t]*0[ \t]+(?:monitors?|background tasks?|shells?)\b")
});
static FENCE: LazyLock<Regex> = LazyLock::new(|| re(r"\A(`{3,}|~{3,})"));
static QUEUED: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)queued messages"));
static PROMPT_LINE: LazyLock<Regex> = LazyLock::new(|| re(r"\A\s*[>›❯]"));
static PASTED: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)\[Pasted (?:Content|text)\b"));
static SCREEN_LS: LazyLock<Regex> = LazyLock::new(|| re(r"(?m)^\s*(\d+)\.(\S+)\s"));

/// Characters that the look-behind `(?<![A-Za-z0-9])` rejects under `re.I`
/// (ASCII alphanumerics plus the non-ASCII letters that case-fold into them).
fn lookbehind_alnum(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '\u{17f}' | '\u{212a}' | '\u{130}' | '\u{131}')
}

/// End positions after `\s*[=:]\s*\w+` starting at `p` (empty when it cannot match).
fn assignment_ends(cs: &[char], p: usize) -> std::ops::Range<usize> {
    let mut k = p;
    while k < cs.len() && py::py_isspace(cs[k]) {
        k += 1;
    }
    if k >= cs.len() || !matches!(cs[k], '=' | ':') {
        return 0..0;
    }
    let mut m = k + 1;
    while m < cs.len() && py::py_isspace(cs[m]) {
        m += 1;
    }
    let mut e = m;
    while e < cs.len() && py::py_isword(cs[e]) {
        e += 1;
    }
    if e == m { 0..0 } else { (m + 1)..(e + 1) }
}

/// The SURVEY alternative
/// `(?<![A-Za-z0-9])1\s*[=:]\s*\w+[^\n]{0,40}(?<![A-Za-z0-9])2\s*[=:]\s*\w+`
/// re-implemented without look-behind (search semantics: any match anywhere).
pub fn survey_scale(text: &str) -> bool {
    let cs: Vec<char> = text.chars().collect();
    for i in 0..cs.len() {
        if cs[i] != '1' || (i > 0 && lookbehind_alnum(cs[i - 1])) {
            continue;
        }
        for a in assignment_ends(&cs, i + 1) {
            let mut j = a;
            while j <= a + 40 && j < cs.len() {
                if cs[j] == '2'
                    && (j == 0 || !lookbehind_alnum(cs[j - 1]))
                    && !assignment_ends(&cs, j + 1).is_empty()
                {
                    return true;
                }
                if cs[j] == '\n' {
                    break;
                }
                j += 1;
            }
        }
    }
    false
}

/// `SURVEY.search(text)`.
pub fn survey(text: &str) -> bool {
    SURVEY_BASE.is_match(text) || survey_scale(text)
}

/// The `screen_state()` dict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenState {
    pub busy: bool,
    pub queued_msgs: bool,
    pub survey: bool,
    pub quota: bool,
    pub approval_prompt: bool,
    pub typed_input: bool,
    pub composer_state: &'static str,
    pub idle_prompt: bool,
    pub hash: String,
}

impl ScreenState {
    /// `{k: v for k, v in s.items() if k != 'hash'}`.
    pub fn to_json_without_hash(&self) -> J {
        crate::jobj! {
            "busy" => self.busy,
            "queued_msgs" => self.queued_msgs,
            "survey" => self.survey,
            "quota" => self.quota,
            "approval_prompt" => self.approval_prompt,
            "typed_input" => self.typed_input,
            "composer_state" => self.composer_state,
            "idle_prompt" => self.idle_prompt,
        }
    }
}

fn last_lines(text: &str, n: usize) -> Vec<&str> {
    let lines: Vec<&str> = text.trim_end_matches('\n').split('\n').collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].to_vec()
}

/// `screen_state(text)`.
pub fn screen_state(text: &str) -> ScreenState {
    let tail = last_lines(text, 25).join("\n");
    let busy = tail_busy(text);
    let queued_msgs = QUEUED.is_match(&tail);
    let lines: Vec<&str> = tail.split('\n').collect();
    let prompt = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| PROMPT_LINE.is_match(l))
        .map(|(i, l)| (i, py::py_strip(l)))
        .next_back();
    let blank = |s: &str| matches!(s, ">" | "›" | "❯");
    let typed_input = prompt.is_some_and(|(_, p)| !blank(p)) || PASTED.is_match(&tail);
    let continuation: Vec<&str> = match prompt {
        Some((i, _)) => lines[i + 1..]
            .iter()
            .map(|l| py::py_strip(l))
            .filter(|l| !l.is_empty())
            .collect(),
        None => Vec::new(),
    };
    let empty_hint = prompt.is_some_and(|(_, p)| blank(p)) && continuation == ["? for shortcuts"];
    let composer_state = if typed_input {
        "draft"
    } else if empty_hint {
        "empty_hint"
    } else {
        "unknown"
    };
    let idle_prompt = !busy && !queued_msgs && composer_state == "empty_hint";
    let hash = sha256_hex(tail.as_bytes())[..16].to_string();
    ScreenState {
        busy,
        queued_msgs,
        survey: survey(&tail),
        quota: QUOTA.is_match(&tail),
        approval_prompt: APPROVAL.is_match(&tail),
        typed_input,
        composer_state,
        idle_prompt,
        hash,
    }
}

/// `tail_busy(text)`: current foreground/background activity from the newest status.
pub fn tail_busy(text: &str) -> bool {
    let raw_lines: Vec<&str> = text.trim_end_matches('\n').split('\n').collect();
    let nonblank: Vec<usize> = raw_lines
        .iter()
        .enumerate()
        .filter(|(_, l)| !py::py_strip(l).is_empty())
        .map(|(i, _)| i)
        .collect();
    let recent = &nonblank[nonblank.len().saturating_sub(12)..];
    let recent_start = recent.first().copied().unwrap_or(raw_lines.len());
    let mut fence: Option<char> = None;
    let mut visible = Vec::new();
    for (index, line) in raw_lines.iter().enumerate() {
        let stripped = py::py_lstrip(line);
        if let Some(m) = FENCE.captures(stripped) {
            let kind = m
                .get(1)
                .and_then(|g| g.as_str().chars().next())
                .unwrap_or('`');
            match fence {
                None => fence = Some(kind),
                Some(f) if f == kind => fence = None,
                Some(_) => {}
            }
            continue;
        }
        if index >= recent_start
            && fence.is_none()
            && !py::py_strip(line).is_empty()
            && !stripped.starts_with(['>', '`', '"', '\''])
        {
            visible.push(*line);
        }
    }
    for line in visible.iter().rev() {
        if ZERO_FOOTER.is_match(line) {
            return false;
        }
        if BACKGROUND_STATUS_RE.is_match(line) {
            return BACKGROUND_BUSY_RE.is_match(line);
        }
        if BUSY_RE.is_match(line) {
            return true;
        }
    }
    false
}

/// `tail_hash(text)`.
pub fn tail_hash(text: &str) -> String {
    let lines: Vec<&str> = text
        .trim_end_matches('\n')
        .split('\n')
        .filter(|l| !py::py_strip(l).is_empty())
        .map(py::py_rstrip)
        .collect();
    let start = lines.len().saturating_sub(25);
    sha256_hex(lines[start..].join("\n").as_bytes())[..16].to_string()
}

/// Parse `screen -ls` output.
pub fn parse_screen_ls(out: &str) -> Screens {
    let mut d = Screens::new();
    for m in SCREEN_LS.captures_iter(out) {
        if let (Some(pid), Some(name)) = (m.get(1).and_then(|g| py::py_int(g.as_str())), m.get(2)) {
            d.insert(name.as_str().to_string(), pid);
        }
    }
    d
}

/// `ssh -n -o BatchMode=yes -o ConnectTimeout=8 <login> <cmd>` (stdin detached via `-n` too).
pub fn ssh_argv<'b>(cmd: &'b str, login: &'b str) -> [&'b str; 8] {
    [
        "ssh",
        "-n",
        "-o",
        "BatchMode=yes",
        "-o",
        "ConnectTimeout=8",
        login,
        cmd,
    ]
}

impl Audit<'_> {
    /// `screens()`.
    pub fn screens(&self) -> Screens {
        let out = self.run(&["screen", "-ls"], 10);
        parse_screen_ls(&out.stdout)
    }

    /// `hardcopy(name, path)`.
    pub fn hardcopy(&self, name: &str, path: &str) -> String {
        self.host.hardcopy(name, path)
    }

    /// `descendants(pid)` in Python's DFS order.
    pub fn descendants(&self, pid: i64) -> Vec<i64> {
        let out = self.run(&["ps", "-e", "-o", "pid=,ppid="], 10);
        let mut kids: HashMap<i64, Vec<i64>> = HashMap::new();
        for line in out.stdout.split('\n') {
            let p = py::py_split_ws(line);
            if p.len() == 2
                && let (Some(child), Some(parent)) = (py::py_int(p[0]), py::py_int(p[1]))
            {
                kids.entry(parent).or_default().push(child);
            }
        }
        let mut res = Vec::new();
        let mut stack = vec![pid];
        while let Some(x) = stack.pop() {
            if let Some(ks) = kids.get(&x) {
                for k in ks {
                    res.push(*k);
                    stack.push(*k);
                }
            }
        }
        res
    }

    /// `pty_last_input(pid)`: atime of the first child's pty slave.
    pub fn pty_last_input(&self, pid: i64) -> Option<Dt> {
        let first = self.descendants(pid).into_iter().next()?;
        let tty = self.host.read_link(&format!("/proc/{first}/fd/0"))?;
        if !tty.starts_with("/dev/pts/") {
            return None;
        }
        py::utc_from_timestamp(self.host.atime(&tty)?)
    }

    /// `remote_screens(host)` (cached per run).
    pub fn remote_screens(&mut self, host: &str) -> Option<BTreeMap<String, i64>> {
        if let Some(cached) = self.remote_cache.get(host) {
            return cached.clone();
        }
        let login = format!("{}@{host}", self.cfg.ssh_user);
        let out = self.host.run(
            &ssh_argv("screen -ls", &login),
            Some(std::time::Duration::from_secs(20)),
            None,
        );
        let value = if !matches!(out.rc, 0 | 1)
            || (!out.stdout.contains("Socket") && !out.stdout.contains("No Sockets"))
        {
            None
        } else {
            Some(parse_screen_ls(&out.stdout))
        };
        self.remote_cache.insert(host.to_string(), value.clone());
        value
    }

    /// `remote_hash(host, name)`.
    pub fn remote_hash(&self, host: &str, name: &str) -> Option<String> {
        if host == self.cfg.forbidden_host {
            return None;
        }
        let cmd = format!(
            "f=/tmp/sc_audit_hc_{name}.txt; rm -f $f; screen -S {name} -X hardcopy $f; sleep 0.3; tail -n 25 $f | sha256sum | cut -c1-16"
        );
        let login = format!("{}@{host}", self.cfg.ssh_user);
        let out = self.host.run(
            &ssh_argv(&cmd, &login),
            Some(std::time::Duration::from_secs(20)),
            None,
        );
        let s = py::py_strip(&out.stdout);
        if s.is_empty() {
            None
        } else {
            Some(s.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn survey_scale_without_lookbehind() {
        assert!(survey_scale(" 1: Bad   2: Fine   3: Good"));
        assert!(survey_scale(
            "Rate this session  1 = poor  2 = ok  3 = great"
        ));
        assert!(!survey_scale("x1 = 5; y2 = 6"));
        assert!(!survey_scale(
            "P1 = JopeDime GPU (all 3 GPUs at 0%) + Admiralty; re-drive"
        ));
        assert!(!survey_scale("1 = a\n2 = b"));
        assert!(survey_scale("1=a 2=b"));
        assert!(!survey_scale("1=a x2=b"));
        assert!(!survey_scale("(1) = yes, (2) = no"));
        assert!(survey_scale("(1 = yes, (2 = no"));
        assert!(!survey_scale(&format!("1=a{}2=b", "x ".repeat(25))));
    }

    #[test]
    fn screen_ls_parsing() {
        let out = "There are screens on:\n\t1234.captain_joil\t(Detached)\n\t99.super_captain\t(Attached)\n2 Sockets in /run/screen.\n";
        let s = parse_screen_ls(out);
        assert_eq!(s.get("captain_joil"), Some(&1234));
        assert_eq!(s.get("super_captain"), Some(&99));
    }
}
