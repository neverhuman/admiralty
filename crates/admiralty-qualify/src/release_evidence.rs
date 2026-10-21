//! Read-only P31 release evidence preflight (port of the retired
//! `tests/qualification/release_evidence.py`). Never deploys or grants
//! acceptance.
//!
//! Exit 0: the declared source/time/hash metadata is ready for independent
//! review. Exit 1: failed, missing or incomplete gates. Exit 2: an invalid
//! manifest or artifact (printed as `{"status": "invalid", ...}`).

use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use chrono::{Datelike, NaiveDate, Timelike};
use rustix::fs::{Mode, OFlags};
use sha2::{Digest, Sha256};

use crate::py::{self, Dump, PyDict, PyValue};
use crate::py_dict;

/// Largest manifest.
pub const MAX_MANIFEST: usize = 1024 * 1024;
/// Largest artifact.
pub const MAX_ARTIFACT: u64 = 64 * 1024 * 1024;
/// Every gate, in report order.
pub const REQUIRED: [&str; 12] = [
    "invariant_matrix",
    "load_query",
    "synthetic_soak",
    "clean_install",
    "provider_transport",
    "restart_continuity",
    "backup_restore",
    "rollback",
    "staged_canary",
    "fleet_handoff",
    "context_fidelity",
    "complete_cost_trial",
];

/// Gates that require an actual (not synthetic) journey.
pub fn actual_required(name: &str) -> bool {
    REQUIRED[3..].contains(&name)
}

/// A preflight failure, classified like the Python exception types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvidenceError {
    /// `ValueError` (exit 2).
    Value(String),
    /// `OSError` (exit 2).
    Os(String),
    /// Any other exception (the Python traceback exit 1).
    Uncaught(String),
}

impl std::fmt::Display for EvidenceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Value(message) | Self::Os(message) | Self::Uncaught(message) => {
                formatter.write_str(message)
            }
        }
    }
}

impl std::error::Error for EvidenceError {}

fn value_error(message: impl Into<String>) -> EvidenceError {
    EvidenceError::Value(message.into())
}

/// `str(OSError)` for an error raised on `filename`.
fn os_error(error: &io::Error, filename: &str) -> EvidenceError {
    let text = error.to_string();
    let description = text
        .rsplit_once(" (os error")
        .map_or(text.as_str(), |(description, _)| description);
    EvidenceError::Os(match error.raw_os_error() {
        Some(code) => format!("[Errno {code}] {description}: {}", py::repr_str(filename)),
        None => text.clone(),
    })
}

// ---------------------------------------------------------------------------
// datetime.fromisoformat (CPython 3.13 C implementation)
// ---------------------------------------------------------------------------

struct Cursor<'a> {
    bytes: &'a [u8],
}

impl Cursor<'_> {
    /// The byte at `index`, or the C string terminator past the end.
    fn at(&self, index: usize) -> u8 {
        self.bytes.get(index).copied().unwrap_or(0)
    }

    fn digits(&self, position: usize, count: usize) -> Option<(i64, usize)> {
        let mut value = 0i64;
        for offset in 0..count {
            let digit = self.at(position + offset).wrapping_sub(b'0');
            if digit > 9 {
                return None;
            }
            value = value * 10 + i64::from(digit);
        }
        Some((value, position + count))
    }
}

fn find_separator(c: &Cursor<'_>, len: usize) -> isize {
    if len == 7 {
        return 7;
    }
    if c.at(4) == b'-' {
        if c.at(5) == b'W' {
            if len < 8 {
                return -1;
            }
            if len > 8 && c.at(8) == b'-' {
                if len == 9 {
                    return -1;
                }
                if len > 10 && c.at(10).is_ascii_digit() {
                    return 8;
                }
                return 10;
            }
            return 8;
        }
        return 10;
    }
    if c.at(4) == b'W' {
        let mut index = 7;
        while index < len && c.at(index).is_ascii_digit() {
            index += 1;
        }
        if index < 9 {
            return index as isize;
        }
        return if index % 2 == 0 { 7 } else { 8 };
    }
    8
}

fn iso_to_ymd(year: i64, week: i64, day: i64) -> Result<(i64, i64, i64), ()> {
    if !(1..=9999).contains(&year) {
        return Err(());
    }
    let jan1 = NaiveDate::from_ymd_opt(year as i32, 1, 1).ok_or(())?;
    if week <= 0 || week >= 53 {
        let first_weekday = jan1.weekday().num_days_from_monday();
        let leap = NaiveDate::from_ymd_opt(year as i32, 2, 29).is_some();
        if !(week == 53 && (first_weekday == 3 || (first_weekday == 2 && leap))) {
            return Err(());
        }
    }
    if day <= 0 || day >= 8 {
        return Err(());
    }
    // Monday of ISO week 1.
    let first_weekday = i64::from(jan1.weekday().num_days_from_monday());
    let mut week1_monday = i64::from(jan1.num_days_from_ce()) - first_weekday;
    if first_weekday > 3 {
        week1_monday += 7;
    }
    let ordinal = week1_monday + (week - 1) * 7 + day - 1;
    let date = NaiveDate::from_num_days_from_ce_opt(ordinal as i32).ok_or(())?;
    Ok((
        i64::from(date.year()),
        i64::from(date.month()),
        i64::from(date.day()),
    ))
}

fn parse_date(c: &Cursor<'_>, len: usize) -> Result<(i64, i64, i64), ()> {
    let (year, mut p) = c.digits(0, 4).ok_or(())?;
    let separator = c.at(p) == b'-';
    if separator {
        p += 1;
    }
    if c.at(p) == b'W' {
        p += 1;
        let (week, next) = c.digits(p, 2).ok_or(())?;
        p = next;
        let day = if p < len {
            if separator {
                if c.at(p) != b'-' {
                    return Err(());
                }
                p += 1;
            }
            c.digits(p, 1).ok_or(())?.0
        } else {
            1
        };
        return iso_to_ymd(year, week, day);
    }
    let (month, next) = c.digits(p, 2).ok_or(())?;
    p = next;
    if separator {
        if c.at(p) != b'-' {
            return Err(());
        }
        p += 1;
    }
    let (day, _) = c.digits(p, 2).ok_or(())?;
    Ok((year, month, day))
}

/// `parse_hh_mm_ss_ff`: `Ok(0|1)` like the C return codes, `Err` for < 0.
fn parse_hh_mm_ss_ff(c: &Cursor<'_>, start: usize, end: usize) -> Result<([i64; 4], i32), ()> {
    let mut values = [0i64; 4];
    let mut p = start;
    let mut has_separator = true;
    for (index, slot) in values.iter_mut().take(3).enumerate() {
        let (value, next) = c.digits(p, 2).ok_or(())?;
        *slot = value;
        p = next;
        let character = c.at(p);
        p += 1;
        if index == 0 {
            has_separator = character == b':';
        }
        if character == b'.' || character == b',' {
            if p >= end {
                return Err(());
            }
            break;
        } else if p >= end {
            return Ok((values, i32::from(character != 0)));
        } else if has_separator && character == b':' {
            continue;
        } else if !has_separator {
            p -= 1;
        } else {
            return Err(());
        }
    }
    let remaining = end.saturating_sub(p);
    let to_parse = remaining.min(6);
    let (mut micro, mut next) = c.digits(p, to_parse).ok_or(())?;
    const CORRECTION: [i64; 5] = [100_000, 10_000, 1_000, 100, 10];
    if to_parse < 6 {
        // C indexes correction[to_parse - 1]; to_parse is at least 1 here.
        micro *= CORRECTION
            .get(to_parse.wrapping_sub(1))
            .copied()
            .unwrap_or(1);
    }
    values[3] = micro;
    while c.at(next).is_ascii_digit() {
        next += 1;
    }
    Ok((values, i32::from(c.at(next) != 0)))
}

/// `(hour, minute, second, microsecond, tz)` where tz is `Some(offset_us)`.
type TimeFields = ([i64; 4], Option<i64>);

fn parse_time(c: &Cursor<'_>, start: usize, len: usize) -> Result<TimeFields, ()> {
    let end = start + len;
    let mut tz = start;
    loop {
        if matches!(c.at(tz), b'Z' | b'+' | b'-') {
            break;
        }
        tz += 1;
        if tz >= end {
            break;
        }
    }
    let (time, rv) = parse_hh_mm_ss_ff(c, start, tz)?;
    if tz == end {
        return if rv == 1 { Err(()) } else { Ok((time, None)) };
    }
    if c.at(tz) == b'Z' {
        return if c.at(tz + 1) != 0 {
            Err(())
        } else {
            Ok((time, Some(0)))
        };
    }
    let sign = if c.at(tz) == b'-' { -1 } else { 1 };
    let (offset, rv) = parse_hh_mm_ss_ff(c, tz + 1, end)?;
    if rv != 0 {
        return Err(());
    }
    let seconds = offset[0] * 3600 + offset[1] * 60 + offset[2];
    Ok((time, Some(sign * (seconds * 1_000_000 + offset[3]))))
}

fn days_in_month(year: i64, month: i64) -> i64 {
    let next = if month == 12 {
        NaiveDate::from_ymd_opt(year as i32 + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(year as i32, month as u32 + 1, 1)
    };
    let first = NaiveDate::from_ymd_opt(year as i32, month as u32, 1);
    match (first, next) {
        (Some(first), Some(next)) => (next - first).num_days(),
        _ => 31,
    }
}

/// `datetime.fromisoformat(text)`: microseconds since 0001-01-01T00:00 (local
/// wall time) and the UTC offset in microseconds when one was parsed.
pub fn fromisoformat(text: &str) -> Result<(i64, Option<i64>), String> {
    let invalid = || format!("Invalid isoformat string: {}", py::repr_str(text));
    if text.chars().count() < 7 {
        return Err(invalid());
    }
    let bytes = text.as_bytes();
    let cursor = Cursor { bytes };
    let len = bytes.len();
    let separator = find_separator(&cursor, len);
    let date_len = if separator < 0 {
        usize::MAX
    } else {
        separator as usize
    };
    let (year, month, day) = parse_date(&cursor, date_len).map_err(|()| invalid())?;
    let mut fields: TimeFields = ([0; 4], None);
    if separator >= 0 && len > date_len {
        let mut p = date_len;
        let lead = cursor.at(p);
        p += if lead & 0x80 == 0 {
            1
        } else {
            match lead & 0xf0 {
                0xe0 => 3,
                0xf0 => 4,
                _ => 2,
            }
        };
        let remaining = len.checked_sub(p).ok_or_else(invalid)?;
        fields = parse_time(&cursor, p, remaining).map_err(|()| invalid())?;
    }
    let ([hour, minute, second, micro], offset) = fields;
    if offset.is_some_and(|offset| offset.abs() >= 86_400_000_000) {
        return Err("offset must be a timedelta strictly between -timedelta(hours=24) and timedelta(hours=24).".into());
    }
    if !(1..=9999).contains(&year) {
        return Err(format!("year {year} is out of range"));
    }
    if !(1..=12).contains(&month) {
        return Err("month must be in 1..12".into());
    }
    if day < 1 || day > days_in_month(year, month) {
        return Err("day is out of range for month".into());
    }
    if !(0..=23).contains(&hour) {
        return Err("hour must be in 0..23".into());
    }
    if !(0..=59).contains(&minute) {
        return Err("minute must be in 0..59".into());
    }
    if !(0..=59).contains(&second) {
        return Err("second must be in 0..59".into());
    }
    if !(0..=999_999).contains(&micro) {
        return Err("microsecond must be in 0..999999".into());
    }
    let date =
        NaiveDate::from_ymd_opt(year as i32, month as u32, day as u32).ok_or_else(invalid)?;
    let days = i64::from(date.num_days_from_ce());
    let local = ((days * 86_400 + hour * 3600 + minute * 60 + second) * 1_000_000) + micro;
    Ok((local, offset))
}

fn now_micros() -> i64 {
    let now = chrono::Utc::now().naive_utc();
    let days = i64::from(now.date().num_days_from_ce());
    let seconds = i64::from(now.num_seconds_from_midnight());
    (days * 86_400 + seconds) * 1_000_000 + i64::from(now.and_utc().timestamp_subsec_micros())
}

/// `timestamp(value)`: a UTC instant in microseconds.
pub fn timestamp(value: Option<&PyValue>) -> Result<i64, EvidenceError> {
    let text = match value {
        Some(PyValue::Str(text)) if text.ends_with('Z') => text,
        _ => return Err(value_error("UTC timestamps must end in Z")),
    };
    let iso = format!("{}+00:00", &text[..text.len() - 1]);
    let (local, offset) = fromisoformat(&iso).map_err(EvidenceError::Value)?;
    let Some(offset) = offset else {
        return Err(EvidenceError::Uncaught(
            "AttributeError: 'NoneType' object has no attribute 'total_seconds'".into(),
        ));
    };
    if offset != 0 {
        return Err(value_error("UTC timestamp required"));
    }
    Ok(local - offset)
}

/// `unique_object`: decode JSON bytes rejecting duplicate keys anywhere.
pub fn load_manifest(raw: &[u8]) -> Result<PyValue, EvidenceError> {
    py::loads_bytes(raw, true).map_err(|error| value_error(error.to_string()))
}

fn identity(metadata: &std::fs::Metadata) -> (u64, i64, i64, i64, i64) {
    use std::os::unix::fs::MetadataExt;
    (
        metadata.size(),
        metadata.mtime(),
        metadata.mtime_nsec(),
        metadata.ctime(),
        metadata.ctime_nsec(),
    )
}

/// `artifact_hash(root, relative)`: hash a regular file beneath `root`
/// through no-follow directory handles.
pub fn artifact_hash(root: &Path, relative: Option<&PyValue>) -> Result<String, EvidenceError> {
    let relative = match relative {
        Some(PyValue::Str(text)) if !text.is_empty() && text.chars().count() <= 1024 => text,
        _ => return Err(value_error("bounded relative artifact path required")),
    };
    let parts: Vec<&str> = relative.split('/').collect();
    if parts.iter().any(|part| matches!(*part, "" | "." | "..")) {
        return Err(value_error(
            "artifact path must stay within the evidence root",
        ));
    }
    let directory_flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let root_text = root.to_string_lossy();
    let mut directory = rustix::fs::open(root, directory_flags, Mode::empty())
        .map_err(|error| os_error(&error.into(), &root_text))?;
    let (last, ancestors) = parts
        .split_last()
        .ok_or_else(|| value_error("bounded relative artifact path required"))?;
    for part in ancestors {
        directory = rustix::fs::openat(&directory, *part, directory_flags, Mode::empty())
            .map_err(|error| os_error(&error.into(), part))?;
    }
    let descriptor = rustix::fs::openat(
        &directory,
        *last,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| os_error(&error.into(), last))?;
    let mut file = File::from(descriptor);
    let before = file.metadata().map_err(|error| os_error(&error, last))?;
    if !before.file_type().is_file() || before.len() > MAX_ARTIFACT {
        return Err(value_error("artifact must be a regular file up to 64 MiB"));
    }
    let mut digest = Sha256::new();
    let mut count: u64 = 0;
    let mut chunk = vec![0u8; 65536];
    loop {
        let read = file
            .read(&mut chunk)
            .map_err(|error| os_error(&error, last))?;
        if read == 0 {
            break;
        }
        count += read as u64;
        if count > MAX_ARTIFACT {
            return Err(value_error("artifact exceeds 64 MiB"));
        }
        digest.update(&chunk[..read]);
    }
    let after = file.metadata().map_err(|error| os_error(&error, last))?;
    if count != before.len() || identity(&before) != identity(&after) {
        return Err(value_error("artifact changed while hashing"));
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn is_lower_hex(text: &str) -> bool {
    text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn str_eq(value: Option<&PyValue>, expected: &str) -> bool {
    value.is_some_and(|value| py::py_eq(value, &PyValue::from(expected)))
}

/// `qualify(manifest, root, expected_revision, now)`; `now` is UTC
/// microseconds since 0001-01-01 (default: the current time).
pub fn qualify(
    manifest: &PyValue,
    root: &Path,
    expected_revision: &str,
    now: Option<i64>,
) -> Result<PyDict, EvidenceError> {
    if !(is_lower_hex(expected_revision)
        && (expected_revision.len() == 40 || expected_revision.len() == 64))
    {
        return Err(value_error("full expected source revision required"));
    }
    let manifest = match manifest {
        PyValue::Dict(dict)
            if dict
                .get("schema_version")
                .is_some_and(|v| py::py_eq(v, &PyValue::Int(1)))
                && str_eq(dict.get("source_revision"), expected_revision) =>
        {
            dict
        }
        _ => {
            return Err(value_error(
                "manifest must bind the exact expected source revision",
            ));
        }
    };
    let checks = match manifest.get("checks") {
        Some(PyValue::List(checks)) if checks.len() <= 64 => checks,
        _ => return Err(value_error("up to 64 named checks required")),
    };
    let now = now.unwrap_or_else(now_micros);
    let mut seen: Vec<String> = Vec::new();
    let mut failures: Vec<PyValue> = Vec::new();
    let mut verified: Vec<PyValue> = Vec::new();
    for check in checks {
        let check = match check {
            PyValue::Dict(check) => check,
            _ => return Err(value_error("unknown or duplicate qualification gate")),
        };
        let name = match check.get("id") {
            Some(PyValue::Str(id)) if REQUIRED.contains(&id.as_str()) && !seen.contains(id) => {
                id.clone()
            }
            _ => return Err(value_error("unknown or duplicate qualification gate")),
        };
        seen.push(name.clone());
        let status = match check.get("status") {
            Some(PyValue::Str(status))
                if matches!(status.as_str(), "passed" | "failed" | "unavailable") =>
            {
                status.clone()
            }
            _ => return Err(value_error("explicit gate status required")),
        };
        if status != "passed" {
            failures.push(format!("{name}: {status}").into());
            continue;
        }
        if !str_eq(check.get("revision"), expected_revision) {
            failures.push(format!("{name}: stale source revision").into());
            continue;
        }
        let kind = check.get("kind");
        let known_kind = str_eq(kind, "actual") || str_eq(kind, "synthetic");
        if !known_kind || (actual_required(&name) && !str_eq(kind, "actual")) {
            failures.push(format!("{name}: actual supported journey required").into());
            continue;
        }
        let start = timestamp(check.get("started_utc"))?;
        let finish = timestamp(check.get("finished_utc"))?;
        if finish < start || finish > now {
            failures.push(format!("{name}: invalid or future observation interval").into());
            continue;
        }
        if name == "synthetic_soak" && finish - start < 86_400_000_000 {
            failures.push(
                format!("{name}: at least 24 hours of observed elapsed time required").into(),
            );
            continue;
        }
        let digest = match check.get("sha256") {
            Some(PyValue::Str(digest)) if digest.len() == 64 && is_lower_hex(digest) => digest,
            _ => return Err(value_error("exact SHA256 required for passed evidence")),
        };
        if artifact_hash(root, check.get("evidence"))? != *digest {
            failures.push(format!("{name}: artifact hash mismatch").into());
            continue;
        }
        if name == "complete_cost_trial"
            && !check.get("savings_claim").is_some_and(|v| v.is_bool(false))
        {
            // Economics is reviewed separately. Complete evidence may retain
            // a negative/inconclusive trial without manufacturing savings.
            failures.push(format!("{name}: preflight cannot authorize a savings claim").into());
            continue;
        }
        verified.push(name.into());
    }
    for name in REQUIRED {
        if !seen.iter().any(|seen| seen == name) {
            failures.push(format!("{name}: missing").into());
        }
    }
    let status = if failures.is_empty() {
        "ready_for_independent_review"
    } else {
        "incomplete"
    };
    let report = py_dict! {
        "schema_version" => 1,
        "source_revision" => expected_revision,
        "status" => status,
        "verified_artifacts" => verified,
        "remaining" => failures,
        "full_packet_acceptance" => false,
        "deployment_authorized" => false,
        "savings_claim" => false,
        "scope" => "Source/time/hash preflight of declared evidence; independent review of contents and actual fleet ownership is required.",
    };
    match report {
        PyValue::Dict(report) => Ok(report),
        _ => Err(EvidenceError::Uncaught("report is not a dict".into())),
    }
}

/// `release_evidence.py` arguments.
#[derive(Debug, Clone, clap::Args)]
pub struct EvidenceArgs {
    /// Bounded JSON evidence manifest.
    #[arg(long)]
    pub manifest: PathBuf,
    /// Directory containing the declared evidence artifacts.
    #[arg(long = "evidence-root")]
    pub evidence_root: PathBuf,
    /// Full lowercase 40- or 64-character source revision.
    #[arg(long = "expected-revision")]
    pub expected_revision: String,
}

fn read_manifest(path: &Path) -> Result<Vec<u8>, EvidenceError> {
    let display = path.to_string_lossy();
    let file = File::open(path).map_err(|error| os_error(&error, &display))?;
    let mut raw = Vec::new();
    file.take(MAX_MANIFEST as u64 + 1)
        .read_to_end(&mut raw)
        .map_err(|error| os_error(&error, &display))?;
    if raw.len() > MAX_MANIFEST {
        return Err(value_error("manifest exceeds 1 MiB"));
    }
    Ok(raw)
}

/// Run the preflight and print its JSON report; returns the exit code.
pub fn run_evidence(args: EvidenceArgs) -> crate::RunResult {
    let outcome = read_manifest(&args.manifest)
        .and_then(|raw| load_manifest(&raw))
        .and_then(|manifest| {
            qualify(
                &manifest,
                &args.evidence_root,
                &args.expected_revision,
                None,
            )
        });
    match outcome {
        Ok(report) => {
            let ready = report.get("status").is_some_and(|status| {
                py::py_eq(status, &PyValue::from("ready_for_independent_review"))
            });
            println!("{}", py::dumps(&PyValue::Dict(report), Dump::INDENT2));
            Ok(if ready { 0 } else { 1 })
        }
        Err(EvidenceError::Uncaught(message)) => {
            eprintln!("{message}");
            Ok(1)
        }
        Err(error) => {
            let invalid = py_dict! {
                "status" => "invalid",
                "error" => error.to_string(),
                "full_packet_acceptance" => false,
                "deployment_authorized" => false,
            };
            println!("{}", py::dumps(&invalid, Dump::DEFAULT));
            Ok(2)
        }
    }
}

#[cfg(test)]
#[path = "release_evidence_tests.rs"]
mod tests;
