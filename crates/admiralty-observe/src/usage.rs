//! Bounded, read-only usage observations. Logs are never copied into output.
use crate::{config::ObserveConfig, io};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

const FILE_LIMIT: usize = 128;
const FILE_BYTES: usize = 2 * 1024 * 1024;
const SCAN_BYTES: usize = 32 * 1024 * 1024;
const WINDOW: i64 = 48 * 3600;
const STATE_BYTES: usize = 8 * 1024 * 1024;
const SESSION_SAMPLES: usize = 4096;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Tokens {
    input: u64,
    cached: Option<u64>,
    output: u64,
}
impl Tokens {
    fn parse(v: &Value, claude: bool) -> Option<Self> {
        let input = v.get("input_tokens")?.as_u64()?;
        let output = v.get("output_tokens")?.as_u64()?;
        let cached = v
            .get(if claude {
                "cache_read_input_tokens"
            } else {
                "cached_input_tokens"
            })
            .and_then(Value::as_u64);
        if !claude && cached.is_some_and(|n| n > input) {
            return None;
        }
        let created = if claude {
            v.get("cache_creation_input_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0)
        } else {
            0
        };
        Some(Self {
            input: if claude {
                input
                    .checked_add(cached.unwrap_or(0))?
                    .checked_add(created)?
            } else {
                input
            },
            cached,
            output,
        })
    }
    fn difference(self, previous: Self) -> (Self, bool) {
        let reset = self.input < previous.input
            || self.output < previous.output
            || matches!((self.cached, previous.cached), (Some(a),Some(b)) if a < b);
        if reset {
            return (self, true);
        }
        (
            Self {
                input: self.input - previous.input,
                output: self.output - previous.output,
                cached: self.cached.zip(previous.cached).map(|(a, b)| a - b),
            },
            false,
        )
    }
    fn total(self) -> u64 {
        self.input.saturating_add(self.output)
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Sample {
    at: i64,
    model: String,
    tokens: Tokens,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct Session {
    id: String,
    family: String,
    cwd: String,
    samples: Vec<Sample>,
    quota: Option<(i64, Value)>,
    resets: usize,
    truncated: bool,
    /// A truncated source becomes complete only for windows starting after this
    /// observed cumulative baseline. None never means a zero baseline.
    coverage_start: Option<i64>,
    incomplete: bool,
    observed_usage: bool,
    model: String,
    previous: Option<Tokens>,
    last_at: Option<i64>,
    messages: BTreeMap<String, Sample>,
}
impl Session {
    fn complete_for(&self, start: i64) -> bool {
        self.observed_usage
            && !self.incomplete
            && (!self.truncated || self.coverage_start.is_some_and(|at| at <= start))
    }
    fn trim(&mut self, now: i64) {
        self.samples
            .retain(|s| s.at >= now - WINDOW && s.at <= now + 5);
        self.messages
            .retain(|_, s| s.at >= now - WINDOW && s.at <= now + 5);
        if self.samples.len() > SESSION_SAMPLES {
            self.samples.drain(..self.samples.len() - SESSION_SAMPLES);
            self.truncated = true;
            self.coverage_start = self.samples.first().map(|s| s.at);
        }
        if self.messages.len() > SESSION_SAMPLES {
            let mut by_time = self
                .messages
                .iter()
                .map(|(id, s)| (s.at, id.clone()))
                .collect::<Vec<_>>();
            by_time.sort();
            for (_, id) in by_time
                .into_iter()
                .take(self.messages.len() - SESSION_SAMPLES)
            {
                self.messages.remove(&id);
            }
            self.truncated = true;
            self.coverage_start = self.messages.values().map(|s| s.at).min();
        }
    }
}
fn merge_session(old: &mut Session, mut incoming: Session) {
    let key = |sample: &Sample| {
        (
            sample.at,
            sample.model.clone(),
            sample.tokens.input,
            sample.tokens.cached,
            sample.tokens.output,
        )
    };
    let previous = old.samples.iter().map(key).collect::<BTreeSet<_>>();
    let next = incoming.samples.iter().map(key).collect::<BTreeSet<_>>();
    let previous_subset = previous.is_subset(&next);
    let next_subset = next.is_subset(&previous);
    let ambiguous = !previous_subset && !next_subset;
    let since = |session: &Session| {
        if !session.truncated {
            i64::MIN
        } else {
            session.coverage_start.unwrap_or(i64::MAX)
        }
    };
    let prefer_incoming = (previous_subset && !next_subset)
        || (previous == next
            && (incoming.incomplete, since(&incoming)) < (old.incomplete, since(old)))
        || (ambiguous && incoming.last_at > old.last_at);
    let quota = [old.quota.clone(), incoming.quota.clone()]
        .into_iter()
        .flatten()
        .max_by_key(|(at, _)| *at);
    if prefer_incoming {
        std::mem::swap(old, &mut incoming);
    }
    old.incomplete |= ambiguous;
    old.quota = quota;
}
#[derive(Clone, Copy)]
struct Rate {
    input: f64,
    cached: f64,
    output: f64,
}
impl Rate {
    fn credits(self, tokens: Tokens) -> Option<f64> {
        let cached = tokens.cached?;
        if cached > tokens.input {
            return None;
        }
        let value = ((tokens.input - cached) as f64 * self.input
            + cached as f64 * self.cached
            + tokens.output as f64 * self.output)
            / 1_000_000.0;
        value.is_finite().then_some(value)
    }
}
fn timestamp(v: &Value) -> Option<i64> {
    DateTime::parse_from_rfc3339(v.as_str()?)
        .ok()
        .map(|t| t.timestamp())
}
fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}
fn percent(part: u64, total: u64) -> Value {
    if total == 0 {
        Value::Null
    } else {
        json!(100.0 * part as f64 / total as f64)
    }
}
fn iso(at: i64) -> Value {
    DateTime::<Utc>::from_timestamp(at, 0)
        .map(|t| json!(t.to_rfc3339()))
        .unwrap_or(Value::Null)
}
fn observed_quota(value: &Value) -> Value {
    let window = |value: &Value| {
        json!({
            "used_percent":value["used_percent"].as_f64().filter(|n|(0.0..=100.0).contains(n)),
            "window_minutes":value["window_minutes"].as_u64(),
            "resets_at":value["resets_at"].as_i64().filter(|n|*n>=0)
        })
    };
    let balance = value["credits"]["balance"]
        .as_f64()
        .or_else(|| {
            value["credits"]["balance"]
                .as_str()
                .and_then(|s| s.parse::<f64>().ok())
        })
        .filter(|n| n.is_finite() && *n >= 0.0);
    json!({"primary":window(&value["primary"]),"secondary":window(&value["secondary"]),"credits":{"balance":balance},"plan_type":value["plan_type"].as_str().map(|s|io::clean(s).chars().take(64).collect::<String>())})
}

/// No symlink traversal and a hard directory-entry budget, even for malformed trees.
fn files(root: &Path, maximum: usize) -> Vec<PathBuf> {
    inventory(root, maximum, None).0
}
fn inventory(root: &Path, maximum: usize, minimum_modified: Option<i64>) -> (Vec<PathBuf>, bool) {
    let mut todo = vec![root.to_path_buf()];
    let mut found = Vec::new();
    let mut visited = 0;
    let mut complete = true;
    while let Some(path) = todo.pop() {
        visited += 1;
        if visited > 8192 {
            complete = false;
            break;
        }
        let Ok(meta) = fs::symlink_metadata(&path) else {
            complete = false;
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            if let Ok(entries) = fs::read_dir(path) {
                for entry in entries.take(8192 - visited) {
                    let Ok(entry) = entry else {
                        complete = false;
                        continue;
                    };
                    if visited + todo.len() >= 8192 {
                        complete = false;
                        break;
                    }
                    todo.push(entry.path());
                }
            } else {
                complete = false;
            }
        } else if meta.is_file() && path.extension().is_some_and(|e| e == "jsonl") {
            let modified = meta.modified().ok();
            let at = modified
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64);
            if minimum_modified.is_none_or(|cut| at.is_none_or(|at| at >= cut)) {
                found.push((modified, path));
            }
        }
    }
    found.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    complete &= found.len() <= maximum;
    (
        found.into_iter().take(maximum).map(|(_, p)| p).collect(),
        complete,
    )
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct CachedFile {
    identity: String,
    offset: u64,
    guard: String,
    pending_record: bool,
    session: Session,
}
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct UsageState {
    version: u32,
    files: BTreeMap<String, CachedFile>,
}
fn file_identity(meta: &fs::Metadata) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        format!("{}:{}", meta.dev(), meta.ino())
    }
    #[cfg(not(unix))]
    {
        format!("{:?}", meta.created().ok())
    }
}
fn guard(file: &mut fs::File, offset: u64) -> Option<String> {
    let size = offset.min(512);
    file.seek(SeekFrom::Start(offset - size)).ok()?;
    let mut bytes = vec![0; size as usize];
    file.read_exact(&mut bytes).ok()?;
    Some(format!("{:x}", Sha256::digest(bytes)))
}
/// Incremental byte offsets and cumulative baselines survive restarts. Gaps,
/// replacement/truncation, bounded eviction and malformed lines fail closed for
/// window completeness; only numeric usage metadata enters this private cache.
fn read_usage(
    path: &Path,
    family: &str,
    old: Option<&CachedFile>,
    budget: &mut usize,
    now: i64,
) -> Option<CachedFile> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(
            (rustix::fs::OFlags::NOFOLLOW
                | rustix::fs::OFlags::NONBLOCK
                | rustix::fs::OFlags::CLOEXEC)
                .bits() as i32,
        );
    }
    let mut file = options.open(path).ok()?;
    let meta = file.metadata().ok()?;
    if !meta.is_file() {
        return None;
    }
    let identity = file_identity(&meta);
    let size = meta.len();
    let reusable = old.filter(|old| {
        old.identity == identity
            && old.offset <= size
            && guard(&mut file, old.offset).as_deref() == Some(old.guard.as_str())
    });
    let maximum = FILE_BYTES.min(*budget) as u64;
    if maximum == 0 {
        return None;
    }
    let continuous = reusable.is_some_and(|old| size - old.offset <= maximum);
    let start = if continuous {
        reusable.map(|old| old.offset).unwrap_or(0)
    } else {
        size.saturating_sub(maximum)
    };
    let skipped = start > 0 && !continuous;
    let mut prefix = String::new();
    if skipped {
        let amount = (32 * 1024).min(budget.saturating_sub(maximum as usize));
        file.seek(SeekFrom::Start(0)).ok()?;
        let mut bytes = Vec::new();
        (&mut file)
            .take(amount as u64)
            .read_to_end(&mut bytes)
            .ok()?;
        *budget = budget.saturating_sub(bytes.len());
        prefix = String::from_utf8_lossy(&bytes).into_owned();
    }
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::new();
    (&mut file)
        .take((size - start).min(maximum))
        .read_to_end(&mut bytes)
        .ok()?;
    *budget = budget.saturating_sub(bytes.len());
    let end = bytes
        .iter()
        .rposition(|b| *b == b'\n')
        .map(|p| p + 1)
        .unwrap_or(0);
    let cut = if skipped {
        bytes[..end]
            .iter()
            .position(|b| *b == b'\n')
            .map(|p| p + 1)
            .unwrap_or(end)
    } else {
        0
    };
    let body = String::from_utf8_lossy(&bytes[cut..end]);
    let mut session = if continuous {
        let mut session = reusable.map(|old| old.session.clone()).unwrap_or_default();
        if family == "codex" {
            append_codex(&mut session, "", &body);
        } else {
            append_claude(&mut session, &body);
        }
        session
    } else if family == "codex" {
        scan_codex(&prefix, &body, skipped)
    } else {
        scan_claude(&body, skipped)
    };
    // Preserve already observed samples across a bounded gap, while refusing to
    // interpolate a cumulative delta over the missing records.
    if !continuous
        && let Some(old) = reusable
        && (old.session.id == session.id || session.id.is_empty())
    {
        if session.id.is_empty() {
            session.id = old.session.id.clone();
        }
        if session.cwd.is_empty() {
            session.cwd = old.session.cwd.clone();
        }
        if family == "codex" {
            let mut preserved = old.session.samples.clone();
            preserved.append(&mut session.samples);
            session.samples = preserved;
        } else {
            for (id, sample) in &old.session.messages {
                session
                    .messages
                    .entry(id.clone())
                    .or_insert_with(|| sample.clone());
            }
            session.samples = session.messages.values().cloned().collect();
            session.samples.sort_by_key(|s| s.at);
        }
    }
    if session.id.is_empty() {
        session.id = format!(
            "{family}:{:x}",
            Sha256::digest(path.as_os_str().to_string_lossy().as_bytes())
        );
    }
    session.incomplete |= session.last_at.is_some_and(|at| at > now + 5);
    session.trim(now);
    let offset = start.checked_add(end as u64)?;
    let final_meta = file.metadata().ok()?;
    if file_identity(&final_meta) != identity || final_meta.len() < size {
        return None;
    }
    Some(CachedFile {
        identity,
        offset,
        guard: guard(&mut file, offset)?,
        pending_record: end < bytes.len(),
        session,
    })
}
fn save_usage(config: &ObserveConfig, files: BTreeMap<String, CachedFile>) -> bool {
    let mut state = UsageState { version: 1, files };
    // Eviction cannot manufacture a complete baseline on the next cycle.
    loop {
        let Ok(value) = serde_json::to_value(&state) else {
            return false;
        };
        let Ok(bytes) = serde_json::to_vec(&value) else {
            return false;
        };
        if bytes.len() <= STATE_BYTES {
            return io::atomic_json(&config.output_dir.join("usage-state-v1.json"), &value).is_ok();
        }
        let Some(key) = state
            .files
            .iter()
            .max_by_key(|(_, f)| f.session.samples.len() + f.session.messages.len())
            .map(|(k, _)| k.clone())
        else {
            return false;
        };
        let Some(file) = state.files.get_mut(&key) else {
            return false;
        };
        if file.session.samples.len() <= 1 && file.session.messages.len() <= 1 {
            state.files.remove(&key);
            continue;
        }
        file.session.samples.drain(..file.session.samples.len() / 2);
        let keep = file.session.samples.iter().map(|s| s.at).min();
        file.session
            .messages
            .retain(|_, s| keep.is_some_and(|at| s.at >= at));
        file.session.truncated = true;
        file.session.coverage_start = keep;
    }
}

fn read_log(path: &Path, budget: &mut usize) -> Option<(String, String, bool)> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(
            (rustix::fs::OFlags::NOFOLLOW
                | rustix::fs::OFlags::NONBLOCK
                | rustix::fs::OFlags::CLOEXEC)
                .bits() as i32,
        );
    }
    let mut file = options.open(path).ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() {
        return None;
    }
    let len = usize::try_from(metadata.len()).ok()?;
    let initial_max = FILE_BYTES.min(*budget);
    let prefix_max = if len > initial_max {
        (32 * 1024).min(*budget / 4)
    } else {
        0
    };
    let max = FILE_BYTES.min(budget.saturating_sub(prefix_max));
    if max == 0 {
        return None;
    }
    let truncated = len > max;
    let mut prefix = String::new();
    if truncated {
        let mut bytes = Vec::new();
        (&mut file)
            .take(prefix_max as u64)
            .read_to_end(&mut bytes)
            .ok()?;
        prefix = String::from_utf8_lossy(&bytes).into_owned();
        file.seek(SeekFrom::Start((len - max) as u64)).ok()?;
    }
    let mut bytes = Vec::new();
    (&mut file).take(max as u64).read_to_end(&mut bytes).ok()?;
    *budget = budget.saturating_sub(bytes.len() + prefix.len());
    let mut tail = String::from_utf8_lossy(&bytes).into_owned();
    if truncated {
        tail = tail
            .split_once('\n')
            .map(|(_, rest)| rest.to_owned())
            .unwrap_or_default();
    }
    // Only complete JSONL records: the writer may currently be appending.
    if !tail.ends_with('\n') {
        tail.truncate(tail.rfind('\n').map(|n| n + 1).unwrap_or(0));
    }
    Some((prefix, tail, truncated))
}

fn scan_codex(prefix: &str, body: &str, truncated: bool) -> Session {
    let mut session = Session {
        family: "codex".into(),
        truncated,
        ..Session::default()
    };
    append_codex(&mut session, prefix, body);
    session
}
fn append_codex(session: &mut Session, prefix: &str, body: &str) {
    // A metadata prefix predates a skipped tail: its model is not evidence for
    // intervening turns. Session identity/cwd remain useful, pricing stays unknown.
    if session.model.is_empty() {
        session.model = "unknown".into();
    }
    for (metadata_only, input) in [(true, prefix), (false, body)] {
        for line in input.lines() {
            let Ok(v) = serde_json::from_str::<Value>(line) else {
                if !metadata_only && !line.trim().is_empty() {
                    session.incomplete = true;
                }
                continue;
            };
            let p = &v["payload"];
            match text(&v, "type") {
                "session_meta" => {
                    if !text(p, "id").is_empty() {
                        session.id = text(p, "id").into();
                    }
                    if !text(p, "cwd").is_empty() {
                        session.cwd = text(p, "cwd").into();
                    }
                }
                "turn_context" => {
                    if !metadata_only && !text(p, "model").is_empty() {
                        session.model = io::clean(text(p, "model"));
                    }
                    if !text(p, "cwd").is_empty() {
                        session.cwd = text(p, "cwd").into();
                    }
                }
                "event_msg" if !metadata_only && text(p, "type") == "token_count" => {
                    let Some(at) = timestamp(&v["timestamp"]) else {
                        continue;
                    };
                    if p["rate_limits"].is_object()
                        && session.quota.as_ref().is_none_or(|(t, _)| at >= *t)
                    {
                        session.quota = Some((at, observed_quota(&p["rate_limits"])));
                    }
                    let Some(total) = Tokens::parse(&p["info"]["total_token_usage"], false) else {
                        if p["info"]["total_token_usage"].is_object() {
                            session.incomplete = true;
                        }
                        continue;
                    };
                    if session.last_at.is_some_and(|last| at < last) {
                        session.incomplete = true;
                    }
                    session.observed_usage = true;
                    let delta = if let Some(old) = session.previous {
                        let (d, reset) = total.difference(old);
                        session.resets += usize::from(reset);
                        Some(d)
                    } else if !session.truncated {
                        Some(total)
                    } else {
                        session.coverage_start = Some(at);
                        None
                    };
                    session.previous = Some(total);
                    session.last_at = Some(at);
                    if let Some(tokens) = delta.filter(|t| t.total() > 0) {
                        session.samples.push(Sample {
                            at,
                            model: session.model.clone(),
                            tokens,
                        });
                    }
                }
                _ => {}
            }
        }
    }
}
fn scan_claude(body: &str, truncated: bool) -> Session {
    let mut session = Session {
        family: "claude".into(),
        truncated,
        ..Session::default()
    };
    append_claude(&mut session, body);
    session
}
fn append_claude(session: &mut Session, body: &str) {
    for line in body.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            if !line.trim().is_empty() {
                session.incomplete = true;
            }
            continue;
        };
        if !text(&v, "sessionId").is_empty() {
            session.id = text(&v, "sessionId").into();
        }
        if !text(&v, "cwd").is_empty() {
            session.cwd = text(&v, "cwd").into();
        }
        let message = &v["message"];
        let id = text(message, "id");
        let (Some(at), Some(tokens)) = (
            timestamp(&v["timestamp"]),
            Tokens::parse(&message["usage"], true),
        ) else {
            continue;
        };
        if id.is_empty() {
            continue;
        }
        session.observed_usage = true;
        if session.truncated && session.coverage_start.is_none() {
            session.coverage_start = Some(at);
        }
        let next = Sample {
            at,
            model: text(message, "model").to_owned(),
            tokens,
        };
        // Streaming updates reuse message.id. Retain one latest usage report.
        if session.messages.get(id).is_none_or(|old| at >= old.at) {
            session.messages.insert(id.into(), next);
        }
    }
    session.samples = session.messages.values().cloned().collect();
    session.samples.sort_by_key(|s| s.at);
}

fn rates(config: &ObserveConfig) -> (BTreeMap<String, Rate>, Value) {
    let Some(root) = &config.rates_dir else {
        return (BTreeMap::new(), Value::Null);
    };
    let mut paths = fs::read_dir(root)
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s.starts_with("codex_rates_") && s.ends_with(".json"))
        })
        .collect::<Vec<_>>();
    paths.sort();
    let Some(path) = paths.last() else {
        return (BTreeMap::new(), Value::Null);
    };
    let Ok(v) = io::read_json(path, 256 * 1024) else {
        return (BTreeMap::new(), Value::Null);
    };
    let mut result = BTreeMap::new();
    for value in v["models"].as_array().into_iter().flatten() {
        let (Some(input), Some(cached), Some(output)) = (
            value["input"].as_f64(),
            value["cached_input"].as_f64(),
            value["output"].as_f64(),
        ) else {
            continue;
        };
        if [input, cached, output]
            .iter()
            .all(|n| n.is_finite() && *n >= 0.0)
            && !text(value, "model").is_empty()
        {
            result.insert(
                text(value, "model").into(),
                Rate {
                    input,
                    cached,
                    output,
                },
            );
        }
    }
    (result, json!(path))
}

#[derive(Default)]
struct Aggregate {
    input: u64,
    cached: u64,
    output: u64,
    sessions: BTreeSet<String>,
    priced: f64,
    unpriced: bool,
    cached_missing: bool,
}
#[cfg(test)]
fn windows(sessions: &[Session], rates: &BTreeMap<String, Rate>, now: i64) -> Value {
    windows_observed(sessions, rates, now, true)
}
fn windows_observed(
    sessions: &[Session],
    rates: &BTreeMap<String, Rate>,
    now: i64,
    source_complete: bool,
) -> Value {
    let mut windows = Map::new();
    let observed = sessions.iter().any(|s| s.observed_usage);
    for minutes in [15_i64, 60] {
        let coverage = observed
            && source_complete
            && sessions.iter().all(|s| s.complete_for(now - minutes * 60));
        let mut models = BTreeMap::<String, Aggregate>::new();
        for s in sessions {
            for sample in s
                .samples
                .iter()
                .filter(|s| s.at > now - minutes * 60 && s.at <= now)
            {
                let a = models.entry(sample.model.clone()).or_default();
                a.input = a.input.saturating_add(sample.tokens.input);
                a.output = a.output.saturating_add(sample.tokens.output);
                a.cached = a.cached.saturating_add(sample.tokens.cached.unwrap_or(0));
                a.cached_missing |= sample.tokens.cached.is_none();
                a.sessions.insert(s.id.clone());
                if let Some(c) = rates
                    .get(&sample.model)
                    .and_then(|r| r.credits(sample.tokens))
                    .filter(|_| s.family == "codex")
                {
                    a.priced += c;
                } else {
                    a.unpriced = true;
                }
            }
        }
        let input = models
            .values()
            .fold(0_u64, |sum, a| sum.saturating_add(a.input));
        let output = models
            .values()
            .fold(0_u64, |sum, a| sum.saturating_add(a.output));
        let priced = models.values().map(|a| a.priced).sum::<f64>();
        let complete = coverage && !models.values().any(|a| a.unpriced);
        let by_model=models.into_iter().map(|(id,a)|(id,json!({"input":coverage.then_some(a.input),"cached":if !coverage || a.cached_missing {Value::Null}else{json!(a.cached)},"output":coverage.then_some(a.output),"sessions":a.sessions.len(),"credits":if !coverage || a.unpriced{Value::Null}else{json!(a.priced)},"lower_bound_input":a.input,"lower_bound_cached":if a.cached_missing {Value::Null}else{json!(a.cached)},"lower_bound_output":a.output,"lower_bound_credits_est":a.priced,"priced_credits_est":a.priced,"unpriced":a.unpriced,"coverage_complete":coverage}))).collect::<Map<_,_>>();
        windows.insert(minutes.to_string(),json!({"by_model":by_model,"input_tokens":coverage.then_some(input),"output_tokens":coverage.then_some(output),"credits_est":complete.then_some(priced),"priced_credits_est":coverage.then_some(priced),"credits_per_hour_est":complete.then_some(priced*60.0/minutes as f64),"lower_bound_input_tokens":input,"lower_bound_output_tokens":output,"lower_bound_credits_est":priced,"lower_bound_credits_per_hour_est":priced*60.0/minutes as f64,"coverage_complete":coverage,"pricing_complete":complete,"incomplete_sessions":sessions.iter().filter(|s|!s.complete_for(now-minutes*60)).count(),"source_inventory_complete":source_complete}));
    }
    Value::Object(windows)
}

fn ledger(config: &ObserveConfig, now: i64) -> Value {
    let mut paths = BTreeSet::new();
    if let Some(root) = &config.coordination_dir {
        paths.insert(root.join("model_usage.jsonl"));
    }
    for project in &config.projects {
        for path in files(&project.repository.join(".agent"), 32) {
            if path.file_name().is_some_and(|n| n == "model_usage.jsonl") {
                paths.insert(path);
            }
        }
    }
    let mut records = BTreeMap::new();
    let mut budget = 8 * 1024 * 1024;
    let mut pending_records = Vec::new();
    for project in &config.projects {
        if let Some(path) = &project.coordination_log {
            if budget == 0 {
                break;
            }
            if let Ok(body) = io::read_text_tail(path, budget.min(256 * 1024)) {
                budget = budget.saturating_sub(body.len());
                pending_records.extend(
                    body.lines()
                        .filter_map(|line| model_line(line, &project.id)),
                );
            }
        }
    }
    for path in paths {
        if let Some((_, body, _)) = read_log(&path, &mut budget) {
            for line in body.lines() {
                let Ok(v) = serde_json::from_str::<Value>(line) else {
                    continue;
                };
                pending_records.push(v);
            }
        }
    }
    for mut v in pending_records {
        let at = timestamp(v.get("ts").or_else(|| v.get("utc")).unwrap_or(&Value::Null));
        if let Some(at) = at.filter(|t| *t <= now) {
            let outcome = text(&v, "outcome").to_ascii_lowercase();
            v["outcome"] = json!(outcome);
            let key = (
                at,
                text(&v, "project").to_owned(),
                text(&v, "task").to_owned(),
                text(&v, "screen").to_owned(),
                text(&v, "model").to_owned(),
                outcome,
            );
            records.entry(key).or_insert(v);
        }
    }
    let mut tasks = BTreeMap::<(String, String), Vec<(i64, Value)>>::new();
    let mut hours = [0_u64; 24];
    let mut failures = 0_u64;
    let mut cumulative = Vec::new();
    let mut outcomes = BTreeMap::<String, u64>::new();
    for ((at, project, task, _, _, _), v) in records {
        *outcomes.entry(text(&v, "outcome").into()).or_default() += 1;
        if is_luna(&v)
            && matches!(
                text(&v, "outcome"),
                "fail" | "loop" | "timeout" | "escalated"
            )
        {
            failures += 1;
            cumulative.push(json!([iso(at), failures]));
            let age = (now - at) / 3600;
            if (0..24).contains(&age) {
                hours[23 - age as usize] += 1;
            }
        }
        if !task.is_empty() {
            tasks.entry((project, task)).or_default().push((at, v));
        }
    }
    let mut first = 0_u64;
    let mut luna_tasks = 0_u64;
    let mut luna_sol = 0;
    let mut sol_astra = 0;
    let mut sol_tasks = 0;
    let mut astra_tasks = 0;
    for task in tasks.values() {
        let judged = task.iter().find(|(_, v)| {
            is_luna(v) && {
                matches!(
                    text(v, "outcome"),
                    "pass" | "fail" | "loop" | "timeout" | "escalated"
                )
            }
        });
        if let Some((_, v)) = judged.filter(|(_, v)| is_luna(v)) {
            luna_tasks += 1;
            if text(v, "outcome") == "pass" {
                first += 1;
            }
        }
        let luna = task.iter().any(|(_, v)| is_luna(v));
        let sol = task.iter().any(|(_, v)| rank(v) == 1);
        let astra = task.iter().any(|(_, v)| rank(v) == 2);
        if luna
            && (sol
                || task
                    .iter()
                    .any(|(_, v)| is_luna(v) && text(v, "outcome") == "escalated"))
        {
            luna_sol += 1;
        }
        if sol {
            sol_tasks += 1;
        }
        if astra {
            astra_tasks += 1;
        }
        if sol && astra {
            sol_astra += 1;
        }
    }
    json!({"model_lines":outcomes.values().sum::<u64>(),"outcomes":outcomes,"escalated":outcomes.get("escalated"),"attempts":outcomes.values().sum::<u64>(),"luna_failures_total":if outcomes.is_empty(){Value::Null}else{json!(failures)},"luna_failures_cum":cumulative.into_iter().rev().take(200).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>(),"luna_failures_per_hour_24h":if outcomes.is_empty(){Value::Null}else{json!(hours)},"luna_first_pass":first,"luna_first_pass_rate":percent(first,luna_tasks),"funnel":{"luna_tasks":if outcomes.is_empty(){Value::Null}else{json!(luna_tasks)},"luna_to_sol":if outcomes.is_empty(){Value::Null}else{json!(luna_sol)},"sol_tasks":sol_tasks,"sol_to_astra":if outcomes.is_empty(){Value::Null}else{json!(sol_astra)},"astra_tasks":astra_tasks},"note":"Deduplicated timestamp/project/task/screen/model/outcome records from configured model_usage.jsonl; own-model T0 is not inferred to be Luna."})
}
fn model_line(line: &str, project: &str) -> Option<Value> {
    let start = line.find("MODEL ")?;
    let parts = line[start..].split('|').map(str::trim).collect::<Vec<_>>();
    if parts.len() < 7 {
        return None;
    }
    Some(
        json!({"ts":parts[0].strip_prefix("MODEL ")?.trim(),"project":project,
        "task":parts[1].strip_prefix("task ")?.trim(),"tier":parts[3].strip_prefix("tier ")?.trim(),
        "model":parts[4].strip_prefix("model ")?.split_whitespace().next()?,
        "screen":parts[5].strip_prefix("screen ")?.trim(),"outcome":parts[6].strip_prefix("outcome ")?.split_whitespace().next()?}),
    )
}
fn is_luna(v: &Value) -> bool {
    text(v, "model").to_ascii_lowercase().contains("luna")
}
fn rank(v: &Value) -> u8 {
    let m = text(v, "model").to_ascii_lowercase();
    if m.contains("astra") {
        2
    } else if m.contains("sol") || m.contains("terra") {
        1
    } else {
        0
    }
}

async fn git(args: Vec<String>, cwd: &Path) -> Option<String> {
    let result = io::run(Path::new("/usr/bin/git"), &args, Some(cwd), 3, 128 * 1024)
        .await
        .ok()?;
    if result.status == 0 {
        Some(result.stdout)
    } else {
        None
    }
}
async fn ancestor(cwd: &Path, sha: &str, reference: String) -> Option<bool> {
    let args = vec![
        "merge-base".into(),
        "--is-ancestor".into(),
        sha.into(),
        reference,
    ];
    let result = io::run(Path::new("/usr/bin/git"), &args, Some(cwd), 3, 128 * 1024)
        .await
        .ok()?;
    match result.status {
        0 => Some(true),
        1 => Some(false),
        _ => None,
    }
}

pub async fn collect(config: &ObserveConfig, snapshot: &Value) -> crate::Result<Value> {
    let now = Utc::now().timestamp();
    let (rates, rates_source) = rates(config);
    let mut sessions = BTreeMap::<String, Session>::new();
    let mut budget = SCAN_BYTES;
    let old = io::read_json(
        &config.output_dir.join("usage-state-v1.json"),
        STATE_BYTES as u64,
    )
    .ok()
    .and_then(|v| serde_json::from_value::<UsageState>(v).ok())
    .filter(|s| s.version == 1)
    .unwrap_or_default();
    let mut next_files = BTreeMap::new();
    let mut sources = BTreeMap::new();
    for (root, family) in [
        (&config.codex_sessions, "codex"),
        (&config.claude_projects, "claude"),
    ] {
        if let Some(root) = root {
            let (paths, mut complete) = inventory(root, FILE_LIMIT, Some(now - WINDOW));
            for path in paths {
                let key = format!(
                    "{family}:{:x}",
                    Sha256::digest(path.as_os_str().to_string_lossy().as_bytes())
                );
                let Some(file) = read_usage(&path, family, old.files.get(&key), &mut budget, now)
                else {
                    complete = false;
                    continue;
                };
                let session = file.session.clone();
                complete &= !file.pending_record;
                next_files.insert(key, file);
                sessions
                    .entry(format!("{}:{}", family, session.id))
                    .and_modify(|old| merge_session(old, session.clone()))
                    .or_insert(session);
            }
            sources.insert(family, complete);
        } else {
            sources.insert(family, false);
        }
    }
    let state_saved = save_usage(config, next_files);
    let sessions = sessions.into_values().collect::<Vec<_>>();
    let codex_sessions = sessions
        .iter()
        .filter(|s| s.family == "codex")
        .cloned()
        .collect::<Vec<_>>();
    let claude_sessions = sessions
        .iter()
        .filter(|s| s.family == "claude")
        .cloned()
        .collect::<Vec<_>>();
    let codex_complete = sources.get("codex").copied().unwrap_or(false);
    let all_complete = (config.codex_sessions.is_some() || config.claude_projects.is_some())
        && (config.codex_sessions.is_none() || sources.get("codex") == Some(&true))
        && (config.claude_projects.is_none() || sources.get("claude") == Some(&true));
    let win = windows_observed(&codex_sessions, &rates, now, codex_complete);
    let other_windows = windows_observed(
        &claude_sessions,
        &BTreeMap::new(),
        now,
        sources.get("claude").copied().unwrap_or(false),
    );
    let stats = ledger(config, now);
    let agents = snapshot["agents"].as_array();
    let mut by_model = BTreeMap::<String, u64>::new();
    let mut by_family = BTreeMap::<String, u64>::new();
    let mut tiers = BTreeMap::<String, u64>::new();
    let mut codex = 0_u64;
    let mut cheap = 0_u64;
    for agent in agents.into_iter().flatten() {
        let family = text(agent, "family");
        let model = text(agent, "model");
        *by_family
            .entry(if family.is_empty() { "unknown" } else { family }.into())
            .or_default() += 1;
        *by_model
            .entry(if model.is_empty() { "unknown" } else { model }.into())
            .or_default() += 1;
        if family == "codex" {
            codex += 1;
            cheap += u64::from(model == "gpt-6-luna");
            *tiers.entry(text(agent, "tier").into()).or_default() += 1;
        }
    }
    let mut quota=sessions.iter().filter_map(|s|s.quota.as_ref()).filter(|(at,_)|*at<=now).max_by_key(|(at,_)|*at).map(|(at,v)|{
        let selected=[&v["primary"],&v["secondary"]].into_iter().filter(|v|v.is_object()).max_by_key(|v|v["window_minutes"].as_u64().unwrap_or(0));
        let p=selected.unwrap_or(&Value::Null);json!({"used_percent":p["used_percent"],"window_minutes":p["window_minutes"],"resets_at_utc":p["resets_at"].as_i64().map(iso),"credits_balance":v["credits"]["balance"],"plan_type":v["plan_type"],"observed_utc":iso(*at),"history":[[iso(*at),p["used_percent"]]],"primary":v["primary"],"secondary":v["secondary"]})
    }).unwrap_or(Value::Null);
    if quota.is_object() {
        merge_quota_history(config, &mut quota, now);
    }
    let baseline = rates.get("gpt-6-luna");
    let mut actual = 0.0;
    let mut luna = 0.0;
    let mut index_known = baseline.is_some() && win["15"]["coverage_complete"] == true;
    let mut token_seen = false;
    for s in &codex_sessions {
        for sample in s.samples.iter().filter(|s| s.at > now - 900 && s.at <= now) {
            token_seen = true;
            match (
                rates
                    .get(&sample.model)
                    .and_then(|r| r.credits(sample.tokens)),
                baseline.and_then(|r| r.credits(sample.tokens)),
            ) {
                (Some(a), Some(b)) if s.family == "codex" => {
                    actual += a;
                    luna += b;
                }
                _ => index_known = false,
            }
        }
    }
    let index = (index_known && token_seen && luna > 0.0).then_some(actual / luna);
    let rates_json = rates
        .iter()
        .map(|(id, r)| {
            (
                id.clone(),
                json!({"input":r.input,"cached":r.cached,"output":r.output}),
            )
        })
        .collect::<Map<_, _>>();
    let efficiency = json!({"agents_total":agents.map(Vec::len),"by_family":by_family,"by_model":by_model,"codex_tiers":tiers,"codex_agents":agents.map(|_|codex),"codex_on_cheapest":agents.map(|_|cheap),"pct_codex_on_cheapest":percent(cheap,codex),"windows":win,"other_provider_windows":{"claude":other_windows},"quota":quota,"ledger_stats":stats,"ledger":stats,"flags":[],"rates_source":rates_source,"rates_per_1m":rates_json,"cost_index_vs_all_luna":index,"cost_index_basis":"measured_tokens_15m","cost_index_note":"Token-weighted configured Codex credit rates, actual last15min. Incomplete windows or unpriced usage make aggregate cost unknown; no own-model price is inferred.","windows_note":"Bounded configured JSONL sources with private persisted offsets/cumulative baselines and48h events. Full totals/cost require a known source start or baseline before each window and complete source enumeration; otherwise null, with observed lower_bound fields separate. Cached input unknown stays unpriced.","scan":{"sessions":sessions.len(),"truncated_files":sessions.iter().filter(|s|s.truncated).count(),"counter_resets":sessions.iter().map(|s|s.resets).sum::<usize>(),"byte_limit":SCAN_BYTES,"limit_reached":budget==0,"source_inventory_complete":sources,"state_persisted":state_saved,"state_byte_limit":STATE_BYTES}});
    let mut yield_value = yield_metrics(config, snapshot, &sessions, &rates, now).await;
    apply_yield_coverage(&mut yield_value, &sessions, config, now, all_complete);
    Ok(json!({"token_efficiency":efficiency,"token_yield":yield_value}))
}

fn merge_quota_history(config: &ObserveConfig, quota: &mut Value, now: i64) {
    let path = config.output_dir.join("quota-history.json");
    let old = io::read_json(&path, 128 * 1024).unwrap_or(Value::Null);
    let mut history = old.as_array().cloned().unwrap_or_default();
    history.extend(quota["history"].as_array().into_iter().flatten().cloned());
    history.retain(|v| {
        timestamp(&v[0]).is_some_and(|at| at <= now && at > now - 12 * 3600)
            && v[1].as_f64().is_some_and(|n| (0.0..=100.0).contains(&n))
    });
    history.sort_by_key(|v| timestamp(&v[0]));
    history.dedup_by(|a, b| a[0] == b[0]);
    if history.len() > 256 {
        history.drain(..history.len() - 256);
    }
    if let (Some(first), Some(last)) = (
        history
            .iter()
            .find(|v| timestamp(&v[0]).is_some_and(|at| now - at >= 1200 && now - at <= 10800)),
        history.last(),
    ) && let (Some(a), Some(b), Some(x), Some(y)) = (
        timestamp(&first[0]),
        timestamp(&last[0]),
        first[1].as_f64(),
        last[1].as_f64(),
    ) && b > a
        && y >= x
    {
        let rate = (y - x) * 3600.0 / (b - a) as f64;
        quota["burn_pct_per_hour"] = json!(rate);
        if rate > 0.0 {
            quota["hours_to_100pct"] = json!((100.0 - y) / rate);
        }
    }
    quota["history"] = json!(history);
    let _ = io::atomic_json(&path, &json!(history));
}

#[derive(Default)]
struct YieldTotal {
    tokens: u64,
    input: u64,
    output: u64,
    cached: u64,
    credits: f64,
    unpriced: bool,
    cached_missing: bool,
    sessions: u64,
    attributed: u64,
    main: u64,
    integration: u64,
    main_credits: f64,
    integration_credits: f64,
}
impl YieldTotal {
    fn value(&self) -> Value {
        json!({"tokens":self.tokens,"input":self.input,"cached":(!self.cached_missing).then_some(self.cached),"output":self.output,"credits":(!self.unpriced).then_some(self.credits),"sessions":self.sessions,"attributed_tokens":self.attributed,"main_tokens":self.main,"integration_tokens":self.integration,"coverage_pct":percent(self.attributed,self.tokens),"yield_main_pct":percent(self.main,self.tokens),"yield_integration_pct":percent(self.integration,self.tokens),"yield_main_credits_pct":(!self.unpriced&&self.credits>0.0).then_some(100.0*self.main_credits/self.credits),"yield_integration_credits_pct":(!self.unpriced&&self.credits>0.0).then_some(100.0*self.integration_credits/self.credits)})
    }
}
async fn yield_metrics(
    config: &ObserveConfig,
    snapshot: &Value,
    sessions: &[Session],
    rates: &BTreeMap<String, Rate>,
    now: i64,
) -> Value {
    let mut totals = BTreeMap::<String, YieldTotal>::new();
    let mut git_budget = 32_usize;
    let git_deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    for session in sessions {
        let samples = session
            .samples
            .iter()
            .filter(|s| s.at > now - WINDOW && s.at <= now)
            .collect::<Vec<_>>();
        if samples.is_empty() {
            continue;
        }
        let project = config
            .projects
            .iter()
            .filter(|p| Path::new(&session.cwd).starts_with(&p.repository))
            .max_by_key(|p| p.repository.components().count());
        let id = project.map(|p| p.id.as_str()).unwrap_or("other");
        let a = totals.entry(id.into()).or_default();
        let count = samples
            .iter()
            .fold(0_u64, |sum, s| sum.saturating_add(s.tokens.total()));
        a.tokens = a.tokens.saturating_add(count);
        a.sessions += 1;
        let mut session_credits = 0.0;
        for sample in &samples {
            a.input = a.input.saturating_add(sample.tokens.input);
            a.cached = a.cached.saturating_add(sample.tokens.cached.unwrap_or(0));
            a.cached_missing |= sample.tokens.cached.is_none();
            a.output = a.output.saturating_add(sample.tokens.output);
            if let Some(c) = rates
                .get(&sample.model)
                .and_then(|r| r.credits(sample.tokens))
                .filter(|_| session.family == "codex")
            {
                a.credits += c;
                session_credits += c;
            } else {
                a.unpriced = true;
            }
        }
        let Some(project) = project else { continue };
        if git_budget == 0 || std::time::Instant::now() >= git_deadline {
            continue;
        }
        let start = samples.first().map(|s| s.at).unwrap_or(now);
        let end = samples.last().map(|s| s.at).unwrap_or(now);
        let cwd = Path::new(&session.cwd);
        let args = vec![
            "log".into(),
            "--no-merges".into(),
            "--max-count=4".into(),
            "--format=%H%x09%at%x09%(trailers:key=Agent-Session,valueonly,separator=%x2C)".into(),
            format!("--since=@{}", start.saturating_sub(60)),
            format!("--until=@{}", end + 300),
            "HEAD".into(),
            "--".into(),
        ];
        git_budget -= 1;
        let Some(log) = git(args, cwd).await else {
            continue;
        };
        let commits = log
            .lines()
            .filter_map(|line| {
                let parts = line.split('\t').collect::<Vec<_>>();
                let sha = *parts.first()?;
                if sha.len() != 40 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return None;
                }
                let trailer = parts.get(2).copied().unwrap_or("");
                if !trailer.is_empty() && !trailer.split(',').any(|id| id.trim() == session.id) {
                    return None;
                }
                Some(sha.to_owned())
            })
            .collect::<Vec<_>>();
        if commits.is_empty() {
            continue;
        }
        let branch = snapshot["projects"][&project.id]["pr"]["branch"].as_str();
        let mut main = false;
        let mut integration = false;
        let mut reference_observed = false;
        for sha in commits {
            for (name, kind) in [
                (Some("refs/remotes/origin/main".to_owned()), 0),
                (
                    branch
                        .filter(|b| !b.starts_with('-') && !b.contains(char::is_whitespace))
                        .map(|b| format!("refs/remotes/origin/{b}")),
                    1,
                ),
            ] {
                let Some(name) = name else { continue };
                if git_budget == 0 || std::time::Instant::now() >= git_deadline {
                    break;
                }
                git_budget -= 1;
                if let Some(landed) = ancestor(cwd, &sha, name).await {
                    reference_observed = true;
                    if landed {
                        if kind == 0 {
                            main = true;
                        } else {
                            integration = true;
                        }
                    }
                }
            }
        }
        if reference_observed {
            a.attributed = a.attributed.saturating_add(count);
        }
        if main {
            a.main = a.main.saturating_add(count);
            a.main_credits += session_credits;
        }
        if main || integration {
            a.integration = a.integration.saturating_add(count);
            a.integration_credits += session_credits;
        }
    }
    let mut total = YieldTotal::default();
    for a in totals.values() {
        total.tokens = total.tokens.saturating_add(a.tokens);
        total.input = total.input.saturating_add(a.input);
        total.cached = total.cached.saturating_add(a.cached);
        total.output = total.output.saturating_add(a.output);
        total.credits += a.credits;
        total.unpriced |= a.unpriced;
        total.cached_missing |= a.cached_missing;
        total.sessions += a.sessions;
        total.attributed = total.attributed.saturating_add(a.attributed);
        total.main = total.main.saturating_add(a.main);
        total.integration = total.integration.saturating_add(a.integration);
        total.main_credits += a.main_credits;
        total.integration_credits += a.integration_credits;
    }
    let projects = totals
        .into_iter()
        .map(|(id, a)| (id, a.value()))
        .collect::<Map<_, _>>();
    json!({"generated_utc":crate::now_utc(),"window_hours":48,"projects":projects,"total":if total.sessions==0{Value::Null}else{total.value()},"hosts_covered":[config.node_id],"method":"Bounded actual JSONL token deltas; project by configured repository/cwd. Yield is an ESTIMATE: a session receives token attribution if a commit in its observed activity window is reachable from that worktree HEAD; exact Agent-Session trailers take precedence. Main/integration require successful git merge-base --is-ancestor against local origin refs (never fetched). Missing or exhausted Git coverage is unattributed, not accepted completion. No patch-id equivalence or remote usage inferred.","attribution_command_budget":32,"attribution_budget_remaining":git_budget,"not_covered":["Unconfigured/remote session files","Providers without measured usage","Worktrees outside configured repository paths","Patch-equivalent rebases without ancestry"]})
}

fn apply_yield_coverage(
    value: &mut Value,
    sessions: &[Session],
    config: &ObserveConfig,
    now: i64,
    source_complete: bool,
) {
    fn mask(row: &mut Value, complete: bool) {
        if !row.is_object() {
            *row = json!({});
        }
        for key in [
            "tokens",
            "input",
            "cached",
            "output",
            "credits",
            "attributed_tokens",
            "main_tokens",
            "integration_tokens",
        ] {
            let observed = row.get(key).cloned().unwrap_or(Value::Null);
            row[format!("lower_bound_{key}")] = observed;
            if !complete {
                row[key] = Value::Null;
            }
        }
        if !complete {
            for key in [
                "coverage_pct",
                "yield_main_pct",
                "yield_integration_pct",
                "yield_main_credits_pct",
                "yield_integration_credits_pct",
            ] {
                row[key] = Value::Null;
            }
        }
        row["usage_coverage_complete"] = json!(complete);
    }
    let complete = source_complete
        && sessions.iter().any(|s| s.observed_usage)
        && sessions.iter().all(|s| s.complete_for(now - WINDOW));
    mask(&mut value["total"], complete);
    for session in sessions {
        let id = config
            .projects
            .iter()
            .filter(|p| Path::new(&session.cwd).starts_with(&p.repository))
            .max_by_key(|p| p.repository.components().count())
            .map(|p| p.id.as_str())
            .unwrap_or("other");
        if !value["projects"][id].is_object() {
            value["projects"][id] = json!({});
        }
    }
    if let Some(projects) = value["projects"].as_object_mut() {
        for (id, row) in projects {
            let matching = sessions
                .iter()
                .filter(|session| {
                    config
                        .projects
                        .iter()
                        .filter(|p| Path::new(&session.cwd).starts_with(&p.repository))
                        .max_by_key(|p| p.repository.components().count())
                        .map(|p| p.id.as_str())
                        .unwrap_or("other")
                        == id
                })
                .collect::<Vec<_>>();
            mask(
                row,
                source_complete
                    && matching.iter().any(|s| s.observed_usage)
                    && matching.iter().all(|s| s.complete_for(now - WINDOW)),
            );
        }
    }
    value["usage_coverage_complete"] = json!(complete);
    value["source_inventory_complete"] = json!(source_complete);
    value["usage_coverage_note"] = json!(
        "Full48h totals and yield percentages require retained coverage for every configured source/session; unknown/truncated coverage leaves them null. lower_bound fields contain observed events only. Historical totals are not fresh runtime qualification or task acceptance."
    );
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    fn config(root: &Path) -> ObserveConfig {
        serde_json::from_value(json!({"schema_version":1,"node_id":"observed-node","output_dir":root,"projects":[],"nodes":[],"coordination_dir":null,"rollout_manifest":null,"codex_sessions":null,"claude_projects":null,"rates_dir":null,"history_source":null,"admiral_screen":null,"gh_program":null})).expect("configured fixture")
    }
    fn event(at: &str, input: u64, cached: u64, output: u64) -> Value {
        json!({"type":"event_msg","timestamp":at,"payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":input,"cached_input_tokens":cached,"output_tokens":output}}}})
    }
    #[test]
    fn codex_counters_deduplicate_and_handle_resets_and_model_changes() {
        let mut lines = vec![
            json!({"type":"session_meta","payload":{"id":"s1","cwd":"/repo"}}),
            json!({"type":"turn_context","payload":{"model":"priced"}}),
            event("2026-10-18T20:00:00Z", 100, 50, 10),
            event("2026-10-18T20:01:00Z", 100, 50, 10),
            event("2026-10-18T20:02:00Z", 120, 60, 15),
            event("2026-10-18T20:03:00Z", 5, 0, 2),
        ];
        lines.push(json!({"type":"turn_context","payload":{"model":"own"}}));
        lines.push(event("2026-10-18T20:04:00Z", 8, 1, 4));
        let body = lines
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        let session = scan_codex("", &body, false);
        assert_eq!(session.samples.len(), 4);
        assert_eq!(session.resets, 1);
        assert_eq!(session.samples[1].tokens.input, 20);
        assert_eq!(session.samples[3].model, "own");
        let rates = BTreeMap::from([(
            "priced".into(),
            Rate {
                input: 10.0,
                cached: 1.0,
                output: 20.0,
            },
        )]);
        let now = timestamp(&json!("2026-10-18T20:05:00Z")).unwrap();
        let win = windows(&[session], &rates, now);
        assert!(win["15"]["credits_est"].is_null());
        assert_eq!(win["15"]["input_tokens"], 128);
        assert!(win["15"]["by_model"]["own"]["credits"].is_null());
    }
    #[test]
    fn claude_streaming_message_updates_are_not_added_twice() {
        let lines = [
            json!({"sessionId":"s","timestamp":"2026-10-18T20:00:00Z","message":{"id":"m","model":"claude","usage":{"input_tokens":10,"output_tokens":1,"cache_read_input_tokens":2}}}),
            json!({"sessionId":"s","timestamp":"2026-10-18T20:00:01Z","message":{"id":"m","model":"claude","usage":{"input_tokens":10,"output_tokens":4,"cache_read_input_tokens":2}}}),
        ];
        let session = scan_claude(
            &lines
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
            false,
        );
        assert_eq!(session.samples.len(), 1);
        assert_eq!(session.samples[0].tokens.input, 12);
        assert_eq!(session.samples[0].tokens.output, 4);
    }
    #[test]
    fn missing_cached_usage_has_unknown_cost_and_truncated_log_needs_baseline() {
        assert!(
            Rate {
                input: 1.0,
                cached: 0.1,
                output: 2.0
            }
            .credits(Tokens {
                input: 10,
                cached: None,
                output: 2
            })
            .is_none()
        );
        let body = [
            event("2026-10-18T20:00:00Z", 100, 50, 10),
            event("2026-10-18T20:01:00Z", 120, 60, 15),
        ]
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n");
        let s = scan_codex("", &body, true);
        assert_eq!(s.samples.len(), 1);
        assert_eq!(s.samples[0].tokens.input, 20);
        assert!(!is_luna(&json!({"tier":"T0","model":"ornith"})));
    }
    #[test]
    fn real_windows_use_event_timestamps_and_configured_token_weighted_cost() {
        let now = 10_000;
        let rates = BTreeMap::from([(
            "model".into(),
            Rate {
                input: 10.0,
                cached: 1.0,
                output: 20.0,
            },
        )]);
        let session = Session {
            id: "s".into(),
            family: "codex".into(),
            observed_usage: true,
            samples: vec![
                Sample {
                    at: now - 3601,
                    model: "model".into(),
                    tokens: Tokens {
                        input: 10,
                        cached: Some(0),
                        output: 0,
                    },
                },
                Sample {
                    at: now - 1000,
                    model: "model".into(),
                    tokens: Tokens {
                        input: 20,
                        cached: Some(0),
                        output: 2,
                    },
                },
                Sample {
                    at: now - 100,
                    model: "model".into(),
                    tokens: Tokens {
                        input: 100,
                        cached: Some(50),
                        output: 10,
                    },
                },
            ],
            ..Session::default()
        };
        let value = windows(&[session], &rates, now);
        assert_eq!(value["15"]["input_tokens"], 100);
        assert_eq!(value["60"]["input_tokens"], 120);
        assert!((value["15"]["credits_est"].as_f64().unwrap() - 0.00075).abs() < 1e-12);
        assert_eq!(value["15"]["by_model"]["model"]["cached"], 50);
        assert!(windows(&[], &rates, now)["15"]["credits_est"].is_null());
    }
    #[tokio::test]
    async fn truncated_large_log_with_one_counter_never_becomes_a_false_zero_window() {
        let temp = tempfile::tempdir().unwrap();
        let logs = temp.path().join("logs");
        fs::create_dir(&logs).unwrap();
        let mut c = config(temp.path());
        c.codex_sessions = Some(logs.clone());
        let now = Utc::now().timestamp();
        let stamp = |at| DateTime::<Utc>::from_timestamp(at, 0).unwrap().to_rfc3339();
        let mut body =
            json!({"type":"session_meta","payload":{"id":"large-session"}}).to_string() + "\n";
        body.push_str(
            &(json!({"type":"turn_context","payload":{"model":"gpt-6-luna"}}).to_string() + "\n"),
        );
        body.push_str(
            &(json!({"type":"response_item","payload":{"text":"x".repeat(FILE_BYTES+1024)}})
                .to_string()
                + "\n"),
        );
        body.push_str(&(event(&stamp(now - 10), 900_000, 450_000, 90_000).to_string() + "\n"));
        fs::write(logs.join("large.jsonl"), body).unwrap();
        fs::write(
            logs.join("other.jsonl"),
            json!({"type":"session_meta","payload":{"id":"old-session"}}).to_string()
                + "\n"
                + &event(&stamp(now - 7200), 100, 50, 10).to_string()
                + "\n",
        )
        .unwrap();
        let value = collect(&c, &json!({"agents":[]})).await.unwrap();
        for minutes in ["15", "60"] {
            let window = &value["token_efficiency"]["windows"][minutes];
            assert_eq!(window["coverage_complete"], false);
            assert!(window["input_tokens"].is_null());
            assert!(window["credits_est"].is_null());
            assert!(window["credits_per_hour_est"].is_null());
            assert_eq!(window["lower_bound_input_tokens"], 0);
        }
        assert!(value["token_efficiency"]["cost_index_vs_all_luna"].is_null());
        assert_eq!(value["token_yield"]["usage_coverage_complete"], false);
        assert!(value["token_yield"]["total"]["tokens"].is_null());
        assert_eq!(value["token_yield"]["total"]["lower_bound_tokens"], 110);
        // A retained baseline can qualify a later15min window, while60min is
        // still unknown. An unrelated old session cannot change that boundary.
        let parsed = scan_codex(
            "",
            &(event(&stamp(now - 1000), 100, 0, 1).to_string()
                + "\n"
                + &event(&stamp(now - 10), 150, 0, 3).to_string()),
            true,
        );
        let windows = windows(&[parsed], &BTreeMap::new(), now);
        assert_eq!(windows["15"]["coverage_complete"], true);
        assert_eq!(windows["15"]["input_tokens"], 50);
        assert_eq!(windows["60"]["coverage_complete"], false);
    }
    #[tokio::test]
    async fn persisted_offsets_preserve_known_windows_after_a_log_outgrows_the_tail_limit() {
        use std::io::Write;
        let temp = tempfile::tempdir().unwrap();
        let logs = temp.path().join("logs");
        fs::create_dir(&logs).unwrap();
        let path = logs.join("incremental.jsonl");
        let mut c = config(temp.path());
        c.codex_sessions = Some(logs);
        let now = Utc::now().timestamp();
        let stamp = |at| DateTime::<Utc>::from_timestamp(at, 0).unwrap().to_rfc3339();
        fs::write(
            &path,
            json!({"type":"session_meta","payload":{"id":"incremental"}}).to_string()
                + "\n"
                + &json!({"type":"turn_context","payload":{"model":"gpt-6-luna"}}).to_string()
                + "\n"
                + &event(&stamp(now - 1000), 100, 50, 10).to_string()
                + "\n",
        )
        .unwrap();
        collect(&c, &json!({"agents":[]})).await.unwrap();
        let padding =
            json!({"type":"response_item","payload":{"text":"p".repeat(FILE_BYTES/2+4096)}})
                .to_string()
                + "\n";
        for (at, input, cached, output) in [(now - 100, 120, 60, 12), (now - 10, 150, 75, 15)] {
            let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
            file.write_all(padding.as_bytes()).unwrap();
            writeln!(file, "{}", event(&stamp(at), input, cached, output)).unwrap();
            drop(file);
            collect(&c, &json!({"agents":[]})).await.unwrap();
        }
        assert!(fs::metadata(&path).unwrap().len() > FILE_BYTES as u64);
        let after_restart = collect(&c, &json!({"agents":[]})).await.unwrap();
        let win = &after_restart["token_efficiency"]["windows"];
        assert_eq!(win["15"]["coverage_complete"], true);
        assert_eq!(win["15"]["input_tokens"], 50);
        assert_eq!(win["60"]["input_tokens"], 150);
        assert_eq!(after_restart["token_yield"]["total"]["tokens"], 165);
        assert_eq!(
            after_restart["token_yield"]["usage_coverage_complete"],
            true
        );
        // Replacing a previously consumed file invalidates its cached counter;
        // the former cumulative baseline must never be reused.
        fs::write(
            &path,
            json!({"type":"session_meta","payload":{"id":"replacement"}}).to_string()
                + "\n"
                + &event(&stamp(now - 5), 7, 0, 1).to_string()
                + "\n",
        )
        .unwrap();
        let replaced = collect(&c, &json!({"agents":[]})).await.unwrap();
        assert_eq!(
            replaced["token_efficiency"]["windows"]["15"]["input_tokens"],
            7
        );
    }
    #[test]
    fn source_or_retention_gaps_null_full_totals_but_preserve_observed_lower_bounds() {
        let now = 10_000;
        let body = event("1970-01-01T02:45:00Z", 100, 50, 10).to_string();
        let session = scan_codex("", &body, false);
        let unknown =
            windows_observed(std::slice::from_ref(&session), &BTreeMap::new(), now, false);
        assert!(unknown["15"]["input_tokens"].is_null());
        assert_eq!(unknown["15"]["lower_bound_input_tokens"], 100);
        let mut bounded = session;
        bounded.samples = (0..SESSION_SAMPLES + 1)
            .map(|i| Sample {
                at: now - 2000 + (i / 3) as i64,
                model: "unknown".into(),
                tokens: Tokens {
                    input: 1,
                    cached: Some(0),
                    output: 0,
                },
            })
            .collect();
        bounded.trim(now);
        assert_eq!(bounded.samples.len(), SESSION_SAMPLES);
        assert!(!bounded.complete_for(now - 3600));
        assert!(bounded.complete_for(now - 900));
    }
    #[test]
    fn stale_copies_prefer_observed_superset_and_divergent_copies_do_not_claim_completeness() {
        let first = event("1970-01-01T02:43:20Z", 100, 50, 10).to_string();
        let second = event("1970-01-01T02:45:00Z", 150, 75, 15).to_string();
        let stale = scan_codex("", &first, false);
        let mut actual = scan_codex("", &(first + "\n" + &second), false);
        merge_session(&mut actual, stale.clone());
        assert_eq!(
            windows(&[actual.clone()], &BTreeMap::new(), 10_000)["15"]["input_tokens"],
            150
        );
        let conflicting = scan_codex(
            "",
            &event("1970-01-01T02:45:30Z", 200, 100, 20).to_string(),
            false,
        );
        merge_session(&mut actual, conflicting);
        assert!(windows(&[actual], &BTreeMap::new(), 10_000)["15"]["input_tokens"].is_null());
        let sanitized = observed_quota(
            &json!({"primary":{"used_percent":50,"window_minutes":10080,"resets_at":20000,"token":"secret"},"credits":{"balance":"9961","credential":"secret"},"prompt":"secret"}),
        );
        assert!(!sanitized.to_string().contains("secret"));
        assert_eq!(sanitized["credits"]["balance"], 9961.0);
    }
    #[tokio::test]
    async fn repeated_collects_and_copied_codex_sessions_do_not_double_bill() {
        let temp = tempfile::tempdir().unwrap();
        let logs = temp.path().join("logs");
        let rate_dir = temp.path().join("rates");
        fs::create_dir(&logs).unwrap();
        fs::create_dir(&rate_dir).unwrap();
        let mut c = config(temp.path());
        c.codex_sessions = Some(logs.clone());
        c.rates_dir = Some(rate_dir.clone());
        fs::write(rate_dir.join("codex_rates_fixture.json"),json!({"models":[{"model":"gpt-6-luna","input":2.5,"cached_input":0.25,"output":12.5}]}).to_string()).unwrap();
        let at = Utc::now().to_rfc3339();
        let body = [
            json!({"type":"session_meta","payload":{"id":"same-session"}}),
            json!({"type":"turn_context","payload":{"model":"gpt-6-luna"}}),
            event(&at, 100, 50, 10),
        ]
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
            + "\n";
        fs::write(logs.join("one.jsonl"), &body).unwrap();
        fs::write(logs.join("copy.jsonl"), body).unwrap();
        let first = collect(&c, &json!({"agents":[]})).await.unwrap();
        let second = collect(&c, &json!({"agents":[]})).await.unwrap();
        assert_eq!(
            first["token_efficiency"]["windows"]["15"]["input_tokens"],
            100
        );
        assert_eq!(
            first["token_efficiency"]["windows"],
            second["token_efficiency"]["windows"]
        );
        assert_eq!(first["token_efficiency"]["scan"]["sessions"], 1);
        assert_eq!(first["token_efficiency"]["cost_index_vs_all_luna"], 1.0);
        assert!(first["token_efficiency"]["quota"].is_null());
        assert_eq!(first["token_yield"]["total"]["coverage_pct"], 0.0);
    }
    #[test]
    fn model_usage_records_deduplicate_across_model_lines_and_jsonl() {
        let temp = tempfile::tempdir().unwrap();
        let mut c = config(temp.path());
        c.coordination_dir = Some(temp.path().to_owned());
        let v=model_line("MODEL 2026-10-18T20:00:00Z | task T1 | class simple | tier T0 | model gpt-6-luna medium | screen worker | outcome pass","p").unwrap();
        let body = format!("{}\n{}\n", v, v);
        fs::write(temp.path().join("model_usage.jsonl"), body).unwrap();
        let stats = ledger(&c, timestamp(&json!("2026-10-18T20:05:00Z")).unwrap());
        assert_eq!(stats["attempts"], 1);
        assert_eq!(stats["luna_first_pass_rate"], 100.0);
        assert_eq!(stats["luna_failures_total"], 0);
    }
    #[tokio::test]
    async fn token_yield_requires_real_git_ancestry_and_never_claims_acceptance() {
        let temp = tempfile::tempdir().unwrap();
        let repository = temp.path().join("repo");
        fs::create_dir(&repository).unwrap();
        for args in [
            vec!["init", "-q"],
            vec!["config", "user.name", "Fixture"],
            vec!["config", "user.email", "fixture@example.invalid"],
        ] {
            git(args.into_iter().map(str::to_owned).collect(), &repository)
                .await
                .expect("isolated git fixture");
        }
        fs::write(repository.join("file.txt"), "actual candidate fixture\n").unwrap();
        git(
            vec!["add".into(), "--".into(), "file.txt".into()],
            &repository,
        )
        .await
        .unwrap();
        git(
            vec![
                "commit".into(),
                "-q".into(),
                "-m".into(),
                "Actual source fixture".into(),
            ],
            &repository,
        )
        .await
        .unwrap();
        git(
            vec![
                "update-ref".into(),
                "refs/remotes/origin/main".into(),
                "HEAD".into(),
            ],
            &repository,
        )
        .await
        .unwrap();
        let mut c = config(temp.path());
        c.projects=serde_json::from_value(json!([{"id":"p","name":"Project","repository":repository,"tracker":{},"manager_screen":null,"worker_prefixes":[],"coordination_log":null}])).unwrap();
        let now = Utc::now().timestamp();
        let session = Session {
            id: "observed-session".into(),
            family: "codex".into(),
            cwd: repository.to_string_lossy().into_owned(),
            samples: vec![Sample {
                at: now,
                model: "priced".into(),
                tokens: Tokens {
                    input: 100,
                    cached: Some(0),
                    output: 10,
                },
            }],
            ..Session::default()
        };
        let rates = BTreeMap::from([(
            "priced".into(),
            Rate {
                input: 1.0,
                cached: 0.1,
                output: 2.0,
            },
        )]);
        let value = yield_metrics(
            &c,
            &json!({"projects":{"p":{}}}),
            std::slice::from_ref(&session),
            &rates,
            now,
        )
        .await;
        assert_eq!(value["total"]["main_tokens"], 110);
        assert_eq!(value["total"]["integration_tokens"], 110);
        assert_eq!(value["total"]["coverage_pct"], 100.0);
        assert!(value["total"].get("accepted").is_none());
        assert_eq!(value["total"]["yield_main_credits_pct"], 100.0);
        git(
            vec![
                "update-ref".into(),
                "-d".into(),
                "refs/remotes/origin/main".into(),
            ],
            &repository,
        )
        .await
        .unwrap();
        let unknown =
            yield_metrics(&c, &json!({"projects":{"p":{}}}), &[session], &rates, now).await;
        assert_eq!(unknown["total"]["coverage_pct"], 0.0);
        assert_eq!(unknown["total"]["main_tokens"], 0);
    }
}
