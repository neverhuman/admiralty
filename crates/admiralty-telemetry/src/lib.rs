//! Compact, agent-ready structured telemetry for Admiralty binaries.
//!
//! Design goals, in order:
//! 1. **One event, one bounded line.** `ALOG/1` lines are
//!    `<utc> <L> <target> <code> key=value…`; values are cleaned, redacted by
//!    key and capped, so logs are safe to hand to an agent or a terminal.
//! 2. **Context-smart volume.** Identical events (same level, target, code and
//!    stable fields) inside a coalescing window print once; the repeats are
//!    summarized as one `x=N` line when the window closes. Volatile fields
//!    (latency, request ids) never split a burst.
//! 3. **Token-budgeted digests.** A bounded in-memory ring feeds
//!    [`digest::render`], which groups events by code, summarizes varying
//!    fields as ranges or small sets and stops at an explicit token budget.
//!
//! Configuration comes from `ADMIRALTY_LOG` (`info`, `warn,srv.http=debug`,
//! `off`) and `ADMIRALTY_LOG_FORMAT` (`compact` or `json`). Nothing here
//! performs network I/O, reads secrets or changes process state beyond
//! stderr writes.
pub mod digest;
pub mod format;

use std::{
    collections::{HashMap, VecDeque},
    hash::{Hash, Hasher},
    io::Write,
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, SystemTime},
};

/// Event severity. Ordering is most to least severe.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl Level {
    pub fn letter(self) -> char {
        match self {
            Level::Error => 'E',
            Level::Warn => 'W',
            Level::Info => 'I',
            Level::Debug => 'D',
            Level::Trace => 'T',
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Level::Error => "error",
            Level::Warn => "warn",
            Level::Info => "info",
            Level::Debug => "debug",
            Level::Trace => "trace",
        }
    }
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text.trim().to_ascii_lowercase().as_str() {
            "e" | "error" | "err" => Level::Error,
            "w" | "warn" | "warning" => Level::Warn,
            "i" | "info" => Level::Info,
            "d" | "debug" => Level::Debug,
            "t" | "trace" => Level::Trace,
            _ => return None,
        })
    }
}

/// Per-target level filter. The longest dotted prefix wins:
/// `warn,srv=info,srv.http=debug` logs `srv.http.access` at debug,
/// `srv.views` at info and everything else at warn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Filter {
    default: Level,
    targets: Vec<(String, Level)>,
}

impl Default for Filter {
    fn default() -> Self {
        Self {
            default: Level::Info,
            targets: Vec::new(),
        }
    }
}

impl Filter {
    pub fn parse(spec: &str) -> Result<Self, String> {
        let mut filter = Filter::default();
        for part in spec.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            match part.split_once('=') {
                Some((target, level)) => {
                    let target = target.trim();
                    if target.is_empty()
                        || !target
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
                    {
                        return Err(format!("invalid log target in {part:?}"));
                    }
                    let level = Level::parse(level)
                        .ok_or_else(|| format!("invalid log level in {part:?}"))?;
                    filter.targets.push((target.to_owned(), level));
                }
                None => {
                    filter.default =
                        Level::parse(part).ok_or_else(|| format!("invalid log level {part:?}"))?;
                }
            }
        }
        // Longest prefix first so the first match is the most specific.
        filter
            .targets
            .sort_by(|a, b| b.0.len().cmp(&a.0.len()).then_with(|| a.0.cmp(&b.0)));
        Ok(filter)
    }

    pub fn enabled(&self, level: Level, target: &str) -> bool {
        let limit = self
            .targets
            .iter()
            .find(|(prefix, _)| {
                target == prefix
                    || target
                        .strip_prefix(prefix.as_str())
                        .is_some_and(|rest| rest.starts_with('.'))
            })
            .map_or(self.default, |(_, level)| *level);
        level <= limit
    }
}

/// A typed field value. Numbers stay numbers in JSON output and digests.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Str(String),
    U64(u64),
    I64(i64),
    F64(f64),
    Bool(bool),
}

impl Value {
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::U64(n) => Some(*n as f64),
            Value::I64(n) => Some(*n as f64),
            Value::F64(n) if n.is_finite() => Some(*n),
            _ => None,
        }
    }
    /// Rendered value text as it appears after `key=` in a compact line.
    pub fn render(&self, key: &str) -> String {
        match self {
            Value::Str(s) => format::compact_string(&format::safe_string(key, s)),
            Value::U64(n) => n.to_string(),
            Value::I64(n) => n.to_string(),
            Value::F64(n) => format::decimal(*n),
            Value::Bool(b) => b.to_string(),
        }
    }
    fn json(&self, key: &str) -> serde_json::Value {
        match self {
            Value::Str(s) => serde_json::Value::String(format::safe_string(key, s)),
            Value::U64(n) => (*n).into(),
            Value::I64(n) => (*n).into(),
            Value::F64(n) => {
                serde_json::Number::from_f64(*n).map_or(serde_json::Value::Null, Into::into)
            }
            Value::Bool(b) => (*b).into(),
        }
    }
    fn hash_into(&self, hasher: &mut impl Hasher) {
        match self {
            Value::Str(s) => s.hash(hasher),
            Value::U64(n) => n.hash(hasher),
            Value::I64(n) => n.hash(hasher),
            Value::F64(n) => n.to_bits().hash(hasher),
            Value::Bool(b) => b.hash(hasher),
        }
    }
}

macro_rules! value_from {
    ($($t:ty => $variant:ident as $cast:ty),* $(,)?) => {$(
        impl From<$t> for Value {
            fn from(value: $t) -> Self {
                Value::$variant(value as $cast)
            }
        }
    )*};
}
value_from!(u64 => U64 as u64, u32 => U64 as u64, u16 => U64 as u64, usize => U64 as u64,
    i64 => I64 as i64, i32 => I64 as i64, f64 => F64 as f64, f32 => F64 as f64);
impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Value::Bool(value)
    }
}
impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Value::Str(value.to_owned())
    }
}
impl From<String> for Value {
    fn from(value: String) -> Self {
        Value::Str(value)
    }
}
impl From<&String> for Value {
    fn from(value: &String) -> Self {
        Value::Str(value.clone())
    }
}

/// One named field. Volatile fields (latency, byte counts, request ids) are
/// rendered but excluded from the coalescing fingerprint.
#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub key: String,
    pub value: Value,
    pub volatile: bool,
}

/// One structured occurrence, as retained in the ring and fed to digests.
#[derive(Clone, Debug, PartialEq)]
pub struct Record {
    pub time: SystemTime,
    pub level: Level,
    pub target: String,
    pub code: String,
    pub fields: Vec<Field>,
    /// Number of occurrences this record stands for (1, or N for a burst summary).
    pub count: u64,
    /// Last occurrence covered by `count`; equals `time` for a single event.
    pub last: SystemTime,
}

impl Record {
    fn fingerprint(&self) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.level.hash(&mut hasher);
        self.target.hash(&mut hasher);
        self.code.hash(&mut hasher);
        for field in self.fields.iter().filter(|f| !f.volatile) {
            field.key.hash(&mut hasher);
            field.value.hash_into(&mut hasher);
        }
        hasher.finish()
    }
}

/// Output encoding for the line sink.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputFormat {
    /// `ALOG/1` key=value lines (default; several times fewer tokens than JSON).
    Compact,
    /// One JSON object per line for log shippers.
    Json,
}

/// Where rendered lines go.
#[derive(Clone)]
pub enum Sink {
    Stderr,
    /// Shared in-memory buffer, for tests and embedding.
    Memory(Arc<Mutex<Vec<u8>>>),
    /// Discard lines but keep ring/digest state (`ADMIRALTY_LOG=off`).
    Null,
}

#[derive(Clone)]
pub struct Config {
    pub component: String,
    pub filter: Filter,
    pub format: OutputFormat,
    /// Identical events inside this window print once; zero disables coalescing.
    pub coalesce: Duration,
    /// Records retained for digests.
    pub ring_capacity: usize,
    pub sink: Sink,
}

impl Config {
    pub fn new(component: &str) -> Self {
        Self {
            component: format::safe_key(component),
            filter: Filter::default(),
            format: OutputFormat::Compact,
            coalesce: Duration::from_secs(60),
            ring_capacity: 1024,
            sink: Sink::Stderr,
        }
    }

    /// Read `ADMIRALTY_LOG` and `ADMIRALTY_LOG_FORMAT`. Invalid values keep the
    /// defaults and are reported in the returned warnings rather than aborting
    /// startup.
    pub fn from_env(component: &str) -> (Self, Vec<String>) {
        Self::from_values(
            component,
            std::env::var("ADMIRALTY_LOG").ok().as_deref(),
            std::env::var("ADMIRALTY_LOG_FORMAT").ok().as_deref(),
        )
    }

    /// Environment parsing without touching the process environment.
    pub fn from_values(
        component: &str,
        filter: Option<&str>,
        output: Option<&str>,
    ) -> (Self, Vec<String>) {
        let mut config = Self::new(component);
        let mut warnings = Vec::new();
        if let Some(spec) = filter {
            if spec.trim().eq_ignore_ascii_case("off") {
                config.sink = Sink::Null;
            } else {
                match Filter::parse(spec) {
                    Ok(filter) => config.filter = filter,
                    Err(error) => warnings.push(error),
                }
            }
        }
        if let Some(spec) = output {
            match spec.trim().to_ascii_lowercase().as_str() {
                "compact" | "alog" | "" => config.format = OutputFormat::Compact,
                "json" | "ndjson" => config.format = OutputFormat::Json,
                other => warnings.push(format!("invalid ADMIRALTY_LOG_FORMAT {other:?}")),
            }
        }
        (config, warnings)
    }
}

struct Burst {
    sample: Record,
    first: SystemTime,
    last: SystemTime,
    suppressed: u64,
}

#[derive(Default)]
struct State {
    bursts: HashMap<u64, Burst>,
    ring: VecDeque<Record>,
    emitted: u64,
    suppressed: u64,
    dropped_by_ring: u64,
}

/// Point-in-time counters for `/metrics` and diagnostics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counters {
    pub emitted: u64,
    pub suppressed: u64,
    pub open_bursts: u64,
    pub ring_len: u64,
    pub dropped_by_ring: u64,
}

pub struct Logger {
    config: Config,
    state: Mutex<State>,
}

impl Logger {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            state: Mutex::new(State::default()),
        }
    }

    pub fn component(&self) -> &str {
        &self.config.component
    }

    pub fn enabled(&self, level: Level, target: &str) -> bool {
        self.config.filter.enabled(level, target)
    }

    /// Start an event; field calls are no-ops when the level is filtered out.
    pub fn event<'a>(&'a self, level: Level, target: &str, code: &str) -> Event<'a> {
        let now = SystemTime::now();
        Event {
            logger: self,
            record: self.enabled(level, target).then(|| Record {
                time: now,
                level,
                target: format::safe_key(target),
                code: format::safe_key(code),
                fields: Vec::new(),
                count: 1,
                last: now,
            }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        // A panicking writer must not silence every later event.
        self.state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    /// Record one event at its own timestamp (tests and replay set `time`).
    pub fn emit_record(&self, mut record: Record) {
        record.last = record.time;
        let now = record.time;
        let mut state = self.lock();
        self.close_expired(&mut state, now);
        let key = record.fingerprint();
        if !self.config.coalesce.is_zero() {
            if let Some(burst) = state.bursts.get_mut(&key) {
                burst.suppressed += 1;
                burst.last = now;
                state.suppressed += 1;
                return;
            }
            state.bursts.insert(
                key,
                Burst {
                    sample: record.clone(),
                    first: now,
                    last: now,
                    suppressed: 0,
                },
            );
        }
        state.emitted += 1;
        self.write(&record);
        self.retain(&mut state, record);
    }

    /// Close bursts whose window ended, printing `x=N` summaries for repeats.
    pub fn flush_expired(&self) {
        let mut state = self.lock();
        self.close_expired(&mut state, SystemTime::now());
    }

    /// Close every open burst (call before exit so repeats are not lost).
    pub fn flush(&self) {
        let mut state = self.lock();
        let keys = state.bursts.keys().copied().collect::<Vec<_>>();
        for key in keys {
            self.close(&mut state, key);
        }
    }

    fn close_expired(&self, state: &mut State, now: SystemTime) {
        let window = self.config.coalesce;
        let mut expired = state
            .bursts
            .iter()
            .filter(|(_, burst)| now.duration_since(burst.first).unwrap_or_default() >= window)
            .map(|(key, burst)| (burst.first, *key))
            .collect::<Vec<_>>();
        // Deterministic output order: oldest burst first.
        expired.sort_unstable();
        for (_, key) in expired {
            self.close(state, key);
        }
    }

    fn close(&self, state: &mut State, key: u64) {
        let Some(burst) = state.bursts.remove(&key) else {
            return;
        };
        if burst.suppressed == 0 {
            return;
        }
        let mut summary = burst.sample;
        summary.time = burst.last;
        summary.count = burst.suppressed;
        summary.last = burst.last;
        // Volatile values belonged to the first occurrence; a summary keeps
        // only the stable identity plus the repeat count and span.
        summary.fields.retain(|f| !f.volatile);
        let span = burst.last.duration_since(burst.first).unwrap_or_default();
        summary.fields.insert(
            0,
            Field {
                key: "span_s".into(),
                value: Value::U64(span.as_secs()),
                volatile: true,
            },
        );
        self.write(&summary);
        self.retain(state, summary);
    }

    fn retain(&self, state: &mut State, record: Record) {
        if self.config.ring_capacity == 0 {
            return;
        }
        while state.ring.len() >= self.config.ring_capacity {
            state.ring.pop_front();
            state.dropped_by_ring += 1;
        }
        state.ring.push_back(record);
    }

    fn write(&self, record: &Record) {
        let line = match self.config.format {
            OutputFormat::Compact => compact_line(record),
            OutputFormat::Json => json_line(&self.config.component, record),
        };
        match &self.config.sink {
            Sink::Stderr => {
                let mut stderr = std::io::stderr().lock();
                // Logging must never take the service down.
                let _ = stderr.write_all(line.as_bytes());
                let _ = stderr.write_all(b"\n");
            }
            Sink::Memory(buffer) => {
                let mut buffer = buffer.lock().unwrap_or_else(|p| p.into_inner());
                buffer.extend_from_slice(line.as_bytes());
                buffer.push(b'\n');
            }
            Sink::Null => {}
        }
    }

    /// Records currently in the ring plus open bursts' pending repeats.
    pub fn snapshot(&self) -> Vec<Record> {
        let state = self.lock();
        let mut records = state.ring.iter().cloned().collect::<Vec<_>>();
        let mut pending = state
            .bursts
            .values()
            .filter(|b| b.suppressed > 0)
            .map(|burst| {
                let mut record = burst.sample.clone();
                record.fields.retain(|f| !f.volatile);
                record.count = burst.suppressed;
                record.time = burst.last;
                record.last = burst.last;
                record
            })
            .collect::<Vec<_>>();
        pending.sort_by_key(|r| r.time);
        records.extend(pending);
        records
    }

    pub fn counters(&self) -> Counters {
        let state = self.lock();
        Counters {
            emitted: state.emitted,
            suppressed: state.suppressed,
            open_bursts: state.bursts.len() as u64,
            ring_len: state.ring.len() as u64,
            dropped_by_ring: state.dropped_by_ring,
        }
    }

    /// Token-budgeted digest of the retained window (see [`digest::render`]).
    pub fn digest(&self, budget_tokens: usize) -> String {
        digest::render(
            &self.config.component,
            &self.snapshot(),
            &digest::Options {
                budget_tokens,
                ..digest::Options::default()
            },
        )
    }
}

/// Builder returned by [`Logger::event`] and [`event`].
pub struct Event<'a> {
    logger: &'a Logger,
    record: Option<Record>,
}

impl Event<'_> {
    /// A stable field: part of the event identity used for coalescing.
    pub fn field(mut self, key: &str, value: impl Into<Value>) -> Self {
        if let Some(record) = &mut self.record {
            record.fields.push(Field {
                key: format::safe_key(key),
                value: value.into(),
                volatile: false,
            });
        }
        self
    }
    /// A volatile field: rendered, but repeats with different values coalesce.
    pub fn volatile(mut self, key: &str, value: impl Into<Value>) -> Self {
        if let Some(record) = &mut self.record {
            record.fields.push(Field {
                key: format::safe_key(key),
                value: value.into(),
                volatile: true,
            });
        }
        self
    }
    /// Optional stable field; `None` is omitted rather than rendered as null.
    pub fn field_opt<V: Into<Value>>(self, key: &str, value: Option<V>) -> Self {
        match value {
            Some(value) => self.field(key, value),
            None => self,
        }
    }
    /// True when this event will be recorded (lets callers skip costly fields).
    pub fn active(&self) -> bool {
        self.record.is_some()
    }
    pub fn emit(self) {
        if let Some(record) = self.record {
            self.logger.emit_record(record);
        }
    }
}

/// Render one `ALOG/1` line. Fields that would exceed [`format::MAX_LINE_BYTES`]
/// are dropped whole and counted, never cut mid-value.
pub fn compact_line(record: &Record) -> String {
    let mut line = format!(
        "{} {} {} {}",
        format::utc_millis(record.time),
        record.level.letter(),
        record.target,
        record.code
    );
    if record.count > 1 {
        line.push_str(&format!(" x={}", record.count));
    }
    let mut dropped = 0usize;
    for field in &record.fields {
        let piece = format!(" {}={}", field.key, field.value.render(&field.key));
        if line.len() + piece.len() > format::MAX_LINE_BYTES - 16 {
            dropped += 1;
            continue;
        }
        line.push_str(&piece);
    }
    if dropped > 0 {
        line.push_str(&format!(" dropped={dropped}"));
    }
    line
}

/// Render one JSON line with short, stable keys.
pub fn json_line(component: &str, record: &Record) -> String {
    let identity = || {
        let mut object = serde_json::Map::new();
        object.insert("ts".into(), format::utc_millis(record.time).into());
        object.insert("lvl".into(), record.level.name().into());
        object.insert("cmp".into(), component.into());
        object.insert("tgt".into(), record.target.clone().into());
        object.insert("code".into(), record.code.clone().into());
        if record.count > 1 {
            object.insert("x".into(), record.count.into());
        }
        object
    };
    let mut object = identity();
    for field in &record.fields {
        if !object.contains_key(&field.key) {
            object.insert(field.key.clone(), field.value.json(&field.key));
        }
    }
    let text = serde_json::Value::Object(object).to_string();
    if text.len() <= format::MAX_LINE_BYTES * 2 {
        return text;
    }
    // Keep identity; drop fields rather than emit an unbounded line.
    let mut minimal = identity();
    minimal.insert("dropped".into(), record.fields.len().into());
    serde_json::Value::Object(minimal).to_string()
}

static GLOBAL: OnceLock<Logger> = OnceLock::new();

/// Install the process logger from the environment. Returns warnings about
/// ignored configuration; a second call keeps the first logger.
pub fn init_from_env(component: &str) -> Vec<String> {
    let (config, warnings) = Config::from_env(component);
    let _ = GLOBAL.set(Logger::new(config));
    let logger = global();
    for warning in &warnings {
        logger
            .event(Level::Warn, "telemetry", "config.ignored")
            .field("detail", warning.as_str())
            .emit();
    }
    warnings
}

/// Install an explicit logger (tests, embedding). Returns false if one exists.
pub fn init(config: Config) -> bool {
    GLOBAL.set(Logger::new(config)).is_ok()
}

/// The process logger; defaults to info/compact/stderr when never initialized.
pub fn global() -> &'static Logger {
    GLOBAL.get_or_init(|| Logger::new(Config::new("admiralty")))
}

/// Shorthand for `global().event(..)`.
pub fn event(level: Level, target: &str, code: &str) -> Event<'static> {
    global().event(level, target, code)
}

/// Close expired bursts every `interval` on a detached thread, so repeat
/// summaries appear even when a component goes quiet. Idempotent per process.
pub fn spawn_flusher(interval: Duration) {
    static STARTED: OnceLock<()> = OnceLock::new();
    if STARTED.set(()).is_err() {
        return;
    }
    let interval = interval.max(Duration::from_secs(1));
    let _ = std::thread::Builder::new()
        .name("admiralty-telemetry-flush".into())
        .spawn(move || {
            loop {
                std::thread::sleep(interval);
                global().flush_expired();
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::UNIX_EPOCH;

    fn memory_logger(coalesce: u64) -> (Logger, Arc<Mutex<Vec<u8>>>) {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let mut config = Config::new("test");
        config.coalesce = Duration::from_secs(coalesce);
        config.sink = Sink::Memory(buffer.clone());
        config.filter = Filter::parse("debug").expect("filter");
        (Logger::new(config), buffer)
    }

    fn lines(buffer: &Arc<Mutex<Vec<u8>>>) -> Vec<String> {
        String::from_utf8(buffer.lock().expect("buffer").clone())
            .expect("utf8")
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn at(logger: &Logger, seconds: u64, route: &str, ms: u64) {
        let mut record = logger
            .event(Level::Warn, "srv.http", "http.error")
            .field("route", route)
            .field("st", 503u32)
            .volatile("ms", ms)
            .record
            .expect("enabled");
        record.time = UNIX_EPOCH + Duration::from_secs(1_800_000_000 + seconds);
        logger.emit_record(record);
    }

    #[test]
    fn filter_uses_longest_dotted_prefix_and_rejects_bad_specs() {
        let filter = Filter::parse("warn, srv=info ,srv.http=debug").expect("valid");
        assert!(filter.enabled(Level::Debug, "srv.http"));
        assert!(filter.enabled(Level::Debug, "srv.http.access"));
        assert!(!filter.enabled(Level::Debug, "srv.views"));
        assert!(filter.enabled(Level::Info, "srv.views"));
        assert!(!filter.enabled(Level::Info, "srvx"));
        assert!(!filter.enabled(Level::Info, "obs.git"));
        assert!(filter.enabled(Level::Error, "obs.git"));
        assert!(Filter::parse("verbose").is_err());
        assert!(Filter::parse("srv http=info").is_err());
        assert!(Filter::parse("srv=loud").is_err());
        assert_eq!(Filter::parse("").expect("empty"), Filter::default());
    }

    #[test]
    fn environment_values_configure_without_aborting_on_mistakes() {
        let (config, warnings) = Config::from_values("srv", Some("debug"), Some("json"));
        assert!(warnings.is_empty());
        assert_eq!(config.format, OutputFormat::Json);
        assert!(config.filter.enabled(Level::Debug, "anything"));
        let (config, warnings) = Config::from_values("srv", Some("loud"), Some("xml"));
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert_eq!(config.format, OutputFormat::Compact);
        assert_eq!(config.filter, Filter::default());
        let (config, _) = Config::from_values("srv", Some("OFF"), None);
        assert!(matches!(config.sink, Sink::Null));
    }

    #[test]
    fn repeats_coalesce_with_volatile_fields_and_summaries_report_counts() {
        let (logger, buffer) = memory_logger(60);
        at(&logger, 0, "/v2/views", 10);
        at(&logger, 1, "/v2/views", 900); // volatile ms differs: still a repeat
        at(&logger, 2, "/v2/views", 11);
        at(&logger, 3, "/v2/fleet", 12); // stable route differs: new event
        assert_eq!(lines(&buffer).len(), 2, "{:?}", lines(&buffer));
        let counters = logger.counters();
        assert_eq!((counters.emitted, counters.suppressed), (2, 2));
        // Both windows close on the next event after 60s; /v2/fleet had no
        // repeats so it closes silently.
        at(&logger, 61, "/v2/views", 13);
        let out = lines(&buffer);
        assert_eq!(out.len(), 4, "{out:?}");
        assert!(
            out[2].contains(" W srv.http http.error x=2 span_s=2 route=/v2/views st=503"),
            "{}",
            out[2]
        );
        assert!(!out[2].contains("ms="), "summary drops volatile values");
        assert!(out[3].ends_with("route=/v2/views st=503 ms=13"));
        logger.flush();
        assert_eq!(
            lines(&buffer).len(),
            4,
            "no repeats pending: flush is silent"
        );
    }

    #[test]
    fn zero_window_disables_coalescing() {
        let (logger, buffer) = memory_logger(0);
        for ms in 0..3u64 {
            at(&logger, ms, "/x", ms);
        }
        assert_eq!(lines(&buffer).len(), 3);
        assert_eq!(logger.counters().suppressed, 0);
    }

    #[test]
    fn disabled_events_cost_nothing_and_never_reach_the_sink() {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let mut config = Config::new("test");
        config.sink = Sink::Memory(buffer.clone());
        config.filter = Filter::parse("warn").expect("filter");
        let logger = Logger::new(config);
        let event = logger.event(Level::Info, "x", "y").field("k", "v");
        assert!(!event.active());
        event.emit();
        assert!(lines(&buffer).is_empty());
        assert_eq!(logger.counters(), Counters::default());
    }

    #[test]
    fn lines_are_single_bounded_redacted_and_parseable() {
        let (logger, buffer) = memory_logger(0);
        logger
            .event(Level::Error, "srv.auth", "auth.rejected")
            .field("token", "super-secret-value")
            .field("detail", "line one\nline two \u{1b}[31mred")
            .field("huge", "x".repeat(5000))
            .field("a", "y".repeat(250))
            .field("b", "y".repeat(250))
            .field("c", "y".repeat(250))
            .field("d", "y".repeat(250))
            .emit();
        let out = lines(&buffer);
        assert_eq!(out.len(), 1);
        let line = &out[0];
        assert!(line.len() <= format::MAX_LINE_BYTES, "{}", line.len());
        assert!(
            line.ends_with(" dropped=1") || line.ends_with(" dropped=2"),
            "{line}"
        );
        assert!(!line.contains("super-secret-value"));
        assert!(line.contains("token=[redacted]"));
        assert!(line.contains("detail=\"line one line two red\""));
        let parsed = digest::parse_line(line).expect("own lines parse");
        assert_eq!(parsed.code, "auth.rejected");
        assert_eq!(parsed.level, Level::Error);
    }

    #[test]
    fn json_lines_keep_types_and_redaction() {
        let record = Record {
            time: UNIX_EPOCH,
            level: Level::Info,
            target: "obs.cycle".into(),
            code: "cycle.ok".into(),
            fields: vec![
                Field {
                    key: "ms".into(),
                    value: Value::U64(42),
                    volatile: true,
                },
                Field {
                    key: "ratio".into(),
                    value: Value::F64(0.126),
                    volatile: false,
                },
                Field {
                    key: "password".into(),
                    value: Value::Str("p".into()),
                    volatile: false,
                },
            ],
            count: 3,
            last: UNIX_EPOCH,
        };
        let value: serde_json::Value =
            serde_json::from_str(&json_line("observe", &record)).expect("json");
        assert_eq!(value["ms"], 42);
        assert_eq!(value["ratio"], 0.126);
        assert_eq!(value["password"], "[redacted]");
        assert_eq!(value["x"], 3);
        assert_eq!(value["cmp"], "observe");
        assert_eq!(compact_line(&record).split(' ').nth(4), Some("x=3"));
        assert!(compact_line(&record).ends_with("ms=42 ratio=0.13 password=[redacted]"));
        let parsed = digest::parse_line(&json_line("observe", &record)).expect("json parses");
        assert_eq!(parsed.count, 3);
    }

    #[test]
    fn ring_is_bounded_and_snapshot_includes_open_burst_repeats() {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let mut config = Config::new("test");
        config.sink = Sink::Memory(buffer);
        config.ring_capacity = 2;
        let logger = Logger::new(config);
        for n in 0..4u64 {
            logger
                .event(Level::Info, "t", "distinct")
                .field("n", n)
                .emit();
        }
        for _ in 0..3 {
            logger.event(Level::Warn, "t", "repeat").emit();
        }
        let counters = logger.counters();
        assert_eq!(counters.ring_len, 2);
        assert_eq!(counters.dropped_by_ring, 3);
        let snapshot = logger.snapshot();
        let pending = snapshot.iter().find(|r| r.code == "repeat" && r.count == 2);
        assert!(pending.is_some(), "{snapshot:?}");
        let digest = logger.digest(400);
        assert!(digest.contains("W×3 t repeat"), "{digest}");
    }
}
