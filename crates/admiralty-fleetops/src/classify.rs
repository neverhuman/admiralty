//! Port of `tools/bin/classify_screen.py`: classify a screen hardcopy into
//! agent kind (grok/codex/claude/unknown) and state (needs_approval, error,
//! needs_input, pr_blocked, working, idle, unknown).
//!
//! Every Python regex is re-used verbatim (none needs look-around), and the
//! floating point accumulation order is preserved so `confidence` values are
//! bit-identical.

use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::compat::{self, JsonLayout};

/// JSON document printed by `fleet classify` (same keys and order as the
/// Python script).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Classification {
    pub session: String,
    pub kind: String,
    pub state: String,
    pub confidence: f64,
    pub signals: Vec<String>,
    pub approval_hint: Option<String>,
    pub pr_refs: Vec<String>,
    pub summary: String,
    pub tail: String,
}

impl Classification {
    /// `json.dump(out, ensure_ascii=False, indent=2)` (no trailing newline).
    pub fn to_pretty_json(&self) -> std::io::Result<String> {
        compat::py_json(self, JsonLayout::Indent(2), false)
    }
}

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("static classifier regex compiles")
}

fn re_i(pattern: &str) -> Regex {
    re(&format!("(?i:{pattern})"))
}

static SCREEN_TAG: LazyLock<Regex> =
    LazyLock::new(|| re(r"\[[\s\x1c-\x1f]*SCREEN:[\s\x1c-\x1f]*[^\]]+\]"));
static CONTROL: LazyLock<Regex> = LazyLock::new(|| re(r"[\x00-\x08\x0b\x0c\x0e-\x1f]"));

/// `clean_lines`.
pub fn clean_lines(text: &str) -> Vec<String> {
    compat::splitlines(text)
        .into_iter()
        .map(|raw| {
            let line = raw.replace('\0', "");
            let line = SCREEN_TAG.replace_all(&line, "");
            let line = CONTROL.replace_all(&line, "");
            compat::py_rstrip(&line).to_string()
        })
        .collect()
}

static GROK_BANNER: LazyLock<Regex> = LazyLock::new(|| re(r"Grok\s+\d"));
static GPT_BANNER: LazyLock<Regex> = LazyLock::new(|| re(r"GPT-\d"));
static CODEX_QUEUED: LazyLock<Regex> = LazyLock::new(|| re(r"\? \d+ question"));
static AGENTS_DONE: LazyLock<Regex> = LazyLock::new(|| re(r"\d+/\d+ agents done"));

/// `detect_kind`.
pub fn detect_kind(text: &str, session: &str) -> (String, f64, Vec<String>) {
    let mut signals = Vec::new();
    let low = text.to_lowercase();
    let session_low = session.to_lowercase();
    let (mut grok, mut codex, mut claude) = (0.0f64, 0.0f64, 0.0f64);

    if GROK_BANNER.is_match(text) || low.contains("always-approve") {
        grok += 0.6;
        signals.push("grok_banner".to_string());
    }
    if low.contains("plan approval") && low.contains("a:approve") {
        grok += 0.2;
        signals.push("grok_plan_approval_ui".to_string());
    }
    if !session.is_empty() && session_low.contains("grok") {
        grok += 0.35;
    }

    if low.contains("ask codex to do anything") || GPT_BANNER.is_match(text) {
        codex += 0.7;
        signals.push("codex_prompt".to_string());
    }
    if CODEX_QUEUED.is_match(text) || text.contains("alt+↑") {
        codex += 0.25;
        signals.push("codex_queued_q".to_string());
    }
    if low.contains("yolo") {
        codex += 0.1;
        signals.push("yolo_mentioned".to_string());
    }
    if !session.is_empty() && session_low.contains("codex") {
        codex += 0.35;
    }

    if low.contains("bypass permissions") {
        claude += 0.55;
        signals.push("claude_bypass".to_string());
    }
    if AGENTS_DONE.is_match(text) || text.contains("/workflows") || low.contains("dynamic workflow")
    {
        claude += 0.3;
        signals.push("claude_workflow_ui".to_string());
    }
    if low.contains("keep going, ping codex") {
        claude += 0.2;
        signals.push("claude_ping_codex_hint".to_string());
    }
    if !session.is_empty() && session_low.contains("claude") {
        claude += 0.35;
    }

    // `max(scores, key=scores.get)`: first maximum in insertion order wins.
    let mut kind = "grok";
    let mut conf = grok;
    if codex > conf {
        kind = "codex";
        conf = codex;
    }
    if claude > conf {
        kind = "claude";
        conf = claude;
    }
    if conf < 0.35 {
        return ("unknown".to_string(), 0.2, signals);
    }
    (kind.to_string(), conf.min(1.0), signals)
}

static PR_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"https?://github\.com/[^\s]+/pull/\d+",
        r"\bpull request\s+#?\d+\b",
        r"\bPR\s*#?\d+\b",
    ]
    .iter()
    .map(|p| re_i(p))
    .collect()
});

/// `extract_pr_refs`.
pub fn extract_pr_refs(text: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for pat in PR_PATTERNS.iter() {
        for m in pat.find_iter(text) {
            let r = m
                .as_str()
                .trim_end_matches(['.', ',', ')', ';', ']'])
                .to_string();
            if !found.contains(&r) {
                found.push(r);
            }
        }
    }
    found.truncate(8);
    found
}

fn nonempty(lines: &[String]) -> Vec<&str> {
    lines
        .iter()
        .map(String::as_str)
        .filter(|l| !compat::py_strip(l).is_empty())
        .collect()
}

fn last_n<'a>(v: &'a [&'a str], n: usize) -> &'a [&'a str] {
    &v[v.len().saturating_sub(n)..]
}

const ACTIVE_APPROVAL_CUES: &[&str] = &[
    r"waiting on plan approval",
    r"\ba:approve\b",
    r"allow once",
    r"allow always",
    r"always allow",
    r"do you want to",
    r"run this command",
    r"permission required",
    r"approve this",
    r"\(y/n\)",
    r"press enter to confirm",
    r"\b1\.\s*allow once\b",
    r"\b2\.\s*allow always\b",
];
const NEAR_APPROVAL_CUES: &[&str] = &[
    r"waiting on plan approval",
    r"\ba:approve\b",
    r"allow once",
    r"\(y/n\)",
];
const CURRENT_PR_CUES: &[&str] = &[
    r"ci (is )?queued",
    r"hosted run \d+ is queued",
    r"ci (is )?fail",
    r"merge blocked",
    r"review requested",
    r"waiting for (the )?pr\b",
    r"waiting (on|for).*ci\b",
    r"watch pr \d+ until ci",
    r"1 monitor still running",
];

static ACTIVE_APPROVAL_RE: LazyLock<Vec<Regex>> =
    LazyLock::new(|| ACTIVE_APPROVAL_CUES.iter().map(|p| re_i(p)).collect());
static NEAR_APPROVAL_RE: LazyLock<Vec<Regex>> =
    LazyLock::new(|| NEAR_APPROVAL_CUES.iter().map(|p| re_i(p)).collect());
static CURRENT_PR_RE: LazyLock<Vec<Regex>> =
    LazyLock::new(|| CURRENT_PR_CUES.iter().map(|p| re_i(p)).collect());
static MODE_BANNER: LazyLock<Regex> = LazyLock::new(|| re(r"always-approve|bypass permissions on"));
static HINT_A: LazyLock<Regex> = LazyLock::new(|| re(r"a:approve|waiting on plan approval"));
static HINT_2: LazyLock<Regex> = LazyLock::new(|| re(r"allow always|2\.\s*allow"));
static HINT_ALWAYS: LazyLock<Regex> = LazyLock::new(|| re(r"always allow|allow always"));
static WORKING_I: LazyLock<Regex> = LazyLock::new(|| re_i(r"working\s*\(\d"));
static ACTIVE_LAST15: LazyLock<Regex> = LazyLock::new(|| {
    re(concat!(
        r"task started|monitor still running|\d+ monitor|",
        r"waiting for \d+ dynamic workflow|running in background|",
        r"\d+/\d+ agents done|",
        r"background terminal running"
    ))
});
static ACTIVE_TAIL: LazyLock<Regex> = LazyLock::new(|| {
    re(concat!(
        r"task started|monitor still running|working\s*\(\d|",
        r"waiting for \d+ dynamic workflow|running in background"
    ))
});
static WAITING_INPUT: LazyLock<Regex> = LazyLock::new(|| {
    re(r"waiting for (your )?input|waiting for human|human approval|awaiting (your )?response")
});
static HIST_PR: LazyLock<Regex> = LazyLock::new(|| {
    re(r"pending handoff|checkout ownership|blocked by the benchmark|applying it and completing ci")
});
static IDLE_AFTER_HANDOFF: LazyLock<Regex> =
    LazyLock::new(|| re(r"ask codex to do anything|build anything"));
static GRANT_NEAR: LazyLock<Regex> =
    LazyLock::new(|| re(r"waiting on (a )?cpu grant|still no grant from|ping codex again"));
static GRANT_LAST: LazyLock<Regex> = LazyLock::new(|| re(r"keep going, ping codex"));
static FATAL: LazyLock<Regex> = LazyLock::new(|| re(r"\bfatal error\b|\bpanic!\b"));
static IDLE_PROMPT_CODEX: LazyLock<Regex> = LazyLock::new(|| re(r": ask codex to do anything"));
static IDLE_PROMPT_BUILD: LazyLock<Regex> = LazyLock::new(|| re(r"build anything"));
static FRESH_BUILD: LazyLock<Regex> = LazyLock::new(|| re(r"\bbuild anything\b"));
// `r"\\d+/\\d+ agents done"` in the Python is a literal backslash + 'd'
// (a latent bug kept for parity).
static FRESH_BUSY: LazyLock<Regex> = LazyLock::new(|| {
    re(concat!(
        r"working\s*\(\d|waiting for response|monitor still running|",
        r"\\d+/\\d+ agents done|pouncing|esc to interrupt"
    ))
});
static GROK_FOOTER: LazyLock<Regex> = LazyLock::new(|| re(r"always-approve"));
static CI_WATCH: LazyLock<Regex> = LazyLock::new(|| {
    re(r"ci (is )?queued|watch pr|hosted run \d+ is queued|monitor still running")
});

/// Result of `classify_state`.
#[derive(Debug, Clone, PartialEq)]
pub struct StateResult {
    pub state: &'static str,
    pub confidence: f64,
    pub signals: Vec<String>,
    pub approval_hint: Option<&'static str>,
}

/// `classify_state`.
pub fn classify_state(text: &str, kind: &str, lines: &[String]) -> StateResult {
    let mut signals: Vec<String> = Vec::new();
    let low = text.to_lowercase();
    let ne = nonempty(lines);
    let tail60 = last_n(&ne, 60);
    let tail_low = tail60.join("\n").to_lowercase();
    let last15 = last_n(&ne, 15).join("\n");
    let last15_low = last15.to_lowercase();
    let near25 = last_n(&ne, 25).join("\n").to_lowercase();

    let mut approval_hint: Option<&'static str> = None;

    let mut needs_approval = false;
    for (pat, rx) in ACTIVE_APPROVAL_CUES.iter().zip(ACTIVE_APPROVAL_RE.iter()) {
        if rx.is_match(&last15_low) {
            needs_approval = true;
            signals.push(format!("approval:{pat}"));
            break;
        }
    }
    if !needs_approval {
        for (pat, rx) in NEAR_APPROVAL_CUES.iter().zip(NEAR_APPROVAL_RE.iter()) {
            if rx.is_match(&tail_low) && !MODE_BANNER.is_match(&last15_low) && rx.is_match(&near25)
            {
                needs_approval = true;
                signals.push(format!("approval_near:{pat}"));
                break;
            }
        }
    }

    if needs_approval {
        approval_hint = Some(if kind == "grok" || HINT_A.is_match(&last15_low) {
            "a"
        } else if kind == "claude" || HINT_2.is_match(&last15_low) {
            "2"
        } else if HINT_ALWAYS.is_match(&last15_low) {
            "always"
        } else {
            "y"
        });
    }

    let mut actively_working = WORKING_I.is_match(text)
        || ACTIVE_LAST15.is_match(&last15_low)
        || ACTIVE_TAIL.is_match(&tail_low);
    if actively_working {
        signals.push("actively_working".to_string());
    }

    let mut needs_input = false;
    if CODEX_QUEUED.is_match(&last15) {
        needs_input = true;
        signals.push("queued_question".to_string());
    }
    if WAITING_INPUT.is_match(&last15_low) {
        needs_input = true;
        signals.push("waiting_for_input".to_string());
    }

    let mut pr_blocked = false;
    for (pat, rx) in CURRENT_PR_CUES.iter().zip(CURRENT_PR_RE.iter()) {
        if rx.is_match(&near25) {
            pr_blocked = true;
            signals.push(format!("pr:{pat}"));
            break;
        }
    }

    if HIST_PR.is_match(&low) {
        signals.push("pr_history_mention".to_string());
        if !actively_working && !pr_blocked && IDLE_AFTER_HANDOFF.is_match(&last15_low) {
            pr_blocked = true;
            signals.push("pr:idle_after_handoff_block".to_string());
        }
    }

    let waiting_grant = GRANT_NEAR.is_match(&near25) || GRANT_LAST.is_match(&last15_low);
    if waiting_grant {
        signals.push("waiting_codex_grant".to_string());
    }

    let is_error = FATAL.is_match(&last15_low) && !actively_working;

    let idle_prompt =
        IDLE_PROMPT_CODEX.is_match(&last15_low) || IDLE_PROMPT_BUILD.is_match(&last15_low);
    let fresh_idle_chrome =
        FRESH_BUILD.is_match(&last15_low) || IDLE_PROMPT_CODEX.is_match(&last15_low);
    let fresh_busy = FRESH_BUSY.is_match(&last15_low);
    if fresh_idle_chrome && !fresh_busy && !needs_approval {
        actively_working = false;
        signals.retain(|s| s != "actively_working");
        signals.push("fresh_idle_chrome".to_string());
    }

    let grok_idle = kind == "grok"
        && GROK_FOOTER.is_match(&last15_low)
        && !actively_working
        && !needs_approval
        && !pr_blocked;

    let done = |state: &'static str, confidence: f64, signals: Vec<String>| StateResult {
        state,
        confidence,
        signals,
        approval_hint,
    };

    if needs_approval {
        return done("needs_approval", 0.88, signals);
    }
    if is_error {
        return done("error", 0.6, signals);
    }
    if needs_input && !actively_working {
        return done("needs_input", 0.8, signals);
    }
    if pr_blocked {
        if actively_working && WORKING_I.is_match(text) && !CI_WATCH.is_match(&near25) {
            return done("working", 0.82, signals);
        }
        return done("pr_blocked", 0.72, signals);
    }
    if actively_working {
        return done("working", 0.85, signals);
    }
    if waiting_grant && !actively_working {
        return done("idle", 0.7, signals);
    }
    if idle_prompt || grok_idle {
        return done("idle", 0.6, signals);
    }
    if waiting_grant {
        return done("idle", 0.65, signals);
    }
    done("unknown", 0.35, signals)
}

static SUMMARY_SKIP: LazyLock<Regex> =
    LazyLock::new(|| re_i(r"Grok \d|Ask Codex|bypass permissions|Shift\+Tab|GPT-\d|ctrl\+[a-z]"));

/// `make_summary`.
pub fn make_summary(kind: &str, state: &str, lines: &[String], pr_refs: &[String]) -> String {
    let stripped: Vec<&str> = lines
        .iter()
        .map(|l| compat::py_strip(l))
        .filter(|l| !l.is_empty())
        .collect();
    let mut interesting: Vec<&str> = Vec::new();
    for ln in last_n(&stripped, 40).iter().rev() {
        if ln.chars().count() < 8 || SUMMARY_SKIP.is_match(ln) {
            continue;
        }
        interesting.push(ln);
        if interesting.len() >= 4 {
            break;
        }
    }
    interesting.reverse();
    let joined = interesting.join(" | ");
    let snippet = compat::take_chars(&joined, 280);
    let pr = if pr_refs.is_empty() {
        "none".to_string()
    } else {
        pr_refs
            .iter()
            .take(3)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ")
    };
    format!("[{kind}/{state}] PRs: {pr}. Recent: {snippet}")
}

/// Classify already-decoded hardcopy text.
pub fn classify_text(raw: &str, session: &str) -> Classification {
    let lines = clean_lines(raw);
    let text = lines.join("\n");
    let (kind, kind_conf, kind_signals) = detect_kind(&text, session);
    let st = classify_state(&text, &kind, &lines);
    let pr_refs = extract_pr_refs(&text);
    let confidence = compat::py_round((0.4 * kind_conf + 0.6 * st.confidence).min(1.0), 3);
    let mut signals = kind_signals;
    signals.extend(st.signals);
    signals.truncate(24);
    let ne = nonempty(&lines);
    let tail = last_n(&ne, 20).join("\n");
    let summary = make_summary(&kind, st.state, &lines, &pr_refs);
    Classification {
        session: session.to_string(),
        kind,
        state: st.state.to_string(),
        confidence,
        signals,
        approval_hint: st.approval_hint.map(str::to_string),
        pr_refs,
        summary,
        tail,
    }
}

/// Python `Path(p).stem`.
pub fn py_stem(path: &Path) -> String {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    match name.rfind('.') {
        Some(i) if i > 0 && i < name.len() - 1 => name[..i].to_string(),
        _ => name,
    }
}

/// Error document for a missing snapshot (`{"error": ..., "session": ...}`).
pub fn missing_snap_json(path: &Path, session: &str) -> String {
    #[derive(Serialize)]
    struct Missing<'a> {
        error: String,
        session: &'a str,
    }
    compat::py_json(
        &Missing {
            error: format!("missing snap: {}", path.display()),
            session,
        },
        JsonLayout::Compact,
        true,
    )
    .unwrap_or_default()
}

/// Classify a snapshot file (`main()` of the Python). `Err` carries the
/// one-line error JSON printed for a missing file (exit 1).
pub fn classify_file(path: &Path, session_arg: &str) -> Result<Classification, String> {
    if !path.exists() {
        return Err(missing_snap_json(path, session_arg));
    }
    let bytes = std::fs::read(path).map_err(|e| {
        format!(
            "{{\"error\": \"cannot read snap: {}: {e}\", \"session\": \"{session_arg}\"}}",
            path.display()
        )
    })?;
    let raw = String::from_utf8_lossy(&bytes);
    let session = if session_arg.is_empty() {
        py_stem(path)
    } else {
        session_arg.to_string()
    };
    Ok(classify_text(&raw, &session))
}

#[cfg(test)]
mod tests;
