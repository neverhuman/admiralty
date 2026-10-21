//! Token-budgeted log digests for agents.
//!
//! A digest answers "what happened, how often, how bad" in a few hundred
//! tokens. Events are grouped by level, target and code; each field is
//! summarized across the group as a single value, a numeric range
//! (`ms=3..812`) or a small set (`route={/a,/b,+3}`). Groups are ordered by
//! severity, then frequency, and rendering stops at the token budget with an
//! explicit omitted count — content is dropped by group, never cut mid-line.
//!
//! The same renderer digests foreign text: `ALOG/1` lines, JSON lines
//! (`lvl`/`level`, `code`/`msg`) and raw lines, which are grouped by a
//! normalized template (digits → `#`, long hex/UUIDs → `<id>`).
use crate::{Field, Level, Record, Value, format};
use std::{
    collections::{BTreeMap, BTreeSet},
    time::{SystemTime, UNIX_EPOCH},
};

/// Tokenizer label shared with Admiralty context capsules.
pub const TOKENIZER: &str = "utf8_bytes_div4_estimate";
/// Smallest budget honored: enough for the header, omitted and footer lines.
pub const MIN_BUDGET_TOKENS: usize = 96;
/// Distinct groups tracked before further new groups are only counted.
pub const MAX_GROUPS: usize = 10_000;
const MAX_SET_SAMPLES: usize = 2;
const MAX_TRACKED_STRINGS: usize = 64;
const MAX_FIELDS_PER_GROUP: usize = 12;

#[derive(Clone, Debug)]
pub struct Options {
    pub budget_tokens: usize,
    /// Ignore records whose last occurrence is older than this.
    pub since: Option<SystemTime>,
    /// Ignore records less severe than this.
    pub min_level: Level,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            budget_tokens: 600,
            since: None,
            min_level: Level::Trace,
        }
    }
}

/// Estimated tokens for text, using the capsule convention (bytes / 4).
pub fn estimate_tokens(text: &str) -> usize {
    text.len().div_ceil(4)
}

#[derive(Default)]
struct FieldSummary {
    seen: u64,
    min: Option<f64>,
    max: Option<f64>,
    numeric_only: bool,
    strings: BTreeSet<String>,
    overflow: bool,
}

impl FieldSummary {
    fn add(&mut self, key: &str, value: &Value) {
        if self.seen == 0 {
            self.numeric_only = true;
        }
        self.seen += 1;
        match value.as_f64() {
            Some(n) => {
                self.min = Some(self.min.map_or(n, |m| m.min(n)));
                self.max = Some(self.max.map_or(n, |m| m.max(n)));
            }
            None => {
                self.numeric_only = false;
                let rendered = value.render(key);
                if self.strings.len() < MAX_TRACKED_STRINGS {
                    self.strings.insert(rendered);
                } else if !self.strings.contains(&rendered) {
                    self.overflow = true;
                }
            }
        }
    }

    fn render(&self, key: &str) -> String {
        if self.numeric_only {
            let (min, max) = (self.min.unwrap_or_default(), self.max.unwrap_or_default());
            return if min == max {
                format!("{key}={}", format::decimal(min))
            } else {
                format!("{key}={}..{}", format::decimal(min), format::decimal(max))
            };
        }
        let mut values = self.strings.iter().cloned().collect::<Vec<_>>();
        if let (Some(min), Some(max)) = (self.min, self.max) {
            values.push(if min == max {
                format::decimal(min)
            } else {
                format!("{}..{}", format::decimal(min), format::decimal(max))
            });
        }
        if values.len() == 1 && !self.overflow {
            return format!("{key}={}", values[0]);
        }
        let extra = values.len().saturating_sub(MAX_SET_SAMPLES);
        let mut shown = values.into_iter().take(MAX_SET_SAMPLES).collect::<Vec<_>>();
        if extra > 0 || self.overflow {
            shown.push(format!(
                "+{}{}",
                extra,
                if self.overflow { "…" } else { "" }
            ));
        }
        format!("{key}={{{}}}", shown.join(","))
    }
}

struct Group {
    level: Level,
    target: String,
    code: String,
    count: u64,
    first: Option<SystemTime>,
    last: Option<SystemTime>,
    fields: BTreeMap<String, FieldSummary>,
    field_order: Vec<String>,
}

impl Group {
    fn add(&mut self, record: &Record) {
        self.count = self.count.saturating_add(record.count.max(1));
        if record.time > UNIX_EPOCH {
            self.first = Some(self.first.map_or(record.time, |t| t.min(record.time)));
            let last = record.last.max(record.time);
            self.last = Some(self.last.map_or(last, |t| t.max(last)));
        }
        for Field { key, value, .. } in &record.fields {
            if key == "span_s" {
                continue;
            }
            if !self.fields.contains_key(key) {
                if self.fields.len() >= MAX_FIELDS_PER_GROUP {
                    continue;
                }
                self.field_order.push(key.clone());
            }
            self.fields.entry(key.clone()).or_default().add(key, value);
        }
    }

    fn line(&self) -> String {
        let mut line = format!(
            "{}×{} {} {}",
            self.level.letter(),
            self.count,
            self.target,
            self.code
        );
        for key in &self.field_order {
            if let Some(summary) = self.fields.get(key) {
                line.push(' ');
                line.push_str(&summary.render(key));
                if summary.seen < self.records_with_fields() {
                    // Present on only some occurrences.
                    line.push('?');
                }
            }
        }
        match (self.first, self.last) {
            (Some(first), Some(last)) if format::utc_clock(first) != format::utc_clock(last) => {
                line.push_str(&format!(
                    " @{}..{}",
                    format::utc_clock(first),
                    format::utc_clock(last)
                ));
            }
            (_, Some(last)) => line.push_str(&format!(" @{}", format::utc_clock(last))),
            _ => {}
        }
        format::bounded(line, format::MAX_LINE_BYTES)
    }

    fn records_with_fields(&self) -> u64 {
        self.fields.values().map(|f| f.seen).max().unwrap_or(0)
    }
}

/// Render a digest of structured records.
pub fn render(component: &str, records: &[Record], options: &Options) -> String {
    let budget = options.budget_tokens.max(MIN_BUDGET_TOKENS);
    let mut groups = BTreeMap::<(Level, String, String), Group>::new();
    let mut overflow_events = 0u64;
    let mut total = 0u64;
    let mut window: (Option<SystemTime>, Option<SystemTime>) = (None, None);
    for record in records {
        if record.level > options.min_level {
            continue;
        }
        if let Some(since) = options.since
            && record.time > UNIX_EPOCH
            && record.last.max(record.time) < since
        {
            continue;
        }
        total = total.saturating_add(record.count.max(1));
        if record.time > UNIX_EPOCH {
            window.0 = Some(window.0.map_or(record.time, |t| t.min(record.time)));
            let last = record.last.max(record.time);
            window.1 = Some(window.1.map_or(last, |t| t.max(last)));
        }
        let key = (record.level, record.target.clone(), record.code.clone());
        if !groups.contains_key(&key) && groups.len() >= MAX_GROUPS {
            overflow_events = overflow_events.saturating_add(record.count.max(1));
            continue;
        }
        groups
            .entry(key)
            .or_insert_with(|| Group {
                level: record.level,
                target: record.target.clone(),
                code: record.code.clone(),
                count: 0,
                first: None,
                last: None,
                fields: BTreeMap::new(),
                field_order: Vec::new(),
            })
            .add(record);
    }
    let mut ordered = groups.into_values().collect::<Vec<_>>();
    ordered.sort_by(|a, b| {
        a.level
            .cmp(&b.level)
            .then(b.count.cmp(&a.count))
            .then(b.last.cmp(&a.last))
            .then(a.code.cmp(&b.code))
    });
    let window_text = match window {
        (Some(first), Some(last)) => format!(
            " window={}..{}",
            format::utc_seconds(first),
            format::utc_seconds(last)
        ),
        _ => String::new(),
    };
    let header = |shown: usize| {
        format!(
            "ADMIRALTY-LOG/1 cmp={}{} events={} groups={} shown={}",
            format::safe_key(component),
            window_text,
            total,
            ordered.len(),
            shown
        )
    };
    // Reserve space for the widest header, omitted line and footer.
    let reserve = header(ordered.len()).len()
        + 1
        + "omitted=000000 groups E=000000 W=000000 I=000000 D=000000 T=000000 events=0000000000"
            .len()
        + 1
        + footer(budget, budget).len();
    let mut body = Vec::new();
    let mut used = reserve;
    let mut omitted = BTreeMap::<Level, usize>::new();
    let mut omitted_events = overflow_events;
    for group in &ordered {
        let line = group.line();
        // Strict priority: once one group is omitted, every later (lower
        // priority) group is omitted too, even if a shorter line would fit.
        if !omitted.is_empty() || (used + line.len() + 1).div_ceil(4) > budget {
            *omitted.entry(group.level).or_default() += 1;
            omitted_events = omitted_events.saturating_add(group.count);
            continue;
        }
        used += line.len() + 1;
        body.push(line);
    }
    let mut text = header(body.len());
    for line in &body {
        text.push('\n');
        text.push_str(line);
    }
    if !omitted.is_empty() || overflow_events > 0 {
        let count = |level| omitted.get(&level).copied().unwrap_or(0);
        text.push_str(&format!(
            "\nomitted={} groups E={} W={} I={} D={} T={} events={}",
            omitted.values().sum::<usize>(),
            count(Level::Error),
            count(Level::Warn),
            count(Level::Info),
            count(Level::Debug),
            count(Level::Trace),
            omitted_events
        ));
    }
    // The footer's own digits are part of the estimate it reports.
    let mut estimate = estimate_tokens(&text) + 1 + footer(budget, budget).len().div_ceil(4);
    for _ in 0..2 {
        estimate = (text.len() + 1 + footer(estimate, budget).len()).div_ceil(4);
    }
    text.push('\n');
    text.push_str(&footer(estimate, budget));
    text
}

fn footer(estimate: usize, budget: usize) -> String {
    format!("est_tokens={estimate}/{budget} tokenizer={TOKENIZER}")
}

/// Digest arbitrary log text (ALOG/1, JSON lines or raw lines).
pub fn render_text(component: &str, text: &str, options: &Options) -> String {
    let records = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| parse_line(line).unwrap_or_else(|| raw_record(line)))
        .collect::<Vec<_>>();
    render(component, &records, options)
}

/// Parse one `ALOG/1` or JSON log line into a record.
pub fn parse_line(line: &str) -> Option<Record> {
    let line = line.trim();
    if line.starts_with('{') {
        return parse_json(line);
    }
    let mut parts = line.splitn(5, ' ');
    let time = format::parse_utc(parts.next()?)?;
    let level = match parts.next()? {
        "E" => Level::Error,
        "W" => Level::Warn,
        "I" => Level::Info,
        "D" => Level::Debug,
        "T" => Level::Trace,
        _ => return None,
    };
    let target = parts.next()?.to_owned();
    let code = parts.next()?.to_owned();
    let mut record = Record {
        time,
        level,
        target,
        code,
        fields: Vec::new(),
        count: 1,
        last: time,
    };
    let mut rest = parts.next().unwrap_or("");
    while !rest.is_empty() {
        let (key, after) = rest.split_once('=')?;
        let (value, remaining) = if after.starts_with('"') {
            // JSON string: find the closing quote that is not escaped.
            let mut escaped = false;
            let mut end = None;
            for (index, c) in after.char_indices().skip(1) {
                match (escaped, c) {
                    (true, _) => escaped = false,
                    (false, '\\') => escaped = true,
                    (false, '"') => {
                        end = Some(index);
                        break;
                    }
                    _ => {}
                }
            }
            let end = end?;
            let text: String = serde_json::from_str(&after[..=end]).ok()?;
            (Value::Str(text), after[end + 1..].trim_start())
        } else {
            let (token, remaining) = after.split_once(' ').unwrap_or((after, ""));
            (typed(token), remaining)
        };
        if key == "x" {
            if let Value::U64(n) = value {
                record.count = n.max(1);
            }
        } else {
            record.fields.push(Field {
                key: key.to_owned(),
                value,
                volatile: false,
            });
        }
        rest = remaining;
    }
    Some(record)
}

fn typed(token: &str) -> Value {
    if let Ok(n) = token.parse::<u64>() {
        Value::U64(n)
    } else if let Ok(n) = token.parse::<i64>() {
        Value::I64(n)
    } else if token.contains('.')
        && let Ok(n) = token.parse::<f64>()
        && n.is_finite()
    {
        Value::F64(n)
    } else if token == "true" || token == "false" {
        Value::Bool(token == "true")
    } else {
        Value::Str(token.to_owned())
    }
}

fn parse_json(line: &str) -> Option<Record> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    let object = value.as_object()?;
    let text = |keys: &[&str]| {
        keys.iter()
            .find_map(|k| object.get(*k).and_then(|v| v.as_str()))
            .map(str::to_owned)
    };
    let level = text(&["lvl", "level", "severity"])
        .and_then(|l| Level::parse(&l))
        .unwrap_or(Level::Info);
    let time = text(&["ts", "timestamp", "time"])
        .and_then(|t| format::parse_utc(&t))
        .unwrap_or(UNIX_EPOCH);
    let target = text(&["tgt", "target", "logger", "module"]).unwrap_or_else(|| "json".into());
    // A structured code is an identifier; a free-text message becomes a
    // quoted template so different values of the same message group together.
    let code = text(&["code"])
        .map(|c| format::safe_key(&c))
        .or_else(|| {
            text(&["msg", "message"]).map(|m| format::compact_string(&template(&format::clean(&m))))
        })
        .unwrap_or_else(|| "event".into());
    let skip = [
        "ts",
        "timestamp",
        "time",
        "lvl",
        "level",
        "severity",
        "tgt",
        "target",
        "logger",
        "module",
        "code",
        "msg",
        "message",
        "cmp",
        "x",
    ];
    let mut fields = Vec::new();
    for (key, value) in object {
        if skip.contains(&key.as_str()) {
            continue;
        }
        let value = match value {
            serde_json::Value::String(s) => Value::Str(s.clone()),
            serde_json::Value::Bool(b) => Value::Bool(*b),
            serde_json::Value::Number(n) => n
                .as_u64()
                .map(Value::U64)
                .or_else(|| n.as_i64().map(Value::I64))
                .or_else(|| n.as_f64().map(Value::F64))
                .unwrap_or(Value::Str(n.to_string())),
            other => Value::Str(format::bounded(other.to_string(), format::MAX_VALUE_BYTES)),
        };
        fields.push(Field {
            key: format::safe_key(key),
            value,
            volatile: false,
        });
    }
    let count = object.get("x").and_then(|x| x.as_u64()).unwrap_or(1).max(1);
    Some(Record {
        time,
        level,
        target: format::safe_key(&target),
        code,
        fields,
        count,
        last: time,
    })
}

/// A record for a line that is not structured: grouped by its template.
pub fn raw_record(line: &str) -> Record {
    let cleaned = format::clean(line);
    let lower = cleaned.to_ascii_lowercase();
    let level = if lower.contains("panicked")
        || lower.contains("error")
        || lower.contains("failed")
        || lower.contains("fatal")
    {
        Level::Error
    } else if lower.contains("warn") {
        Level::Warn
    } else {
        Level::Info
    };
    Record {
        time: UNIX_EPOCH,
        level,
        target: "raw".into(),
        code: format::compact_string(&template(&cleaned)),
        fields: Vec::new(),
        count: 1,
        last: UNIX_EPOCH,
    }
}

/// Collapse volatile parts of free text so repeated messages group together:
/// identifiers (≥8 hex chars or UUIDs) become `<id>`, digit runs become `#`.
pub fn template(text: &str) -> String {
    let mut out = String::with_capacity(text.len().min(160));
    for word in text.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        let core = word.trim_matches(|c: char| !c.is_ascii_alphanumeric());
        let hexish = core.len() >= 8
            && core.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
            && core.chars().any(|c| c.is_ascii_digit());
        if hexish {
            out.push_str(&word.replace(core, "<id>"));
            continue;
        }
        let mut previous_digit = false;
        for c in word.chars() {
            if c.is_ascii_digit() {
                if !previous_digit {
                    out.push('#');
                }
                previous_digit = true;
            } else {
                previous_digit = false;
                out.push(c);
            }
        }
        if out.len() >= 160 {
            break;
        }
    }
    format::bounded(out, 160)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn record(seconds: u64, level: Level, code: &str, fields: Vec<(&str, Value)>) -> Record {
        let time = UNIX_EPOCH + Duration::from_secs(1_800_000_000 + seconds);
        Record {
            time,
            level,
            target: "srv.http".into(),
            code: code.into(),
            fields: fields
                .into_iter()
                .map(|(key, value)| Field {
                    key: key.into(),
                    value,
                    volatile: false,
                })
                .collect(),
            count: 1,
            last: time,
        }
    }

    #[test]
    fn groups_summarize_ranges_and_sets_ordered_by_severity_then_frequency() {
        let mut records = Vec::new();
        for (n, route) in ["/v2/views", "/v2/fleet", "/v2/views", "/v1/x", "/v1/y"]
            .into_iter()
            .enumerate()
        {
            records.push(record(
                n as u64,
                Level::Warn,
                "http.error",
                vec![
                    ("route", Value::Str(route.into())),
                    ("st", Value::U64(503)),
                    ("ms", Value::U64(3 + n as u64 * 200)),
                ],
            ));
        }
        records.push(record(9, Level::Error, "store.fail", vec![]));
        let mut summary = record(10, Level::Info, "http.summary", vec![]);
        summary.count = 40;
        records.push(summary);
        let text = render("admiraltyd", &records, &Options::default());
        let lines = text.lines().collect::<Vec<_>>();
        assert!(
            lines[0].starts_with("ADMIRALTY-LOG/1 cmp=admiraltyd window=2027-01-15T08:00:00Z.."),
            "{text}"
        );
        assert!(lines[0].ends_with("events=46 groups=3 shown=3"), "{text}");
        assert_eq!(lines[1], "E×1 srv.http store.fail @08:00:09Z");
        assert_eq!(
            lines[2],
            "W×5 srv.http http.error route={/v1/x,/v1/y,+2} st=503 ms=3..803 @08:00:00Z..08:00:04Z"
        );
        assert!(lines[3].starts_with("I×40 srv.http http.summary"));
        assert!(lines[4].starts_with("est_tokens="), "{text}");
        let estimate = lines[4]
            .trim_start_matches("est_tokens=")
            .split('/')
            .next()
            .and_then(|n| n.parse::<usize>().ok())
            .expect("estimate");
        assert_eq!(estimate, estimate_tokens(&text), "{text}");
    }

    #[test]
    fn budget_drops_whole_groups_and_reports_what_was_omitted() {
        let records = (0..200)
            .map(|n| {
                record(
                    n,
                    if n % 2 == 0 {
                        Level::Info
                    } else {
                        Level::Debug
                    },
                    &format!("code.{n}"),
                    vec![("detail", Value::Str("x".repeat(60)))],
                )
            })
            .collect::<Vec<_>>();
        for budget in [MIN_BUDGET_TOKENS, 200, 600] {
            let text = render(
                "c",
                &records,
                &Options {
                    budget_tokens: budget,
                    ..Options::default()
                },
            );
            assert!(estimate_tokens(&text) <= budget, "{budget}: {text}");
            let omitted = text
                .lines()
                .find(|l| l.starts_with("omitted="))
                .expect("omitted line");
            assert!(omitted.contains("events="), "{omitted}");
            // Info groups outrank debug groups when space runs out.
            let shown_debug = text.lines().filter(|l| l.starts_with("D×")).count();
            let shown_info = text.lines().filter(|l| l.starts_with("I×")).count();
            assert!(shown_debug == 0 || shown_info == 100, "{text}");
        }
    }

    #[test]
    fn since_and_level_filters_apply_before_grouping() {
        let records = vec![
            record(0, Level::Error, "old", vec![]),
            record(100, Level::Debug, "noisy", vec![]),
            record(100, Level::Warn, "recent", vec![]),
        ];
        let text = render(
            "c",
            &records,
            &Options {
                since: Some(UNIX_EPOCH + Duration::from_secs(1_800_000_050)),
                min_level: Level::Info,
                ..Options::default()
            },
        );
        assert!(text.contains("W×1 srv.http recent"), "{text}");
        assert!(!text.contains("old") && !text.contains("noisy"), "{text}");
    }

    #[test]
    fn foreign_text_is_grouped_by_template_with_inferred_levels() {
        let text = "\
2027-01-15T08:00:00.000Z W obs.git git.timeout project=dope ms=3000
2027-01-15T08:01:00.000Z W obs.git git.timeout x=4 project=dope
{\"ts\":\"2027-01-15T08:02:00Z\",\"level\":\"ERROR\",\"msg\":\"request 1234 failed after 15ms\"}
{\"ts\":\"2027-01-15T08:03:00Z\",\"level\":\"ERROR\",\"msg\":\"request 99 failed after 7ms\"}
thread 'main' panicked at src/lib.rs:10:5
thread 'main' panicked at src/lib.rs:22:9
commit 4ce46c66bb510858653372505a29320a668b853b checked
compiling 12 crates
";
        let digest = render_text("ci", text, &Options::default());
        assert!(
            digest.contains("W×5 obs.git git.timeout project=dope ms=3000?"),
            "{digest}"
        );
        assert!(
            digest.contains("E×2 json \"request # failed after #ms\" @08:02:00Z..08:03:00Z"),
            "{digest}"
        );
        assert!(
            digest.contains("E×2 raw \"thread 'main' panicked at src/lib.rs:#:#\""),
            "{digest}"
        );
        assert!(
            digest.contains("I×1 raw \"commit <id> checked\""),
            "{digest}"
        );
        assert!(digest.contains(" events=11 "), "{digest}");
    }

    #[test]
    fn templates_collapse_identifiers_but_keep_words() {
        assert_eq!(template("took 15ms on port 8080"), "took #ms on port #");
        assert_eq!(
            template("job 6f1c2a3b-0000-4000-8000-1234567890ab done"),
            "job <id> done"
        );
        assert_eq!(
            template("deadbeef"),
            "deadbeef",
            "no digit: a word, not an id"
        );
    }

    #[test]
    fn alog_parser_round_trips_quotes_and_rejects_garbage() {
        let line = "2027-01-15T08:00:00.000Z I srv.http http.summary x=3 note=\"a b=c \\\"q\\\"\" n=-4 r=0.5 ok=true";
        let parsed = parse_line(line).expect("parse");
        assert_eq!(parsed.count, 3);
        assert_eq!(parsed.fields[0].value, Value::Str("a b=c \"q\"".into()));
        assert_eq!(parsed.fields[1].value, Value::I64(-4));
        assert_eq!(parsed.fields[2].value, Value::F64(0.5));
        assert_eq!(parsed.fields[3].value, Value::Bool(true));
        assert!(parse_line("2027-01-15T08:00:00.000Z X t c").is_none());
        assert!(parse_line("hello world").is_none());
        assert!(parse_line("2027-01-15T08:00:00.000Z I t c k=\"unterminated").is_none());
    }
}
