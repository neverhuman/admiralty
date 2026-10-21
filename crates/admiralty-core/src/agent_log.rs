//! Compact, deterministic operational log lines for agents and operators.
//!
//! One event per line with a stable dotted event name and short lowercase keys.
//! `logfmt` (the default) is readable in a terminal and costs far fewer tokens
//! than prose or pretty JSON; `json` emits the same fields as NDJSON for tools.
//!
//! [`Rollup`] collapses an unchanged condition: the first occurrence and every
//! change are always written; identical repeats inside the window are counted
//! and reported with `repeat=N`, never silently discarded. Volatile fields such
//! as durations are excluded from the identity so a persistent failure does not
//! produce one line per observation cycle.
//!
//! Formatting is pure. Callers own the clock and the output stream.
use serde_json::{Map, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Debug,
    Info,
    Warn,
    Error,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Debug => "debug",
            Level::Info => "info",
            Level::Warn => "warn",
            Level::Error => "error",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Format {
    #[default]
    Logfmt,
    Json,
}

impl std::str::FromStr for Format {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "logfmt" | "text" => Ok(Format::Logfmt),
            "json" | "ndjson" => Ok(Format::Json),
            other => Err(format!("unknown log format {other:?}; use logfmt or json")),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub ts: String,
    pub level: Level,
    pub event: String,
    pub fields: Vec<(String, Value)>,
}

/// Longest rendered string value; longer text is cut with an explicit marker.
pub const MAX_VALUE_CHARS: usize = 240;

fn bounded(text: &str) -> String {
    let mut chars = text.chars();
    let head = chars.by_ref().take(MAX_VALUE_CHARS).collect::<String>();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

fn quote(text: &str) -> String {
    let text = bounded(text);
    let plain = !text.is_empty()
        && text
            .chars()
            .all(|c| !c.is_whitespace() && !c.is_control() && !matches!(c, '"' | '=' | '\\'));
    if plain {
        return text;
    }
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn logfmt_value(value: &Value) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => quote(s),
        Value::Array(items) if items.iter().all(|v| !v.is_array() && !v.is_object()) => {
            let joined = items
                .iter()
                .map(|v| match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .collect::<Vec<_>>()
                .join(",");
            if items.is_empty() {
                "[]".into()
            } else {
                quote(&joined)
            }
        }
        other => quote(&other.to_string()),
    }
}

impl Event {
    pub fn new(ts: impl Into<String>, level: Level, event: impl Into<String>) -> Self {
        Event {
            ts: ts.into(),
            level,
            event: event.into(),
            fields: Vec::new(),
        }
    }

    /// Append a field. Keys are kept in insertion order for stable output.
    pub fn field(mut self, key: impl Into<String>, value: impl Into<Value>) -> Self {
        self.fields.push((key.into(), value.into()));
        self
    }

    pub fn render(&self, format: Format) -> String {
        match format {
            Format::Logfmt => {
                let mut line =
                    format!("{} {} {}", self.ts, self.level.as_str(), quote(&self.event));
                for (key, value) in &self.fields {
                    line.push(' ');
                    line.push_str(&quote(key));
                    line.push('=');
                    line.push_str(&logfmt_value(value));
                }
                line
            }
            Format::Json => {
                let mut object = Map::new();
                object.insert("ts".into(), Value::String(self.ts.clone()));
                object.insert("lvl".into(), Value::String(self.level.as_str().into()));
                object.insert("ev".into(), Value::String(self.event.clone()));
                for (key, value) in &self.fields {
                    let value = match value {
                        Value::String(s) => Value::String(bounded(s)),
                        other => other.clone(),
                    };
                    object.insert(key.clone(), value);
                }
                Value::Object(object).to_string()
            }
        }
    }

    fn identity(&self, volatile: &[&str]) -> String {
        let mut identity = format!("{}\u{1f}{}", self.level.as_str(), self.event);
        for (key, value) in &self.fields {
            if !volatile.contains(&key.as_str()) {
                identity.push('\u{1f}');
                identity.push_str(key);
                identity.push('=');
                identity.push_str(&value.to_string());
            }
        }
        identity
    }
}

/// Collapse consecutive identical events. Not a rate limiter for distinct
/// events: any change in level, event name or non-volatile field is written.
#[derive(Clone, Debug)]
pub struct Rollup {
    window_s: i64,
    volatile: Vec<String>,
    last: Option<Pending>,
}

#[derive(Clone, Debug)]
struct Pending {
    identity: String,
    event: Event,
    emitted_at: i64,
    first_repeat_ts: Option<String>,
    repeats: u64,
}

impl Rollup {
    pub fn new(window_s: i64, volatile: &[&str]) -> Self {
        Rollup {
            window_s: window_s.max(0),
            volatile: volatile.iter().map(|s| (*s).to_owned()).collect(),
            last: None,
        }
    }

    /// Return the lines to write for `event` observed at `now_s` (seconds).
    pub fn admit(&mut self, event: Event, now_s: i64) -> Vec<Event> {
        let volatile = self.volatile.iter().map(String::as_str).collect::<Vec<_>>();
        let identity = event.identity(&volatile);
        let mut out = Vec::new();
        if let Some(last) = self.last.as_mut()
            && last.identity == identity
        {
            last.repeats += 1;
            if last.first_repeat_ts.is_none() {
                last.first_repeat_ts = Some(event.ts.clone());
            }
            if now_s.saturating_sub(last.emitted_at) >= self.window_s {
                let mut summary = event.clone();
                summary.fields.push(("repeat".into(), last.repeats.into()));
                if let Some(since) = last.first_repeat_ts.take() {
                    summary.fields.push(("since".into(), since.into()));
                }
                last.repeats = 0;
                last.emitted_at = now_s;
                last.event = event;
                out.push(summary);
            } else {
                last.event = event;
            }
            return out;
        }
        if let Some(previous) = self.flush() {
            out.push(previous);
        }
        out.push(event.clone());
        self.last = Some(Pending {
            identity,
            event,
            emitted_at: now_s,
            first_repeat_ts: None,
            repeats: 0,
        });
        out
    }

    /// Report repeats still held by the rollup (for example at shutdown).
    pub fn flush(&mut self) -> Option<Event> {
        let last = self.last.as_mut()?;
        if last.repeats == 0 {
            return None;
        }
        let mut summary = last.event.clone();
        summary.fields.push(("repeat".into(), last.repeats.into()));
        if let Some(since) = last.first_repeat_ts.take() {
            summary.fields.push(("since".into(), since.into()));
        }
        last.repeats = 0;
        Some(summary)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn event(ts: &str, ms: u64, error: &str) -> Event {
        Event::new(ts, Level::Error, "observe.cycle_failed")
            .field("ms", ms)
            .field("err", error)
    }

    #[test]
    fn logfmt_quotes_only_when_needed_and_bounds_values() {
        let line = Event::new("2026-10-21T00:00:00Z", Level::Info, "observe.cycle")
            .field("ms", 1834)
            .field("projects", json!(["joil", "dope"]))
            .field("note", "two words \"quoted\"")
            .field("empty", "")
            .field("unknown", Value::Null)
            .field("long", "x".repeat(400))
            .render(Format::Logfmt);
        assert!(
            line.starts_with("2026-10-21T00:00:00Z info observe.cycle ms=1834 projects=joil,dope ")
        );
        assert!(line.contains(r#"note="two words \"quoted\"""#));
        assert!(line.contains(r#"empty="""#));
        assert!(line.contains("unknown=null"));
        assert!(line.contains(&format!("long={}…", "x".repeat(MAX_VALUE_CHARS))));
        assert!(!line.contains('\n'));
    }

    #[test]
    fn control_characters_never_split_a_log_line() {
        let line = Event::new("t", Level::Warn, "x")
            .field("err", "line one\nline two\u{7}")
            .render(Format::Logfmt);
        assert_eq!(line, r#"t warn x err="line one\nline two\u{7}""#);
        let json = Event::new("t", Level::Warn, "x")
            .field("err", "a\nb")
            .render(Format::Json);
        let parsed: Value = serde_json::from_str(&json).expect("ndjson");
        assert_eq!(parsed["err"], "a\nb");
        assert_eq!(parsed["lvl"], "warn");
        assert!(!json.contains('\n'));
    }

    #[test]
    fn rollup_writes_first_and_changes_and_counts_identical_repeats() {
        let mut rollup = Rollup::new(600, &["ms"]);
        assert_eq!(rollup.admit(event("t0", 10, "disk"), 0).len(), 1);
        // Same failure with a different duration is a repeat, not a new line.
        assert!(rollup.admit(event("t1", 20, "disk"), 15).is_empty());
        assert!(rollup.admit(event("t2", 30, "disk"), 30).is_empty());
        // Window elapsed: one summary line carries the exact suppressed count.
        let out = rollup.admit(event("t3", 40, "disk"), 615);
        assert_eq!(out.len(), 1);
        let line = out[0].render(Format::Logfmt);
        assert!(line.contains("repeat=3"), "{line}");
        assert!(line.contains("since=t1"), "{line}");
        // A change flushes pending repeats before the new event.
        assert!(rollup.admit(event("t4", 50, "disk"), 630).is_empty());
        let out = rollup.admit(event("t5", 60, "network"), 645);
        assert_eq!(out.len(), 2);
        assert!(out[0].render(Format::Logfmt).contains("repeat=1"));
        assert!(out[1].render(Format::Logfmt).contains("err=network"));
        assert!(rollup.flush().is_none());
    }

    #[test]
    fn level_change_is_never_collapsed() {
        let mut rollup = Rollup::new(600, &[]);
        rollup.admit(Event::new("a", Level::Info, "observe.cycle"), 0);
        let out = rollup.admit(Event::new("b", Level::Warn, "observe.cycle"), 1);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].level, Level::Warn);
    }

    #[test]
    fn format_parses_known_names_only() {
        assert_eq!("json".parse::<Format>(), Ok(Format::Json));
        assert_eq!("logfmt".parse::<Format>(), Ok(Format::Logfmt));
        assert!("yaml".parse::<Format>().is_err());
    }
}
