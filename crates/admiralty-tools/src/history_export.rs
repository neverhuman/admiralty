//! Export the captains dashboard's running history into a sanitized,
//! git-friendly dataset.
//!
//! Port of the retired `tools/export_history.py`. Reads (read-only) the live
//! runtime files of the captains dashboard and writes JSONL/CSV under `<out>/`
//! (default `<repo>/data/history`). Append-style datasets are union-merged with
//! what is already in `<out>` because several sources are rolling windows.
//! Re-running is idempotent. Never exports raw prompts, screen hardcopies,
//! credentials or message ids. Absolute home paths are rewritten to `~/`. A
//! secret-pattern scan runs over every output file; any hit aborts with exit
//! code 3 and nothing should be committed.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;
use sha2::{Digest, Sha256};

use crate::error::{ToolError, ToolResult};
use crate::fsx;
use crate::pycsv;
use crate::pyjson::{
    self, DumpOptions, LoadOptions, Map, OrderedDict, Value, py_hash_key, py_tuple_key,
};
use crate::pytime;

/// Projects exported per row.
pub const PROJECTS: [&str; 4] = ["joil", "dope", "jopedime", "jarambit"];
/// Hosts exported per row.
pub const HOSTS: [&str; 3] = ["xbabe1", "xbabe2", "xbabe3"];
/// Agent tiers exported per row.
pub const TIERS: [&str; 6] = ["luna", "sol", "astra", "claude", "agy", "other"];
const STAGES: [&str; 7] = [
    "done",
    "on_main",
    "in_pr",
    "implemented",
    "in_progress",
    "blocked",
    "not_started",
];
const LEDGER_FIELDS: [&str; 12] = [
    "ts",
    "project",
    "task",
    "screen",
    "host",
    "tier",
    "model",
    "effort",
    "outcome",
    "escalated_to",
    "reason",
    "tokens",
];

/// Secret patterns: (Python source text shown in hits, Rust equivalent).
/// Only pattern 8 differs: Python's `["\']` class is `["']` in Rust syntax.
pub const SECRET_PATTERNS: [(&str, &str); 10] = [
    (r"gh[pousr]_[A-Za-z0-9]{20,}", r"gh[pousr]_[A-Za-z0-9]{20,}"),
    (
        r"github_pat_[A-Za-z0-9_]{20,}",
        r"github_pat_[A-Za-z0-9_]{20,}",
    ),
    (r"\bsk-[A-Za-z0-9_\-]{20,}", r"\bsk-[A-Za-z0-9_\-]{20,}"),
    (r"\bAKIA[0-9A-Z]{16}\b", r"\bAKIA[0-9A-Z]{16}\b"),
    (
        r"\bxox[abprs]-[A-Za-z0-9\-]{10,}",
        r"\bxox[abprs]-[A-Za-z0-9\-]{10,}",
    ),
    (
        r"-----BEGIN [A-Z ]*PRIVATE KEY-----",
        r"-----BEGIN [A-Z ]*PRIVATE KEY-----",
    ),
    (r"\bAIza[0-9A-Za-z_\-]{35}\b", r"\bAIza[0-9A-Za-z_\-]{35}\b"),
    (
        r#"(?i)\b(api[_-]?key|secret|passwd|password|bearer)\b["\']?\s*[:=]\s*["\']?[A-Za-z0-9/+_\-]{16,}"#,
        r#"(?i)\b(api[_-]?key|secret|passwd|password|bearer)\b["']?\s*[:=]\s*["']?[A-Za-z0-9/+_\-]{16,}"#,
    ),
    (
        r"\beyJ[A-Za-z0-9_\-]{20,}\.[A-Za-z0-9_\-]{20,}\.",
        r"\beyJ[A-Za-z0-9_\-]{20,}\.[A-Za-z0-9_\-]{20,}\.",
    ),
    (
        r"(?i)authorization:\s*\S{12,}",
        r"(?i)authorization:\s*\S{12,}",
    ),
];

static SECRET_RES: LazyLock<Vec<(String, Regex)>> = LazyLock::new(|| {
    SECRET_PATTERNS
        .iter()
        .map(|(python, rust)| {
            (
                python.chars().take(40).collect(),
                Regex::new(rust).expect("static secret pattern"),
            )
        })
        .collect()
});

/// Exporter inputs (Python `argparse` values plus the module globals).
#[derive(Clone, Debug)]
pub struct ExportOptions {
    /// `--out`.
    pub out: PathBuf,
    /// `--dash` (dashboard runtime).
    pub dash: PathBuf,
    /// `--capd` (captains runtime).
    pub capd: PathBuf,
    /// `--check`: only run the secret scan.
    pub check: bool,
    /// Repository root (`REPO`) holding `captains/rates/`.
    pub repo: PathBuf,
    /// `HOME` used for path rewriting and the scan.
    pub home: String,
}

impl ExportOptions {
    /// Python defaults for a repository and home: `<repo>/data/history`,
    /// `$CDASH_RUNTIME` or `<home>/xbabe2_kicker/captains-dashboard`, and
    /// `<home>/xbabe2_kicker/captains-20261017`.
    pub fn defaults(repo: PathBuf, home: String) -> Self {
        let dash = std::env::var_os("CDASH_RUNTIME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(format!("{home}/xbabe2_kicker/captains-dashboard")));
        Self {
            out: repo.join("data").join("history"),
            dash,
            capd: PathBuf::from(format!("{home}/xbabe2_kicker/captains-20261017")),
            check: false,
            repo,
            home,
        }
    }
}

fn iso(timestamp: &Value) -> ToolResult<String> {
    let seconds = pyjson::py_float(timestamp)?;
    Ok(pytime::strftime_z(&pytime::fromtimestamp_utc(seconds)?))
}

fn parse_iso(value: &Value) -> Option<f64> {
    let text = pyjson::py_str(value).replace('Z', "+00:00");
    pytime::fromisoformat(&text)?.timestamp()
}

/// Recursively rewrite home paths; cap long strings at 400 characters.
pub fn clean(value: &Value, home: &str) -> Value {
    match value {
        Value::Str(text) => {
            let rewritten = text.replace(&format!("{home}/"), "~/").replace(home, "~");
            Value::Str(rewritten.chars().take(400).collect())
        }
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, item)| (key.clone(), clean(item, home)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(|item| clean(item, home)).collect()),
        other => other.clone(),
    }
}

fn lenient() -> LoadOptions {
    LoadOptions::default()
}

fn read_text(path: &Path) -> io::Result<String> {
    let data = fs::read(path)?;
    let text = String::from_utf8(data)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    Ok(pyjson::universal_newlines(&text))
}

/// `rj(path, default)`: any failure yields `None`.
fn rj(path: &Path) -> Option<Value> {
    let text = read_text(path).ok()?;
    pyjson::loads(&text, lenient()).ok()
}

fn project_of(cwd: &Value) -> ToolResult<String> {
    let lowered = if cwd.truthy() {
        match cwd {
            Value::Str(text) => text.to_lowercase(),
            other => {
                return Err(ToolError::type_error(format!(
                    "'{}' object has no attribute 'lower'",
                    other.type_name()
                )));
            }
        }
    } else {
        String::new()
    };
    for (key, project) in [
        ("/joil", "joil"),
        ("/dope", "dope"),
        ("jopedime", "jopedime"),
        ("jope-dime", "jopedime"),
        ("/jarambit", "jarambit"),
    ] {
        if lowered.contains(key) {
            return Ok(project.to_owned());
        }
    }
    Ok("other".to_owned())
}

fn py_or(left: &Value, right: &Value) -> Value {
    if left.truthy() {
        left.clone()
    } else {
        right.clone()
    }
}

// ---------------------------------------------------------------- merge helpers

fn read_jsonl_dir(directory: &Path, prefix: &str) -> ToolResult<Vec<Value>> {
    let mut rows = Vec::new();
    for file in fsx::glob_sorted(directory, &format!("{prefix}_*.jsonl")) {
        let text = read_text(&file)?;
        for line in text.split_terminator('\n') {
            let line = pyjson::py_strip(line);
            if !line.is_empty()
                && let Ok(value) = pyjson::loads(line, lenient())
            {
                rows.push(value);
            }
        }
    }
    Ok(rows)
}

fn compact_sorted(value: &Value) -> ToolResult<String> {
    pyjson::dumps(value, &DumpOptions::PYTHON.sorted().separators(",", ":"))
}

fn write_jsonl_by_day(
    directory: &Path,
    prefix: &str,
    rows: &[Value],
    key: &str,
) -> ToolResult<usize> {
    fs::create_dir_all(directory)?;
    let mut by_day: OrderedDict<(String, Vec<&Value>)> = OrderedDict::new();
    for row in rows {
        let stamp = row.index_key(key)?;
        let day = match pyjson::py_prefix(stamp, 10)? {
            Value::Str(day) => day,
            other => {
                return Err(ToolError::type_error(format!(
                    "unhashable type: '{}'",
                    other.type_name()
                )));
            }
        };
        match by_day.get_mut(&day) {
            Some((_, items)) => items.push(row),
            None => by_day.insert(day.clone(), (day, vec![row])),
        }
    }
    for file in fsx::glob_sorted(directory, &format!("{prefix}_*.jsonl")) {
        let name = file
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let characters: Vec<char> = name.chars().collect();
        let start = prefix.chars().count() + 1;
        let end = characters.len().saturating_sub(6);
        let day: String = if start < end {
            characters[start..end].iter().collect()
        } else {
            String::new()
        };
        if by_day.get(&day).is_none() {
            fs::remove_file(&file)?;
        }
    }
    for (day, items) in by_day.values() {
        let sorted = pyjson::py_sorted(items.clone(), |row| {
            Ok(Value::Array(vec![
                row.index_key(key)?.clone(),
                Value::Str(pyjson::dumps(row, &DumpOptions::PYTHON.sorted())?),
            ]))
        })?;
        let mut text = String::new();
        for row in sorted {
            text.push_str(&compact_sorted(row)?);
            text.push('\n');
        }
        let temporary = directory.join(format!(".{prefix}_{day}.tmp"));
        fs::write(&temporary, text)?;
        fs::rename(&temporary, directory.join(format!("{prefix}_{day}.jsonl")))?;
    }
    Ok(rows.len())
}

fn read_csv(path: &Path) -> Vec<Value> {
    let Ok(data) = fs::read(path) else {
        return Vec::new();
    };
    let Ok(text) = String::from_utf8(data) else {
        return Vec::new();
    };
    pycsv::read_dicts(&text).unwrap_or_default()
}

fn write_csv(path: &Path, rows: &[Value], fields: &[String]) -> ToolResult<usize> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut text = String::new();
    pycsv::write_row(fields, &mut text);
    for row in rows {
        let cells: Vec<String> = fields
            .iter()
            .map(|field| match row.get(field) {
                Ok(Some(value)) if !value.is_null() => Ok(pyjson::py_str(value)),
                Ok(_) => Ok(String::new()),
                Err(error) => Err(error),
            })
            .collect::<ToolResult<_>>()?;
        pycsv::write_row(&cells, &mut text);
    }
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    let temporary = PathBuf::from(temporary);
    fs::write(&temporary, text)?;
    fs::rename(&temporary, path)?;
    Ok(rows.len())
}

fn owned_fields(fields: &[&str]) -> Vec<String> {
    fields.iter().map(|field| (*field).to_owned()).collect()
}

fn first_last(rows: &[Value], key: &str) -> ToolResult<(Value, Value)> {
    let first = match rows.first() {
        Some(row) => row.index_key(key)?.clone(),
        None => Value::Null,
    };
    let last = match rows.last() {
        Some(row) => row.index_key(key)?.clone(),
        None => Value::Null,
    };
    Ok((first, last))
}

// ---------------------------------------------------------------- datasets

fn timeseries_fields() -> Vec<String> {
    let mut fields = owned_fields(&[
        "ts_utc",
        "ts",
        "cost_index",
        "cost_basis",
        "quota_pct",
        "credits_h",
        "luna_fail_total",
        "luna_fp_rate",
    ]);
    fields.extend(TIERS.iter().map(|tier| format!("tier_{tier}")));
    for project in PROJECTS {
        for suffix in ["done", "total", "in_progress", "ci", "pr", "hygiene"] {
            fields.push(format!("{project}_{suffix}"));
        }
        fields.extend(
            STAGES
                .iter()
                .map(|stage| format!("{project}_stage_{stage}")),
        );
    }
    for host in HOSTS {
        fields.push(format!("{host}_load_pct"));
        fields.push(format!("{host}_disk_root_pct"));
    }
    fields.extend(owned_fields(&[
        "ty_tokens",
        "ty_output",
        "ty_credits",
        "ty_integration_tokens",
        "ty_main_tokens",
        "ty_yield_integration_pct",
        "ty_yield_main_pct",
        "ty_coverage_pct",
    ]));
    for project in PROJECTS.iter().chain(["other"].iter()) {
        fields.push(format!("ty_{project}_tokens"));
        fields.push(format!("ty_{project}_integration_tokens"));
        fields.push(format!("ty_{project}_main_tokens"));
        fields.push(format!("ty_{project}_credits_per_done_todo"));
    }
    fields
}

fn flatten_timeseries(row: &Value) -> ToolResult<Value> {
    let mut flat = Map::new();
    for key in [
        "ts_utc",
        "ts",
        "cost_index",
        "cost_basis",
        "quota_pct",
        "credits_h",
        "luna_fail_total",
        "luna_fp_rate",
    ] {
        flat.insert(key, row.get_value(key)?.clone());
    }
    let tiers = row.get_value("tiers")?;
    for tier in TIERS {
        let value = if tiers.truthy() {
            tiers.get(tier)?.cloned().unwrap_or(Value::Int(0))
        } else {
            Value::Int(0)
        };
        flat.insert(format!("tier_{tier}"), value);
    }
    let projects = row.get_value("proj")?;
    let hygiene = row.get_value("hyg")?;
    for project in PROJECTS {
        let entry = projects.or_empty_get(project)?;
        for (suffix, key) in [
            ("done", "done"),
            ("total", "total"),
            ("in_progress", "ip"),
            ("ci", "ci"),
            ("pr", "pr"),
        ] {
            flat.insert(
                format!("{project}_{suffix}"),
                entry.or_empty_get(key)?.clone(),
            );
        }
        flat.insert(
            format!("{project}_hygiene"),
            hygiene.or_empty_get(project)?.clone(),
        );
        let stages = entry.or_empty_get("st")?;
        for stage in STAGES {
            flat.insert(
                format!("{project}_stage_{stage}"),
                stages.or_empty_get(stage)?.clone(),
            );
        }
    }
    let hosts = row.get_value("hosts")?;
    for host in HOSTS {
        let entry = hosts.or_empty_get(host)?;
        flat.insert(
            format!("{host}_load_pct"),
            entry.or_empty_get("load")?.clone(),
        );
        flat.insert(
            format!("{host}_disk_root_pct"),
            entry.or_empty_get("disk")?.clone(),
        );
    }
    let yields = row.get_value("ty")?;
    for (column, key) in [
        ("ty_tokens", "tok"),
        ("ty_output", "out"),
        ("ty_credits", "cr"),
        ("ty_integration_tokens", "int"),
        ("ty_main_tokens", "main"),
        ("ty_yield_integration_pct", "yi"),
        ("ty_yield_main_pct", "ym"),
        ("ty_coverage_pct", "cov"),
    ] {
        flat.insert(column, yields.or_empty_get(key)?.clone());
    }
    let per_project = yields.or_empty_get("p")?;
    if per_project.truthy() {
        let map = per_project.as_object().ok_or_else(|| {
            ToolError::type_error(format!(
                "'{}' object has no attribute 'items'",
                per_project.type_name()
            ))
        })?;
        for (project, value) in map.iter() {
            let known = PROJECTS.contains(&project.as_str()) || project == "other";
            if let (true, Value::Array(items)) = (known, value) {
                let mut padded = items.clone();
                padded.resize(items.len() + 4, Value::Null);
                padded.truncate(4);
                let columns = [
                    format!("ty_{project}_tokens"),
                    format!("ty_{project}_integration_tokens"),
                    format!("ty_{project}_main_tokens"),
                    format!("ty_{project}_credits_per_done_todo"),
                ];
                for (column, item) in columns.into_iter().zip(padded) {
                    flat.insert(column, item);
                }
            }
        }
    }
    Ok(Value::Object(flat))
}

fn ex_timeseries(dash: &Path, out: &Path, home: &str) -> ToolResult<Value> {
    let directory = out.join("timeseries");
    let mut have: OrderedDict<Value> = OrderedDict::new();
    for row in read_jsonl_dir(&directory, "history")? {
        have.insert(py_hash_key(row.index_key("ts_utc")?)?, row.clone());
    }
    match read_text(&dash.join("history.jsonl")) {
        Ok(text) => {
            for line in text.split_terminator('\n') {
                let Ok(row) = pyjson::loads(line, lenient()) else {
                    continue;
                };
                if !row.as_object().is_some_and(|map| map.contains_key("ts")) {
                    continue;
                }
                let mut row = clean(&row, home);
                let stamp = iso(row.index_key("ts")?)?;
                if let Some(map) = row.as_object_mut() {
                    map.insert("ts_utc", stamp.as_str());
                }
                have.insert(py_hash_key(&Value::Str(stamp))?, row);
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let rows = pyjson::py_sorted(have.into_values(), |row| {
        Ok(row.index_key("ts_utc")?.clone())
    })?;
    write_jsonl_by_day(&directory, "history", &rows, "ts_utc")?;
    let flat = rows
        .iter()
        .map(flatten_timeseries)
        .collect::<ToolResult<Vec<_>>>()?;
    write_csv(&directory.join("history.csv"), &flat, &timeseries_fields())?;
    let (first, last) = first_last(&rows, "ts_utc")?;
    Ok(Value::Array(vec![rows.len().into(), first, last]))
}

fn norm_ledger(raw: &Map, home: &str) -> Value {
    let get = |key: &str| raw.get(key).cloned().unwrap_or(Value::Null);
    let legacy =
        (raw.contains_key("utc") && !raw.contains_key("ts")) || raw.contains_key("to_tier");
    let ts = py_or(&get("ts"), &get("utc"));
    let escalated = if get("escalated_to").is_null() {
        get("to_tier")
    } else {
        get("escalated_to")
    };
    let outcome = get("outcome");
    let outcome = if outcome.truthy() {
        pyjson::py_str(&outcome).to_lowercase()
    } else {
        String::new()
    };
    let mut row = Map::new();
    row.insert("ts", ts);
    for key in [
        "project", "task", "screen", "host", "tier", "model", "effort",
    ] {
        row.insert(key, get(key));
    }
    row.insert(
        "outcome",
        if outcome.is_empty() {
            Value::Null
        } else {
            Value::Str(outcome)
        },
    );
    row.insert("escalated_to", escalated);
    row.insert("reason", py_or(&get("reason"), &get("note")));
    row.insert("tokens", get("tokens"));
    row.insert(
        "source_format",
        if legacy { "legacy_s9" } else { "canonical" },
    );
    for key in ["class", "attempt", "minutes"] {
        if !get(key).is_null() {
            row.insert(key, get(key));
        }
    }
    clean(&Value::Object(row), home)
}

fn ex_ledger(capd: &Path, out: &Path, home: &str) -> ToolResult<Value> {
    let directory = out.join("ledger");
    let mut have: OrderedDict<Value> = OrderedDict::new();
    let key = |row: &Value| pyjson::dumps(row, &DumpOptions::PYTHON.sorted());
    for row in read_jsonl_dir(&directory, "model_usage")? {
        have.insert(key(&row)?, row);
    }
    let mut bad = 0usize;
    match read_text(&capd.join("model_usage.jsonl")) {
        Ok(text) => {
            for line in text.split_terminator('\n') {
                let line = pyjson::py_strip(line);
                if line.is_empty() {
                    continue;
                }
                let Ok(raw) = pyjson::loads(line, lenient()) else {
                    bad += 1;
                    continue;
                };
                let Some(raw) = raw.as_object() else {
                    bad += 1;
                    continue;
                };
                let mut row = norm_ledger(raw, home);
                let ts = row.get_value("ts")?.clone();
                let parsed = if ts.truthy() { parse_iso(&ts) } else { None };
                let Some(parsed) = parsed else {
                    bad += 1;
                    continue;
                };
                let stamp = iso(&Value::Float(parsed))?;
                if let Some(map) = row.as_object_mut() {
                    map.insert("ts", stamp);
                }
                have.insert(key(&row)?, row);
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let rows = pyjson::py_sorted(have.into_values(), |row| Ok(row.index_key("ts")?.clone()))?;
    write_jsonl_by_day(&directory, "model_usage", &rows, "ts")?;
    let mut fields = owned_fields(&LEDGER_FIELDS);
    fields.extend(owned_fields(&[
        "source_format",
        "class",
        "attempt",
        "minutes",
    ]));
    write_csv(&directory.join("model_usage.csv"), &rows, &fields)?;
    let (first, last) = first_last(&rows, "ts")?;
    Ok(Value::Array(vec![
        rows.len().into(),
        bad.into(),
        first,
        last,
    ]))
}

fn quota_row(sample: &Value) -> ToolResult<(String, Value)> {
    let first = pyjson::py_index(sample, 0)?;
    let stamp = iso(&first)?;
    let rounded = pyjson::round_float(pyjson::py_float(&first)?, 1);
    let used = pyjson::py_index(sample, 1)?;
    let credits = if pyjson::py_len(sample)? > 2 {
        pyjson::py_index(sample, 2)?
    } else {
        Value::Null
    };
    let row = crate::pyobj! {
        "ts_utc" => stamp.as_str(),
        "ts" => Value::Float(rounded),
        "used_pct" => used,
        "credits_balance" => credits,
    };
    Ok((stamp, row))
}

fn ex_quota(dash: &Path, out: &Path) -> ToolResult<Value> {
    let path = out.join("quota").join("codex_quota.csv");
    let mut have: OrderedDict<Value> = OrderedDict::new();
    for row in read_csv(&path) {
        have.insert(py_hash_key(row.index_key("ts_utc")?)?, row.clone());
    }
    let samples = rj(&dash.join(".quota_history.json")).unwrap_or(Value::Array(Vec::new()));
    let samples = if samples.truthy() {
        pyjson::py_iter(&samples)?
    } else {
        Vec::new()
    };
    for sample in samples {
        if let Ok((stamp, row)) = quota_row(&sample) {
            have.insert(py_hash_key(&Value::Str(stamp))?, row);
        }
    }
    let rows = pyjson::py_sorted(have.into_values(), |row| {
        Ok(row.index_key("ts_utc")?.clone())
    })?;
    let written = write_csv(
        &path,
        &rows,
        &owned_fields(&["ts_utc", "ts", "used_pct", "credits_balance"]),
    )?;
    Ok(written.into())
}

fn first_root_disk(host: &Value) -> ToolResult<Value> {
    let empty = Value::Array(Vec::new());
    let disks = match host.get("disks")? {
        Some(disks) => disks,
        None => &empty,
    };
    for disk in pyjson::py_iter(disks)? {
        if disk.get_value("mount")? == &Value::Str("/".to_owned()) {
            return Ok(if disk.truthy() {
                disk
            } else {
                Value::Object(Map::new())
            });
        }
    }
    Ok(Value::Object(Map::new()))
}

fn gpu_average(host: &Value) -> ToolResult<Value> {
    let gpus = host.get_value("gpus")?;
    if !gpus.truthy() {
        return Ok(Value::Null);
    }
    let gpus_list = host.index_key("gpus")?;
    let mut total = Value::Int(0);
    for gpu in pyjson::py_iter(gpus_list)? {
        let util = match gpu.get("util")? {
            Some(util) => util.clone(),
            None => Value::Int(0),
        };
        let util = if util.truthy() { util } else { Value::Int(0) };
        total = pyjson::py_add(&total, &util)?;
    }
    let count = pyjson::py_len(gpus_list)?;
    let average = pyjson::py_div(&total, &Value::from(count))?;
    Ok(Value::Float(pyjson::round_float(average, 1)))
}

fn host_snapshot(stamp: &Value, host: &Value) -> ToolResult<Value> {
    let memory = host.get_value("mem")?;
    let root = first_root_disk(host)?;
    let load = host.get_value("load")?;
    let load1 = if load.truthy() {
        pyjson::py_index(load, 0)?
    } else {
        Value::Null
    };
    let gpus = host.get_value("gpus")?;
    let gpu_count = if gpus.truthy() {
        pyjson::py_len(gpus)?
    } else {
        0
    };
    Ok(crate::pyobj! {
        "ts_utc" => stamp.clone(),
        "host" => host.get_value("host")?.clone(),
        "reachable" => host.get_value("reachable")?.clone(),
        "nproc" => host.get_value("nproc")?.clone(),
        "load1" => load1,
        "mem_total_gb" => memory.or_empty_get("total_gb")?.clone(),
        "mem_avail_gb" => memory.or_empty_get("avail_gb")?.clone(),
        "swap_used_gb" => memory.or_empty_get("swap_used_gb")?.clone(),
        "disk_root_pct" => root.get_value("pct")?.clone(),
        "disk_root_avail_bytes" => root.get_value("avail")?.clone(),
        "gpus" => gpu_count,
        "gpu_util_avg" => gpu_average(host)?,
    })
}

fn ex_resources(dash: &Path, out: &Path, home: &str) -> ToolResult<Value> {
    let path = out.join("resources").join("disk_used.csv");
    let mut have: OrderedDict<Value> = OrderedDict::new();
    for row in read_csv(&path) {
        let key = py_tuple_key(&[
            row.index_key("ts_utc")?,
            row.index_key("host")?,
            row.index_key("mount")?,
        ])?;
        have.insert(key, row.clone());
    }
    let history = rj(&dash.join(".res_history.json")).unwrap_or(Value::Object(Map::new()));
    if history.truthy() {
        let map = history.as_object().ok_or_else(|| {
            ToolError::type_error(format!(
                "'{}' object has no attribute 'items'",
                history.type_name()
            ))
        })?;
        for (key, series) in map.iter() {
            let (host, mount) = key.split_once(':').unwrap_or((key.as_str(), ""));
            if host == "xbabe0" {
                continue;
            }
            for sample in pyjson::py_iter(series)? {
                let pair = pyjson::py_iter(&sample)?;
                let [stamp, used] = pair.as_slice() else {
                    return Err(ToolError::type_error(format!(
                        "expected 2 values to unpack (got {})",
                        pair.len()
                    )));
                };
                let mount = clean(&Value::Str(mount.to_owned()), home);
                let row = crate::pyobj! {
                    "ts_utc" => iso(stamp)?,
                    "host" => host,
                    "mount" => mount.clone(),
                    "used_bytes" => pyjson::py_int(used)?,
                };
                let key = py_tuple_key(&[
                    row.index_key("ts_utc")?,
                    &Value::Str(host.to_owned()),
                    &mount,
                ])?;
                have.insert(key, row);
            }
        }
    }
    let rows = pyjson::py_sorted(have.into_values(), |row| {
        Ok(Value::Array(vec![
            row.index_key("ts_utc")?.clone(),
            row.index_key("host")?.clone(),
            row.index_key("mount")?.clone(),
        ]))
    })?;
    let disk_rows = write_csv(
        &path,
        &rows,
        &owned_fields(&["ts_utc", "host", "mount", "used_bytes"]),
    )?;
    // Host snapshot (one row per host per export run).
    let global = rj(&dash.join("global_resources.json")).unwrap_or(Value::Object(Map::new()));
    let global = if global.truthy() {
        global
    } else {
        Value::Object(Map::new())
    };
    let snapshot_path = out.join("resources").join("host_snapshots.csv");
    let mut snapshots: OrderedDict<Value> = OrderedDict::new();
    for row in read_csv(&snapshot_path) {
        let key = py_tuple_key(&[row.index_key("ts_utc")?, row.index_key("host")?])?;
        snapshots.insert(key, row.clone());
    }
    let stamp = global.get_value("generated_utc")?.clone();
    let hosts = global.get_value("hosts")?;
    let hosts = if hosts.truthy() {
        pyjson::py_iter(hosts)?
    } else {
        Vec::new()
    };
    for host in hosts {
        if !stamp.truthy() || host.get_value("host")? == &Value::Str("xbabe0".to_owned()) {
            continue;
        }
        let row = host_snapshot(&stamp, &host)?;
        let key = py_tuple_key(&[&stamp, row.index_key("host")?])?;
        snapshots.insert(key, row);
    }
    let snapshot_rows = pyjson::py_sorted(snapshots.into_values(), |row| {
        Ok(Value::Array(vec![
            row.index_key("ts_utc")?.clone(),
            row.index_key("host")?.clone(),
        ]))
    })?;
    write_csv(
        &snapshot_path,
        &snapshot_rows,
        &owned_fields(&[
            "ts_utc",
            "host",
            "reachable",
            "nproc",
            "load1",
            "mem_total_gb",
            "mem_avail_gb",
            "swap_used_gb",
            "disk_root_pct",
            "disk_root_avail_bytes",
            "gpus",
            "gpu_util_avg",
        ]),
    )?;
    Ok(Value::Array(vec![
        disk_rows.into(),
        snapshot_rows.len().into(),
    ]))
}

fn sqlite_error(error: rusqlite::Error) -> ToolError {
    ToolError::type_error(format!("sqlite3: {error}"))
}

fn sqlite_value(value: rusqlite::types::ValueRef<'_>) -> ToolResult<Value> {
    use rusqlite::types::ValueRef;
    Ok(match value {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(number) => Value::Int(i128::from(number)),
        ValueRef::Real(number) => Value::Float(number),
        ValueRef::Text(bytes) => Value::Str(
            String::from_utf8(bytes.to_vec())
                .map_err(|_| ToolError::type_error("Could not decode to UTF-8"))?,
        ),
        ValueRef::Blob(_) => return Err(ToolError::type_error("unsupported BLOB value")),
    })
}

fn query_rows(
    connection: &rusqlite::Connection,
    sql: &str,
    columns: usize,
) -> ToolResult<Vec<Vec<Value>>> {
    let mut statement = connection.prepare(sql).map_err(sqlite_error)?;
    let mut rows = statement.query([]).map_err(sqlite_error)?;
    let mut result = Vec::new();
    while let Some(row) = rows.next().map_err(sqlite_error)? {
        let mut values = Vec::with_capacity(columns);
        for index in 0..columns {
            values.push(sqlite_value(row.get_ref(index).map_err(sqlite_error)?)?);
        }
        result.push(values);
    }
    Ok(result)
}

fn ex_tokens(dash: &Path, out: &Path) -> ToolResult<Value> {
    let database = dash.join("tokens.db");
    if !database.exists() {
        return Ok(Value::Array(vec![0.into(), 0.into()]));
    }
    let connection = rusqlite::Connection::open_with_flags(
        format!("file:{}?mode=ro", database.display()),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
            | rusqlite::OpenFlags::SQLITE_OPEN_URI
            | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(sqlite_error)?;
    connection
        .busy_timeout(Duration::from_secs(10))
        .map_err(sqlite_error)?;
    let path = out.join("tokens").join("codex_sessions.csv");
    let mut have: OrderedDict<Value> = OrderedDict::new();
    for row in read_csv(&path) {
        have.insert(py_hash_key(row.index_key("session")?)?, row.clone());
    }
    for values in query_rows(
        &connection,
        "SELECT id,tool,model,effort,cwd,host,start,end,input,cached,output,reasoning,credits FROM sessions",
        13,
    )? {
        let [
            sid,
            tool,
            model,
            effort,
            cwd,
            host,
            start,
            end,
            input,
            cached,
            output,
            reasoning,
            credits,
        ] = values.as_slice()
        else {
            continue;
        };
        let stamp = |value: &Value| -> ToolResult<Value> {
            Ok(if value.truthy() {
                Value::Str(iso(value)?)
            } else {
                Value::Null
            })
        };
        let credits_est = if credits.is_null() {
            Value::Null
        } else {
            pyjson::py_round(credits, 3)?
        };
        let row = crate::pyobj! {
            "session" => sid.clone(),
            "tool" => tool.clone(),
            "model" => model.clone(),
            "effort" => effort.clone(),
            "project" => project_of(cwd)?,
            "host" => host.clone(),
            "start_utc" => stamp(start)?,
            "end_utc" => stamp(end)?,
            "input" => input.clone(),
            "cached" => cached.clone(),
            "output" => output.clone(),
            "reasoning" => reasoning.clone(),
            "credits_est" => credits_est,
        };
        have.insert(py_hash_key(sid)?, row);
    }
    let rows = pyjson::py_sorted(have.into_values(), |row| {
        Ok(Value::Array(vec![
            py_or(row.get_value("start_utc")?, &Value::Str(String::new())),
            row.index_key("session")?.clone(),
        ]))
    })?;
    let session_rows = write_csv(
        &path,
        &rows,
        &owned_fields(&[
            "session",
            "tool",
            "model",
            "effort",
            "project",
            "host",
            "start_utc",
            "end_utc",
            "input",
            "cached",
            "output",
            "reasoning",
            "credits_est",
        ]),
    )?;
    let hourly_path = out.join("tokens").join("claude_hourly.csv");
    let mut aggregate: OrderedDict<Value> = OrderedDict::new();
    for row in read_csv(&hourly_path) {
        let key = py_tuple_key(&[
            row.index_key("hour_utc")?,
            row.index_key("project")?,
            row.index_key("model")?,
        ])?;
        aggregate.insert(key, row.clone());
    }
    let mut fresh: OrderedDict<(Vec<Value>, [Value; 4])> = OrderedDict::new();
    for values in query_rows(
        &connection,
        "SELECT ts,model,cwd,input,cached,output FROM claude_msgs",
        6,
    )? {
        let [ts, model, cwd, input, cached, output] = values.as_slice() else {
            continue;
        };
        if ts.is_null() {
            continue;
        }
        let hour: String = iso(ts)?.chars().take(13).collect();
        let parts = vec![
            Value::Str(format!("{hour}:00Z")),
            Value::Str(project_of(cwd)?),
            py_or(model, &Value::Str("unknown".to_owned())),
        ];
        let key = py_tuple_key(&parts.iter().collect::<Vec<_>>())?;
        if fresh.get(&key).is_none() {
            fresh.insert(
                key.clone(),
                (
                    parts,
                    [Value::Int(0), Value::Int(0), Value::Int(0), Value::Int(0)],
                ),
            );
        }
        if let Some((_, totals)) = fresh.get_mut(&key) {
            totals[0] = pyjson::py_add(&totals[0], &Value::Int(1))?;
            for (slot, value) in [input, cached, output].into_iter().enumerate() {
                let value = py_or(value, &Value::Int(0));
                totals[slot + 1] = pyjson::py_add(&totals[slot + 1], &value)?;
            }
        }
    }
    // The database is cumulative: fresh hourly totals replace older exports.
    for (parts, totals) in fresh.into_values() {
        let key = py_tuple_key(&parts.iter().collect::<Vec<_>>())?;
        let [hour, project, model] =
            <[Value; 3]>::try_from(parts).map_err(|_| ToolError::type_error("hourly key shape"))?;
        let [messages, input, cached, output] = totals;
        aggregate.insert(
            key,
            crate::pyobj! {
                "hour_utc" => hour,
                "project" => project,
                "model" => model,
                "messages" => messages,
                "input" => input,
                "cached" => cached,
                "output" => output,
            },
        );
    }
    let hourly = pyjson::py_sorted(aggregate.into_values(), |row| {
        Ok(Value::Array(vec![
            row.index_key("hour_utc")?.clone(),
            row.index_key("project")?.clone(),
            row.index_key("model")?.clone(),
        ]))
    })?;
    write_csv(
        &hourly_path,
        &hourly,
        &owned_fields(&[
            "hour_utc", "project", "model", "messages", "input", "cached", "output",
        ]),
    )?;
    Ok(Value::Array(vec![session_rows.into(), hourly.len().into()]))
}

fn any_matches(rows: &[Value], key: &str, expected: &Value) -> ToolResult<bool> {
    for row in rows {
        if row.get_value(key)? == expected {
            return Ok(true);
        }
    }
    Ok(false)
}

fn ex_token_yield(dash: &Path, out: &Path, home: &str) -> ToolResult<Value> {
    let source = rj(&dash.join("token_yield.json")).unwrap_or(Value::Null);
    let directory = out.join("token_yield");
    let mut rows = read_jsonl_dir(&directory, "token_yield")?;
    if source.truthy() && source.get_value("generated_utc")?.truthy() {
        let mut snapshot = Map::new();
        snapshot.insert("generated_utc", source.index_key("generated_utc")?.clone());
        for key in [
            "window_hours",
            "total",
            "by_tier",
            "projects",
            "method",
            "hosts_covered",
            "not_covered",
        ] {
            snapshot.insert(key, source.get_value(key)?.clone());
        }
        let snapshot = clean(&Value::Object(snapshot), home);
        let generated = snapshot.index_key("generated_utc")?.clone();
        if !any_matches(&rows, "generated_utc", &generated)? {
            rows.push(snapshot);
        }
    }
    write_jsonl_by_day(&directory, "token_yield", &rows, "generated_utc")?;
    Ok(rows.len().into())
}

fn project_snapshot(status: &Value, captains: &Value, project: &str) -> ToolResult<Value> {
    let entry = status.get_value("projects")?.or_empty_get(project)?;
    let pr = entry.or_empty_get("pr")?;
    let ci = pr.or_empty_get("ci")?;
    let hygiene = captains
        .get_value("captains")?
        .or_empty_get(project)?
        .or_empty_get("hygiene")?;
    let get = |key: &str| -> ToolResult<Value> { Ok(entry.or_empty_get(key)?.clone()) };
    let stage_meta = entry.or_empty_get("stage_meta")?;
    let stage_meta = if stage_meta.truthy() {
        let mut map = Map::new();
        for key in [
            "pr_number",
            "pr_open",
            "main_sha",
            "main_ci_green",
            "accepted_in_pr",
            "blocked_code_stage",
        ] {
            map.insert(key, stage_meta.or_empty_get(key)?.clone());
        }
        Value::Object(map)
    } else {
        Value::Null
    };
    let length_or_zero = |value: Value| -> ToolResult<usize> {
        if value.truthy() {
            pyjson::py_len(&value)
        } else {
            Ok(0)
        }
    };
    let head = py_or(
        &py_or(pr.or_empty_get("head")?, pr.or_empty_get("head_sha")?),
        &Value::Str(String::new()),
    );
    let head = pyjson::py_prefix(&head, 12)?;
    let head = if head.truthy() { head } else { Value::Null };
    let needs = get("needs_jepson")?;
    let needs = match &needs {
        Value::Array(items) => Value::from(items.len()),
        _ => Value::Null,
    };
    let hygiene = if hygiene.truthy() {
        let mut map = Map::new();
        for key in [
            "score",
            "grade",
            "stale_merged_worktrees",
            "uncommitted_over_45m",
            "lanes_behind",
            "temp_growth_gb",
        ] {
            map.insert(key, hygiene.get_value(key)?.clone());
        }
        Value::Object(map)
    } else {
        Value::Null
    };
    Ok(crate::pyobj! {
        "done" => get("done")?,
        "total" => get("total")?,
        "counts" => get("counts")?,
        "stages" => get("stages")?,
        "stage_meta" => stage_meta,
        "packets_accepted" => get("packets_accepted")?,
        "packets_total" => get("packets_total")?,
        "blocked" => length_or_zero(get("blocked_ids")?)?,
        "workers" => length_or_zero(get("workers")?)?,
        "pr_number" => pr.or_empty_get("number")?.clone(),
        "pr_head" => head,
        "ci_state" => ci.or_empty_get("state")?.clone(),
        "needs_jepson" => needs,
        "hygiene" => hygiene,
    })
}

fn ex_snapshots(dash: &Path, capd: &Path, out: &Path, home: &str) -> ToolResult<Value> {
    let or_empty = |value: Option<Value>| match value {
        Some(value) if value.truthy() => value,
        _ => Value::Object(Map::new()),
    };
    let status = or_empty(rj(&dash.join("status.json")));
    let captains = or_empty(rj(&capd.join("super_captain_status.json")));
    let directory = out.join("snapshots");
    let mut rows = read_jsonl_dir(&directory, "projects")?;
    let stamp = status.get_value("generated_utc")?.clone();
    if stamp.truthy() {
        let parsed = parse_iso(&stamp).ok_or_else(|| {
            ToolError::type_error(
                "float() argument must be a string or a real number, not 'NoneType'",
            )
        })?;
        let stamp = Value::Str(iso(&Value::Float(parsed))?);
        let mut projects = Map::new();
        for project in PROJECTS {
            projects.insert(project, project_snapshot(&status, &captains, project)?);
        }
        let snapshot = clean(
            &crate::pyobj! { "ts_utc" => stamp.clone(), "projects" => Value::Object(projects) },
            home,
        );
        if !any_matches(&rows, "ts_utc", &stamp)? {
            rows.push(snapshot);
        }
    }
    write_jsonl_by_day(&directory, "projects", &rows, "ts_utc")?;
    Ok(rows.len().into())
}

fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn ex_rates(repo: &Path, out: &Path) -> ToolResult<Value> {
    let directory = repo.join("captains").join("rates");
    let mut index = Vec::new();
    for file in fsx::glob_sorted(&directory, "*_rates_*.json") {
        let data = fs::read(&file)?;
        let document = pyjson::loads_bytes(&data, lenient())
            .map_err(|error| ToolError::type_error(format!("{}: {error}", file.display())))?;
        let name = file
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let models = document.get_value("models")?;
        let models = if models.truthy() {
            pyjson::py_len(models)?
        } else {
            0
        };
        index.push(crate::pyobj! {
            "file" => format!("captains/rates/{name}"),
            "vendor" => document.get_value("vendor")?.clone(),
            "as_of" => document.get_value("as_of")?.clone(),
            "unit" => document.get_value("unit")?.clone(),
            "currency" => document.get_value("currency")?.clone(),
            "baseline_model" => document.get_value("baseline_model")?.clone(),
            "models" => models,
            "sha256" => sha256_hex(&data),
        });
    }
    let count = index.len();
    let document = crate::pyobj! {
        "note" => "Rate files live in captains/rates/ (single source of truth). Vendors use different currencies; never compare across files.",
        "files" => Value::Array(index),
    };
    fs::create_dir_all(out)?;
    let path = out.join("rates_index.json");
    let temporary = out.join("rates_index.json.tmp");
    fs::write(
        &temporary,
        pyjson::dumps(&document, &DumpOptions::PYTHON.indent(1))?,
    )?;
    fs::rename(&temporary, &path)?;
    Ok(count.into())
}

/// One secret-scan hit: (relative path, pattern prefix, character offset).
pub type Hit = (String, String, usize);

fn walk_files(directory: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
    entries.sort_by_key(fs::DirEntry::file_name);
    let mut subdirectories = Vec::new();
    for entry in entries {
        let path = entry.path();
        let is_dir = fs::metadata(&path).is_ok_and(|meta| meta.is_dir());
        if is_dir {
            if !fsx::is_symlink(&path) {
                subdirectories.push(path);
            }
        } else {
            files.push(path);
        }
    }
    for subdirectory in subdirectories {
        walk_files(&subdirectory, files);
    }
}

/// Secret-pattern scan over every file under `out`.
pub fn secret_scan(out: &Path, home: &str) -> Vec<Hit> {
    let mut files = Vec::new();
    walk_files(out, &mut files);
    let mut hits = Vec::new();
    for path in files {
        if !fs::metadata(&path).is_ok_and(|meta| meta.is_file()) {
            continue;
        }
        let Ok(data) = fs::read(&path) else {
            continue;
        };
        let text = pyjson::universal_newlines(&String::from_utf8_lossy(&data));
        let relative = path.strip_prefix(out).map_or_else(
            |_| path.display().to_string(),
            |rel| rel.display().to_string(),
        );
        let char_offset = |byte: usize| text[..byte].chars().count();
        for (pattern, expression) in SECRET_RES.iter() {
            if let Some(found) = expression.find(&text) {
                hits.push((
                    relative.clone(),
                    pattern.clone(),
                    char_offset(found.start()),
                ));
            }
        }
        if let Some(position) = text.find(home) {
            hits.push((
                relative.clone(),
                "absolute home path".to_owned(),
                char_offset(position),
            ));
        }
    }
    hits
}

/// Run the export; writes the summary and scan result to `stdout`/`stderr`
/// exactly like the Python script. Returns 0, or 3 on a secret-scan hit.
pub fn export(
    options: &ExportOptions,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> ToolResult<i32> {
    let home = options.home.as_str();
    if !options.check {
        let out = options.out.as_path();
        let summary = crate::pyobj! {
            "timeseries" => ex_timeseries(&options.dash, out, home)?,
            "ledger" => ex_ledger(&options.capd, out, home)?,
            "quota_rows" => ex_quota(&options.dash, out)?,
            "resources" => ex_resources(&options.dash, out, home)?,
            "tokens" => ex_tokens(&options.dash, out)?,
            "token_yield_snapshots" => ex_token_yield(&options.dash, out, home)?,
            "project_snapshots" => ex_snapshots(&options.dash, &options.capd, out, home)?,
            "rate_files" => ex_rates(&options.repo, out)?,
        };
        writeln!(stdout, "{}", pyjson::dumps_default(&summary))?;
    }
    let hits = secret_scan(&options.out, home);
    if !hits.is_empty() {
        for (path, pattern, offset) in hits.iter().take(50) {
            writeln!(
                stderr,
                "SECRET-SCAN HIT ({}, {}, {offset})",
                pyjson::py_repr_str(path),
                pyjson::py_repr_str(pattern)
            )?;
        }
        return Ok(3);
    }
    writeln!(stdout, "secret scan: clean")?;
    Ok(0)
}

/// `admiralty history export` arguments (same flags as the Python argparse).
#[derive(clap::Args, Debug, Clone)]
pub struct ExportArgs {
    /// Output directory (default: `<repo>/data/history`).
    #[arg(long)]
    pub out: Option<PathBuf>,
    /// Dashboard runtime (default: `$CDASH_RUNTIME` or `~/xbabe2_kicker/captains-dashboard`).
    #[arg(long)]
    pub dash: Option<PathBuf>,
    /// Captains runtime (default: `~/xbabe2_kicker/captains-20261017`).
    #[arg(long)]
    pub capd: Option<PathBuf>,
    /// Only run the secret scan on `--out`.
    #[arg(long)]
    pub check: bool,
    /// Repository root holding `captains/rates/` (default: current directory).
    #[arg(long)]
    pub repo: Option<PathBuf>,
}

/// Resolve CLI arguments into exporter options.
pub fn options_from_args(args: &ExportArgs) -> ToolResult<ExportOptions> {
    let repo = match &args.repo {
        Some(repo) => repo.clone(),
        None => std::env::current_dir()?,
    };
    let mut options = ExportOptions::defaults(repo, fsx::home_dir());
    if let Some(out) = &args.out {
        options.out.clone_from(out);
    }
    if let Some(dash) = &args.dash {
        options.dash.clone_from(dash);
    }
    if let Some(capd) = &args.capd {
        options.capd.clone_from(capd);
    }
    options.check = args.check;
    Ok(options)
}

/// Run `history export`; returns the exit code (1 on unexpected errors).
pub fn run(args: &ExportArgs) -> Result<i32, crate::BoxError> {
    let options = options_from_args(args)?;
    let mut stdout = io::stdout().lock();
    let mut stderr = io::stderr();
    match export(&options, &mut stdout, &mut stderr) {
        Ok(code) => Ok(code),
        Err(error) => {
            eprintln!("history_export: {error}");
            Ok(1)
        }
    }
}

#[cfg(test)]
#[path = "history_export_tests.rs"]
mod tests;
