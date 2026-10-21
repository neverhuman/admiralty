//! Read-only native queue health (port of `tools/native_queue_health.py`).
//!
//! Queue persistence never proves consumption. This module performs no
//! dispatch, UI input, provider calls, database mutation, session resume,
//! deletion or retry. Missing/incomplete evidence stays `unknown`; an
//! unreadable database is never reported as an empty queue. `payload_json`
//! is never selected.

use std::path::{Component, Path, PathBuf};

use rusqlite::{Connection, OpenFlags, types::Value};

use crate::py::{J, Obj, PyErr};

/// Default staleness bound (5 minutes).
pub const DEFAULT_STALE_MS: i64 = 300_000;
/// Default/maximum number of threads reported.
pub const DEFAULT_MAX_THREADS: i64 = 128;

/// `pathlib.Path(path).resolve()` (non-strict).
pub fn resolve_lenient(path: &Path) -> PathBuf {
    if let Ok(p) = std::fs::canonicalize(path) {
        return p;
    }
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|d| d.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };
    let mut existing;
    let mut rest: Vec<std::ffi::OsString> = Vec::new();
    let mut normalized = PathBuf::new();
    for c in abs.components() {
        match c {
            Component::ParentDir => {
                normalized.pop();
            }
            Component::CurDir => {}
            other => normalized.push(other.as_os_str()),
        }
    }
    let mut cur = normalized.clone();
    loop {
        if let Ok(p) = std::fs::canonicalize(&cur) {
            existing = p;
            break;
        }
        match (
            cur.file_name().map(|n| n.to_os_string()),
            cur.parent().map(Path::to_path_buf),
        ) {
            (Some(name), Some(parent)) => {
                rest.push(name);
                cur = parent;
            }
            _ => {
                existing = normalized;
                rest.clear();
                break;
            }
        }
    }
    for name in rest.iter().rev() {
        existing.push(name);
    }
    existing
}

/// `Path.as_uri()` percent-encoding of an absolute path.
fn file_uri(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut out = String::from("file://");
    for b in path.as_os_str().as_bytes() {
        let c = *b;
        if c.is_ascii_alphanumeric() || matches!(c, b'/' | b'_' | b'.' | b'-' | b'~') {
            out.push(c as char);
        } else {
            out.push_str(&format!("%{c:02X}"));
        }
    }
    out
}

/// Python's sqlite3 exception class for a rusqlite error.
fn sqlite_error_name(e: &rusqlite::Error) -> &'static str {
    use rusqlite::ErrorCode as C;
    match e {
        rusqlite::Error::SqliteFailure(f, _) => match f.code {
            C::InternalMalfunction | C::NotFound => "InternalError",
            C::OutOfMemory => "MemoryError",
            C::DatabaseCorrupt | C::NotADatabase | C::Unknown => "DatabaseError",
            C::TooBig => "DataError",
            C::ConstraintViolation | C::TypeMismatch => "IntegrityError",
            C::ApiMisuse | C::ParameterOutOfRange => "InterfaceError",
            _ => "OperationalError",
        },
        rusqlite::Error::NulError(_) | rusqlite::Error::InvalidPath(_) => "ValueError",
        _ => "OperationalError",
    }
}

/// Python `repr(bytes)`.
fn bytes_repr(b: &[u8]) -> String {
    let quote = if b.contains(&b'\'') && !b.contains(&b'"') {
        '"'
    } else {
        '\''
    };
    let mut out = format!("b{quote}");
    for &c in b {
        match c {
            b'\\' => out.push_str("\\\\"),
            b'\t' => out.push_str("\\t"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            c if c as char == quote => {
                out.push('\\');
                out.push(c as char);
            }
            0x20..=0x7e => out.push(c as char),
            c => out.push_str(&format!("\\x{c:02x}")),
        }
    }
    out.push(quote);
    out
}

fn value_to_j(v: Value) -> J {
    match v {
        Value::Null => J::Null,
        Value::Integer(i) => J::Int(i),
        Value::Real(f) => J::Float(f),
        Value::Text(s) => J::Str(s),
        Value::Blob(b) => J::Str(bytes_repr(&b)),
    }
}

struct Row {
    thread_id: J,
    count: i64,
    oldest: Value,
    newest: Value,
}

fn query(path: &Path, max_threads: i64) -> rusqlite::Result<Vec<Row>> {
    // mode=ro deliberately retains SQLite's WAL handling; immutable=1 would
    // miss a live writer's committed WAL rows. Never copy/read payload_json.
    let uri = format!("{}?mode=ro", file_uri(path));
    let conn = Connection::open_with_flags(
        uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(std::time::Duration::from_millis(250))?;
    conn.execute_batch("PRAGMA query_only=ON")?;
    conn.execute_batch("PRAGMA busy_timeout=250")?;
    let mut budget = 0u32;
    // at most about one million SQLite VM steps
    conn.progress_handler(
        1000,
        Some(move || {
            budget += 1;
            budget > 1000
        }),
    )?;
    let mut stmt = conn.prepare(
        "SELECT thread_id, COUNT(*), MIN(created_at_ms), MAX(created_at_ms) \
         FROM queued_items GROUP BY thread_id ORDER BY MIN(created_at_ms), thread_id LIMIT ?",
    )?;
    let rows = stmt.query_map([max_threads + 1], |r| {
        Ok(Row {
            thread_id: value_to_j(r.get::<_, Value>(0)?),
            count: r.get::<_, i64>(1)?,
            oldest: r.get::<_, Value>(2)?,
            newest: r.get::<_, Value>(3)?,
        })
    })?;
    let collected: rusqlite::Result<Vec<Row>> = rows.collect();
    collected
}

/// `read_queue_health(path, now_ms=..., max_threads=..., stale_ms=...)`.
///
/// Returns `ValueError` for invalid bounds, like the Python version.
pub fn read_queue_health(
    path: &Path,
    now_ms: i64,
    max_threads: i64,
    stale_ms: i64,
) -> Result<J, PyErr> {
    let path = resolve_lenient(path);
    if !(1..=128).contains(&max_threads) || stale_ms <= 0 {
        return Err(PyErr::value_error("invalid queue observation bounds"));
    }
    let mut report = Obj::new();
    report.set("status", "unknown");
    report.set("complete", false);
    report.set("threads", J::Arr(Vec::new()));
    report.set("observed_ms", now_ms);
    report.set("source", path.to_string_lossy().to_string());
    report.set("consumption_verified", false);
    report.set("read_only", true);
    if !path.is_file() {
        report.set("reason", "queue database missing; no empty-queue inference");
        return Ok(J::Obj(report));
    }
    match query(&path, max_threads) {
        Ok(rows) => {
            let limit = usize::try_from(max_threads).unwrap_or(0);
            let complete = rows.len() <= limit;
            report.set("complete", complete);
            let mut threads = Vec::new();
            for row in rows.into_iter().take(limit) {
                let (oldest, newest) = (row.oldest, row.newest);
                let clock = match (&oldest, &newest) {
                    (Value::Integer(o), Value::Integer(n)) if 0 <= *o && o <= n && *n <= now_ms => {
                        Some(*o)
                    }
                    _ => None,
                };
                let age = clock.map(|o| now_ms - o);
                let state = match age {
                    None => "clock_unknown",
                    Some(a) if a >= stale_ms || row.count >= 16 => "stalled",
                    Some(_) => "pending",
                };
                threads.push(crate::jobj! {
                    "thread_id" => row.thread_id,
                    "pending" => row.count,
                    "oldest_created_ms" => value_to_j(oldest),
                    "newest_created_ms" => value_to_j(newest),
                    "oldest_age_ms" => age,
                    "state" => state,
                    "consumption_verified" => false,
                });
            }
            report.set("threads", J::Arr(threads));
            report.set("status", if complete { "observed" } else { "incomplete" });
        }
        Err(e) => {
            report.set(
                "reason",
                format!("{}: queue state unavailable", sqlite_error_name(&e)),
            );
            report.set("threads", J::Arr(Vec::new()));
            report.set("status", "unknown");
            report.set("complete", false);
        }
    }
    Ok(J::Obj(report))
}

/// `require_live_target(thread_id, cwd, thread, turn)`: preflight only; never
/// resume a notLoaded thread to make this check pass. A caller must
/// additionally bind the live process incarnation and its actual endpoint.
/// Transport acceptance remains distinct from recipient consumption.
pub fn require_live_target(thread_id: &str, cwd: &str, thread: &J, turn: &J) -> Result<J, PyErr> {
    let get = |j: &J, k: &str| {
        j.as_obj()
            .and_then(|o| o.get(k))
            .cloned()
            .unwrap_or_default()
    };
    if !crate::py::eq_str(&get(thread, "id"), thread_id)
        || !crate::py::eq_str(&get(thread, "cwd"), cwd)
    {
        return Err(PyErr::value_error("recipient identity/cwd mismatch"));
    }
    let status_obj = thread
        .as_obj()
        .and_then(|o| o.get("status"))
        .cloned()
        .unwrap_or_else(|| J::Obj(Obj::new()));
    let status = match &status_obj {
        J::Obj(o) => o.get("type").cloned().unwrap_or_default(),
        other => return Err(PyErr::attr_get(other)),
    };
    if crate::py::eq_str(&status, "notLoaded") {
        return Err(PyErr::value_error(
            "recipient not loaded on this endpoint; do not resume or queue as fallback",
        ));
    }
    if !crate::py::eq_str(&status, "active")
        || !turn.truthy()
        || !crate::py::eq_str(&get(turn, "status"), "inProgress")
        || !get(turn, "id").truthy()
    {
        return Err(PyErr::value_error(
            "no verified active turn for same-turn steering",
        ));
    }
    Ok(get(turn, "id"))
}
