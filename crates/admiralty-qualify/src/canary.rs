//! Bounded offline HTTP smoke checks for explicitly supplied Admiralty
//! binaries (port of the retired `scripts/check/canary-smoke.py`).
//!
//! The harness never builds or discovers binaries, never consults `PATH`,
//! never attaches to an existing service and never uses the protected legacy
//! ports. It starts the real `admiraltyd` and the CLI demo on loopback port
//! `0` inside private scratch scopes, parses each child's bounded stderr
//! listener announcement, runs fixed read-only checks with a whole-request
//! deadline, terminates its owned children (TERM, bounded wait, KILL) and
//! prints one sorted compact JSON receipt.

use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::http::{self, HttpFailure, HttpResponse, HttpRuntime, Request, SystemRuntime};
use crate::process::{
    BoundedCapture, ChildPoll, cleanup_owned_child, drain_pipe, join_capture, token_urlsafe,
    utc_isoformat_now,
};
use crate::py::{self, Dump, PyDict, PyValue};
use crate::py_dict;
use crate::pynet::{PyIp, ip_address, urlsplit};

/// Largest accepted JSON response.
pub const MAX_JSON_BYTES: usize = 1024 * 1024;
/// Largest accepted path argument or JSON string, in UTF-8 bytes.
pub const MAX_STRING_BYTES: usize = 4096;
/// Largest accepted JSON object/array.
pub const MAX_ITEMS: usize = 1024;
/// Retained child output per stream.
pub const MAX_OUTPUT_BYTES: usize = 64 * 1024;
/// Largest accepted HTTP response body.
pub const MAX_HTTP_BYTES: usize = 1024 * 1024;
/// Child listener startup bound.
pub const MAX_STARTUP_SECONDS: f64 = 15.0;
/// Whole-request HTTP bound.
pub const MAX_REQUEST_SECONDS: f64 = 4.0;
/// Whole-smoke bound.
pub const MAX_TOTAL_SECONDS: f64 = 45.0;
/// Legacy ports that must never be contacted or announced.
pub const PROTECTED_PORTS: [u32; 2] = [18890, 18891];
/// The only supported listener announcement prefix.
pub const LISTEN_PREFIX: &[u8] = b"Admiralty listening on http://";
const LISTEN_SUFFIX: &[u8] = b"(read-only; native execution disabled)";

/// A failed check. `Smoke` mirrors the Python `SmokeError`; `Other` is any
/// other exception class (it is not decorated with child diagnostics).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CanaryError {
    /// `SmokeError`.
    Smoke(String),
    /// Any other exception.
    Other(String),
}

impl CanaryError {
    /// `str(error)`.
    pub fn message(&self) -> &str {
        match self {
            Self::Smoke(message) | Self::Other(message) => message,
        }
    }
}

impl std::fmt::Display for CanaryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.message())
    }
}

impl std::error::Error for CanaryError {}

type Checked<T> = Result<T, CanaryError>;

fn smoke(message: impl Into<String>) -> CanaryError {
    CanaryError::Smoke(message.into())
}

fn other(error: impl std::fmt::Display) -> CanaryError {
    CanaryError::Other(error.to_string())
}

/// `require(condition, message)`.
pub fn require(condition: bool, message: &str) -> Checked<()> {
    if condition {
        Ok(())
    } else {
        Err(smoke(message))
    }
}

/// `require_status(actual, expected, label)`.
pub fn require_status(actual: u16, expected: u16, label: &str) -> Checked<()> {
    require(
        actual == expected,
        &format!("{label} returned HTTP {actual}, expected {expected}"),
    )
}

fn walk_no_symlinks(path: &Path, unavailable: &str, symlink: &str) -> Checked<()> {
    let mut current = PathBuf::from("/");
    for component in path.components() {
        if let Component::Normal(part) = component {
            current.push(part);
            let metadata = fs::symlink_metadata(&current).map_err(|_| smoke(unavailable))?;
            require(!metadata.file_type().is_symlink(), symlink)?;
        }
    }
    Ok(())
}

fn real_uid() -> u32 {
    rustix::process::getuid().as_raw()
}

/// `validate_absolute_owned_file(value, executable=...)`.
pub fn validate_absolute_owned_file(value: &str, executable: bool) -> Checked<PathBuf> {
    require(
        value.len() <= MAX_STRING_BYTES,
        "binary path exceeds string bound",
    )?;
    let path = PathBuf::from(value);
    require(path.is_absolute(), "binary path must be absolute")?;
    require(
        !path.components().any(|c| c == Component::ParentDir),
        "binary path cannot contain parent traversal",
    )?;
    walk_no_symlinks(
        &path,
        "binary path component is unavailable",
        "binary path contains a symlink",
    )?;
    let metadata = fs::symlink_metadata(&path).map_err(other)?;
    require(
        metadata.file_type().is_file(),
        "binary must be a regular file",
    )?;
    require(
        metadata.uid() == real_uid(),
        "binary must be owned by the invoking user",
    )?;
    if executable {
        require(
            rustix::fs::access(&path, rustix::fs::Access::EXEC_OK).is_ok(),
            "binary is not executable",
        )?;
    }
    Ok(path)
}

/// `validate_scratch_root(value)`.
pub fn validate_scratch_root(value: &str) -> Checked<PathBuf> {
    require(
        value.len() <= MAX_STRING_BYTES,
        "scratch path exceeds string bound",
    )?;
    let path = PathBuf::from(value);
    require(path.is_absolute(), "scratch root must be absolute")?;
    require(
        !path.components().any(|c| c == Component::ParentDir),
        "scratch root cannot contain parent traversal",
    )?;
    walk_no_symlinks(
        &path,
        "scratch root component is unavailable",
        "scratch root contains a symlink",
    )?;
    let metadata = fs::symlink_metadata(&path).map_err(other)?;
    require(
        metadata.file_type().is_dir(),
        "scratch root must be a directory",
    )?;
    require(
        metadata.uid() == real_uid(),
        "scratch root must be owned by the invoking user",
    )?;
    require(
        metadata.mode() & 0o077 == 0,
        "scratch root must not be group/world accessible",
    )?;
    Ok(path)
}

/// `validate_listener_line(line)`: the child's `(host, port)` announcement.
pub fn validate_listener_line(line: &[u8]) -> Checked<(String, u32)> {
    let end = line
        .iter()
        .rposition(|byte| !matches!(byte, b'\r' | b'\n'))
        .map_or(0, |position| position + 1);
    let line = &line[..end];
    require(
        line.starts_with(LISTEN_PREFIX),
        "missing supported child listener announcement",
    )?;
    let rest = &line[LISTEN_PREFIX.len()..];
    let (url_bytes, suffix) = match rest.iter().position(|&byte| byte == b' ') {
        Some(space) => (&rest[..space], Some(&rest[space + 1..])),
        None => (rest, None),
    };
    require(
        suffix == Some(LISTEN_SUFFIX),
        "unsupported or future listener announcement",
    )?;
    require(
        url_bytes.len() <= MAX_STRING_BYTES,
        "listener announcement is oversized",
    )?;
    require(url_bytes.is_ascii(), "listener URL is not ASCII")?;
    let url = format!("http://{}", String::from_utf8_lossy(url_bytes));
    let parts = urlsplit(&url).map_err(CanaryError::Other)?;
    require(
        parts.scheme == "http"
            && (parts.path.is_empty() || parts.path == "/")
            && parts.query.is_empty()
            && parts.fragment.is_empty(),
        "listener must be a bare HTTP URL",
    )?;
    require(
        parts.username().is_none() && parts.password().is_none(),
        "listener URL cannot contain user information",
    )?;
    let hostname = parts.hostname();
    require(hostname.is_some(), "listener URL has no host")?;
    let invalid = || smoke("listener must use a literal loopback address and valid port");
    let address = ip_address(hostname.as_deref().unwrap_or_default()).map_err(|_| invalid())?;
    let port = parts.port().map_err(|_| invalid())?;
    require(address.is_loopback(), "child listener is not loopback")?;
    let port = port.filter(|port| (1..=65535).contains(port));
    require(port.is_some(), "child listener port is invalid")?;
    let port = port.unwrap_or_default();
    require(
        !PROTECTED_PORTS.contains(&port),
        "protected legacy port is forbidden",
    )?;
    Ok((address.compressed(), port))
}

/// `redact(data, secret)`: lossy UTF-8, secret replaced, last 4096 characters.
pub fn redact(data: &[u8], secret: &str) -> String {
    let mut text = String::from_utf8_lossy(data).into_owned();
    if !secret.is_empty() {
        text = text.replace(secret, "[redacted]");
    }
    py::tail_chars(&text, 4096)
}

/// `find_listener(capture)`: only LF-terminated records are considered.
pub fn find_listener(capture: &BoundedCapture) -> Checked<Option<(String, u32)>> {
    require(
        !capture.oversized(),
        "child stderr exceeded bounded output storage",
    )?;
    let snapshot = capture.snapshot();
    let mut records: Vec<&[u8]> = snapshot.split(|&byte| byte == b'\n').collect();
    records.pop();
    for record in records {
        if record.starts_with(LISTEN_PREFIX) {
            return validate_listener_line(record).map(Some);
        }
    }
    Ok(None)
}

/// `wait_for_listener(child, capture, deadline)`.
pub fn wait_for_listener(
    child: &mut dyn ChildPoll,
    capture: &BoundedCapture,
    deadline: f64,
) -> Checked<(String, u32)> {
    while http::monotonic() < deadline {
        require(
            !capture.oversized(),
            "child stderr exceeded bounded output storage",
        )?;
        if let Some(listener) = find_listener(capture)? {
            return Ok(listener);
        }
        if let Some(code) = child.poll() {
            return Err(smoke(format!(
                "child exited before listener announcement (exit {code})"
            )));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Err(smoke(
        "child startup deadline elapsed before listener announcement",
    ))
}

/// `wait_for_child_listener`: decorate a `SmokeError` with redacted stderr.
pub fn wait_for_child_listener(
    child: &mut dyn ChildPoll,
    capture: &BoundedCapture,
    deadline: f64,
    secret: &str,
) -> Checked<(String, u32)> {
    match wait_for_listener(child, capture, deadline) {
        Err(CanaryError::Smoke(message)) => {
            let diagnostic = redact(&capture.snapshot(), secret);
            let diagnostic = py::strip(&diagnostic);
            if diagnostic.is_empty() {
                Err(CanaryError::Smoke(message))
            } else {
                Err(CanaryError::Smoke(format!(
                    "{message}; child_stderr={diagnostic}"
                )))
            }
        }
        result => result,
    }
}

/// Mutable request limits (the Python tests patched the module global).
#[derive(Debug, Clone, Copy)]
pub struct HttpLimits {
    /// Per-request whole deadline in seconds.
    pub request_seconds: f64,
}

impl Default for HttpLimits {
    fn default() -> Self {
        Self {
            request_seconds: MAX_REQUEST_SECONDS,
        }
    }
}

/// `bounded_http(host, port, method, path, headers=, body=, deadline=)`.
pub fn bounded_http(
    host: &str,
    port: u32,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&[u8]>,
    deadline: f64,
) -> Checked<(u16, std::collections::BTreeMap<String, String>, Vec<u8>)> {
    bounded_http_with(
        &SystemRuntime,
        HttpLimits::default(),
        HttpCall {
            host,
            port,
            method,
            path,
            headers,
            body,
        },
        deadline,
    )
}

/// One `bounded_http` call's arguments.
pub(crate) struct HttpCall<'a> {
    pub host: &'a str,
    pub port: u32,
    pub method: &'a str,
    pub path: &'a str,
    pub headers: &'a [(&'a str, &'a str)],
    pub body: Option<&'a [u8]>,
}

pub(crate) fn bounded_http_with(
    runtime: &dyn HttpRuntime,
    limits: HttpLimits,
    call: HttpCall<'_>,
    deadline: f64,
) -> Checked<(u16, std::collections::BTreeMap<String, String>, Vec<u8>)> {
    let address: PyIp = ip_address(call.host).map_err(CanaryError::Other)?;
    require(address.is_loopback(), "HTTP destination is not loopback")?;
    require(
        !PROTECTED_PORTS.contains(&call.port),
        "protected legacy port is forbidden",
    )?;
    require(
        call.path.starts_with('/') && !call.path.contains(['\r', '\n']),
        "invalid fixed API path",
    )?;
    if let Some(found) = call
        .path
        .chars()
        .find(|c| (*c as u32) <= 0x20 || *c == '\u{7f}')
    {
        return Err(CanaryError::Other(format!(
            "URL can't contain control characters. {} (found at least {})",
            py::repr_str(call.path),
            py::repr_str(&found.to_string())
        )));
    }
    let request_deadline = deadline.min(runtime.monotonic() + limits.request_seconds);
    require(
        request_deadline - runtime.monotonic() > 0.0,
        "whole-smoke deadline elapsed",
    )?;
    let port = u16::try_from(call.port).map_err(|_| other("port must be 0-65535."))?;
    let compressed = address.compressed();
    let host_header = if address.is_v6() {
        format!("[{compressed}]")
    } else {
        compressed.clone()
    };
    let host_header = if port == 80 {
        host_header
    } else {
        format!("{host_header}:{port}")
    };
    let request = Request {
        method: call.method,
        path: call.path,
        host_header,
        headers: call.headers,
        body: call.body,
    };
    match http::exchange(
        runtime,
        &compressed,
        port,
        &request,
        request_deadline,
        MAX_HTTP_BYTES,
    ) {
        Ok(HttpResponse {
            status,
            headers,
            body,
        }) => Ok((status, headers, body)),
        Err(HttpFailure::Deadline) => Err(smoke("per-request HTTP deadline elapsed")),
        Err(HttpFailure::Transport(_)) => Err(smoke("bounded loopback HTTP request failed")),
        Err(HttpFailure::InvalidContentLength) => Err(smoke("invalid HTTP content length")),
        Err(HttpFailure::TooLarge) => Err(smoke("HTTP response exceeds byte bound")),
        Err(HttpFailure::Truncated) => {
            Err(smoke("HTTP response ended before declared content length"))
        }
    }
}

/// `parse_json_body(data)`: a bounded JSON object.
pub fn parse_json_body(data: &[u8]) -> Checked<PyDict> {
    require(
        data.len() <= MAX_JSON_BYTES,
        "JSON response exceeds one MiB",
    )?;
    let value = py::loads_bytes(data, false).map_err(|_| smoke("response is not valid JSON"))?;
    let PyValue::Dict(dict) = value else {
        return Err(smoke("expected JSON object"));
    };
    let wrapped = PyValue::Dict(dict);
    require(
        json_depth(&wrapped, 0) <= 32,
        "JSON response nesting exceeds bound",
    )?;
    validate_json_bounds(&wrapped)?;
    match wrapped {
        PyValue::Dict(dict) => Ok(dict),
        _ => Err(smoke("expected JSON object")),
    }
}

/// `json_depth(value)` (keys count as one nesting level).
pub fn json_depth(value: &PyValue, depth: usize) -> usize {
    match value {
        PyValue::Dict(dict) => {
            let mut deepest = depth;
            if !dict.is_empty() {
                deepest = deepest.max(depth + 1);
            }
            for item in dict.values() {
                deepest = deepest.max(json_depth(item, depth + 1));
            }
            deepest
        }
        PyValue::List(items) => items
            .iter()
            .map(|item| json_depth(item, depth + 1))
            .fold(depth, usize::max),
        _ => depth,
    }
}

/// `validate_json_bounds(value)`.
pub fn validate_json_bounds(value: &PyValue) -> Checked<()> {
    match value {
        PyValue::Dict(dict) => {
            require(dict.len() <= MAX_ITEMS, "JSON object exceeds item bound")?;
            for (key, item) in dict.iter() {
                require(
                    key.len() <= MAX_STRING_BYTES,
                    "JSON key exceeds string bound",
                )?;
                validate_json_bounds(item)?;
            }
        }
        PyValue::List(items) => {
            require(
                items.len() <= MAX_ITEMS,
                "JSON collection exceeds item bound",
            )?;
            for item in items {
                validate_json_bounds(item)?;
            }
        }
        PyValue::Str(text) => {
            require(text.len() <= MAX_STRING_BYTES, "JSON string exceeds bound")?;
        }
        _ => {}
    }
    Ok(())
}

/// `generated_model_policy()`.
pub fn generated_model_policy() -> PyValue {
    py_dict! {
        "schema_version" => 1,
        "revision" => "0",
        "models" => vec![py_dict! {
            "id" => "canary-smoke-model",
            "label" => "Synthetic canary policy",
            "provider" => "codex_cli",
            "provider_model" => "synthetic-canary-model",
            "tier" => 0,
            "enabled" => true,
            "node_id" => PyValue::Null,
            "endpoint" => PyValue::Null,
            "credential_file_ref" => PyValue::Null,
            "max_concurrency" => 1,
            "capabilities" => vec![PyValue::from("coding")],
            "task_max_attempts" => 1,
            "escalate_to" => PyValue::Null,
        }],
    }
}

/// `write_private(path, data)`: exclusive no-follow create, fsync, `0600`.
pub fn write_private(path: &Path, data: &[u8]) -> io::Result<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(path)?;
    file.write_all(data)?;
    file.flush()?;
    file.sync_all()?;
    drop(file);
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

/// `isolated_env(directory)`.
pub fn isolated_env(directory: &Path) -> Vec<(String, String)> {
    let text = directory.to_string_lossy().into_owned();
    vec![
        ("HOME".into(), text.clone()),
        ("TMPDIR".into(), text.clone()),
        ("TMP".into(), text.clone()),
        ("TEMP".into(), text),
        ("LANG".into(), "C.UTF-8".into()),
        ("LC_ALL".into(), "C.UTF-8".into()),
    ]
}

fn spawn_with_stderr(
    binary: &Path,
    args: &[&str],
    env: &[(String, String)],
    capture: &BoundedCapture,
) -> Checked<Child> {
    let mut child = Command::new(binary)
        .args(args)
        .env_clear()
        .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(other)?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| smoke("child stderr pipe was not created"))?;
    drain_pipe(stderr, capture);
    Ok(child)
}

fn record_check<T>(
    results: &mut Vec<PyValue>,
    name: &str,
    check: impl FnOnce() -> Checked<T>,
) -> Checked<T> {
    let started = http::monotonic();
    let outcome = check();
    let duration_ms = py::round_half_even((http::monotonic() - started) * 1000.0);
    match &outcome {
        Ok(_) => results.push(py_dict! {
            "name" => name,
            "state" => "pass",
            "duration_ms" => duration_ms,
        }),
        Err(error) => results.push(py_dict! {
            "name" => name,
            "state" => "fail",
            "duration_ms" => duration_ms,
            "diagnostic" => py::truncate_chars(error.message(), 512),
        }),
    }
    outcome
}

/// `validate_fleet(value, simulated=...)`.
pub fn validate_fleet(value: &PyDict, simulated: bool) -> Checked<()> {
    require(
        value.get("simulated").is_some_and(|v| v.is_bool(simulated)),
        "fleet simulated mode did not match expected mode",
    )?;
    require(
        value
            .get("native_execution_qualified")
            .is_some_and(|v| v.is_bool(false)),
        "fleet missing native_execution_qualified=false",
    )
}

/// `validate_models(value)`.
pub fn validate_models(value: &PyDict) -> Checked<()> {
    require(
        value
            .get("native_execution_qualified")
            .is_some_and(|v| v.is_bool(false)),
        "models missing native_execution_qualified=false",
    )?;
    let models = value.get("models").and_then(PyValue::as_list);
    require(
        models.is_some_and(|models| models.len() == 1),
        "synthetic model policy was not visible",
    )?;
    let first = models.and_then(|models| models.first());
    let first = first.ok_or_else(|| smoke("synthetic model policy was not visible"))?;
    let PyValue::Dict(first) = first else {
        return Err(CanaryError::Other(format!(
            "'{}' object has no attribute 'get'",
            first.type_name()
        )));
    };
    require(
        first
            .get("id")
            .is_some_and(|id| py::py_eq(id, &PyValue::from("canary-smoke-model"))),
        "unexpected model policy",
    )
}

fn bearer(token: &str) -> String {
    format!("Bearer {token}")
}

fn server_checks(
    host: &str,
    port: u32,
    token: &str,
    deadline: f64,
    results: &mut Vec<PyValue>,
) -> Checked<()> {
    record_check(results, "real_health_native_unqualified", || {
        let (status, _, body) = bounded_http(host, port, "GET", "/healthz", &[], None, deadline)?;
        let value = parse_json_body(&body)?;
        require_status(status, 200, "health")?;
        require(
            value
                .get("status")
                .is_some_and(|s| py::py_eq(s, &PyValue::from("ok"))),
            "health check did not return ok",
        )?;
        require(
            value
                .get("native_execution_qualified")
                .is_some_and(|v| v.is_bool(false)),
            "health reported native execution qualified",
        )
    })?;
    for (path, name) in [("/v1/fleet", "real_fleet"), ("/v1/models", "real_models")] {
        record_check(results, &format!("unauthorized_{name}_rejected"), || {
            let (status, _, _) = bounded_http(host, port, "GET", path, &[], None, deadline)?;
            require_status(status, 401, &format!("unauthorized {path}"))
        })?;
    }
    record_check(results, "model_auth_precedes_body_validation", || {
        let (status, _, _) = bounded_http(
            host,
            port,
            "POST",
            "/v1/models/select",
            &[("Content-Type", "application/json")],
            Some(b"{"),
            deadline,
        )?;
        require_status(status, 401, "unauthorized malformed model request")
    })?;
    let authorization = bearer(token);
    record_check(results, "authorized_real_fleet", || {
        let (status, _, body) = bounded_http(
            host,
            port,
            "GET",
            "/v1/fleet",
            &[("Authorization", &authorization)],
            None,
            deadline,
        )?;
        let value = parse_json_body(&body)?;
        require_status(status, 200, "authorized real fleet")?;
        validate_fleet(&value, false)
    })?;
    record_check(results, "authorized_models_policy", || {
        let (status, _, body) = bounded_http(
            host,
            port,
            "GET",
            "/v1/models",
            &[("Authorization", &authorization)],
            None,
            deadline,
        )?;
        let value = parse_json_body(&body)?;
        require_status(status, 200, "authorized model listing")?;
        validate_models(&value)
    })?;
    record_check(results, "model_selection_proposal_only", || {
        let request = py_dict! {
            "request" => py_dict! { "required_capabilities" => vec![PyValue::from("coding")] },
            "capacity" => PyDict::new(),
        };
        let body = py::dumps(&request, Dump::COMPACT);
        let (status, _, body) = bounded_http(
            host,
            port,
            "POST",
            "/v1/models/select",
            &[
                ("Authorization", &authorization),
                ("Content-Type", "application/json"),
            ],
            Some(body.as_bytes()),
            deadline,
        )?;
        let value = parse_json_body(&body)?;
        require_status(status, 200, "model selection proposal")?;
        require(
            value
                .get("status")
                .is_some_and(|s| py::py_eq(s, &PyValue::from("selected"))),
            "model selection proposal failed",
        )?;
        require(
            value
                .get("model_id")
                .is_some_and(|s| py::py_eq(s, &PyValue::from("canary-smoke-model"))),
            "model selection proposal changed policy",
        )?;
        let cost_status = match value.get("usage_cost") {
            None => None,
            Some(PyValue::Dict(cost)) => cost.get("status"),
            Some(other_value) => {
                return Err(CanaryError::Other(format!(
                    "'{}' object has no attribute 'get'",
                    other_value.type_name()
                )));
            }
        };
        require(
            cost_status.is_some_and(|s| py::py_eq(s, &PyValue::from("unknown"))),
            "model proposal implied known cost",
        )
    })?;
    Ok(())
}

fn demo_checks(host: &str, port: u32, deadline: f64, results: &mut Vec<PyValue>) -> Checked<()> {
    record_check(results, "demo_fleet_simulated", || {
        let (status, _, body) = bounded_http(host, port, "GET", "/v1/fleet", &[], None, deadline)?;
        let value = parse_json_body(&body)?;
        require_status(status, 200, "demo fleet")?;
        validate_fleet(&value, true)
    })?;
    for (method, path, name) in [
        ("GET", "/v1/models", "models"),
        ("PUT", "/v1/models", "model_policy_write"),
        ("GET", "/v1/legacy", "legacy_policy"),
    ] {
        record_check(results, &format!("demo_no_{name}"), || {
            let body: Option<&[u8]> = if method == "PUT" { Some(b"{}") } else { None };
            let (status, _, _) = bounded_http(
                host,
                port,
                method,
                path,
                &[("Content-Type", "application/json")],
                body,
                deadline,
            )?;
            require_status(status, 404, &format!("demo {path}"))
        })?;
    }
    Ok(())
}

/// `run_readonly(binary, args, env, deadline)`: `(returncode, stdout, stderr)`.
pub fn run_readonly(
    binary: &Path,
    args: &[&str],
    env: &[(String, String)],
    deadline: f64,
) -> Checked<(i32, Vec<u8>, Vec<u8>)> {
    let mut child = Command::new(binary)
        .args(args)
        .env_clear()
        .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(other)?;
    let stdout_capture = BoundedCapture::new(MAX_OUTPUT_BYTES);
    let stderr_capture = BoundedCapture::new(MAX_OUTPUT_BYTES);
    let result = (|| {
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let (Some(stdout), Some(stderr)) = (stdout, stderr) else {
            return Err(smoke("CLI output pipes were not created"));
        };
        drain_pipe(stdout, &stdout_capture);
        drain_pipe(stderr, &stderr_capture);
        let remaining = MAX_REQUEST_SECONDS.min(deadline - http::monotonic());
        require(remaining > 0.0, "whole-smoke deadline elapsed")?;
        let status = crate::process::wait_timeout(&mut child, Duration::from_secs_f64(remaining))
            .map_err(other)?
            .ok_or_else(|| smoke("read-only CLI command exceeded deadline"))?;
        join_capture(&stdout_capture, Duration::from_millis(100));
        join_capture(&stderr_capture, Duration::from_millis(100));
        require(
            !stdout_capture.oversized() && !stderr_capture.oversized(),
            "read-only CLI output exceeded storage bound",
        )?;
        Ok((
            crate::process::returncode(status),
            stdout_capture.snapshot(),
            stderr_capture.snapshot(),
        ))
    })();
    if child.poll().is_none() {
        cleanup_owned_child(&mut child).map_err(smoke)?;
    }
    result
}

/// `binary_digest(path)`: `sha256:<hex>`.
pub fn binary_digest(path: &Path) -> io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut digest = Sha256::new();
    io::copy(&mut file, &mut digest)?;
    Ok(format!("sha256:{:x}", digest.finalize()))
}

#[derive(Default)]
struct SmokeScope {
    real_root: Option<PathBuf>,
    demo_root: Option<PathBuf>,
    real_child: Option<Child>,
    demo_child: Option<Child>,
}

/// `tempfile.mkdtemp(prefix=, dir=)` then `chmod 0700`; the directory is
/// recorded in `slot` before the chmod so cleanup always removes it.
fn make_private_dir(
    slot: &mut Option<PathBuf>,
    scratch: &Path,
    prefix: &str,
) -> io::Result<PathBuf> {
    let directory = tempfile::Builder::new()
        .prefix(prefix)
        .tempdir_in(scratch)?
        .keep();
    *slot = Some(directory.clone());
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
    Ok(directory)
}

fn smoke_body(
    scope: &mut SmokeScope,
    server_binary: &Path,
    cli_binary: &Path,
    scratch: &Path,
    deadline: f64,
    results: &mut Vec<PyValue>,
) -> Checked<()> {
    let real_root =
        make_private_dir(&mut scope.real_root, scratch, "adm-canary-real-").map_err(other)?;
    let demo_root =
        make_private_dir(&mut scope.demo_root, scratch, "adm-canary-demo-").map_err(other)?;
    let token = token_urlsafe().map_err(other)?;
    let token_file = real_root.join("auth.token");
    let model_file = real_root.join("models.json");
    let database = real_root.join("state.sqlite");
    write_private(&token_file, format!("{token}\n").as_bytes()).map_err(other)?;
    let policy = py::dumps(&generated_model_policy(), Dump::COMPACT);
    write_private(&model_file, policy.as_bytes()).map_err(other)?;
    let real_capture = BoundedCapture::new(MAX_OUTPUT_BYTES);
    let demo_capture = BoundedCapture::new(MAX_OUTPUT_BYTES);
    let env = isolated_env(&real_root);
    let demo_env = isolated_env(&demo_root);
    let server_args = [
        "--database".to_owned(),
        database.to_string_lossy().into_owned(),
        "--token-file".to_owned(),
        token_file.to_string_lossy().into_owned(),
        "--models-file".to_owned(),
        model_file.to_string_lossy().into_owned(),
        "--listen".to_owned(),
        "127.0.0.1:0".to_owned(),
    ];
    let server_args: Vec<&str> = server_args.iter().map(String::as_str).collect();
    let real_child = scope.real_child.insert(spawn_with_stderr(
        server_binary,
        &server_args,
        &env,
        &real_capture,
    )?);
    let (real_host, real_port) = record_check(results, "real_server_startup", || {
        let startup = deadline.min(http::monotonic() + MAX_STARTUP_SECONDS);
        wait_for_child_listener(real_child, &real_capture, startup, &token)
    })?;
    server_checks(&real_host, real_port, &token, deadline, results)?;
    let demo_child = scope.demo_child.insert(spawn_with_stderr(
        cli_binary,
        &["demo", "--listen", "127.0.0.1:0"],
        &demo_env,
        &demo_capture,
    )?);
    let (demo_host, demo_port) = record_check(results, "demo_startup", || {
        let startup = deadline.min(http::monotonic() + MAX_STARTUP_SECONDS);
        wait_for_child_listener(demo_child, &demo_capture, startup, &token)
    })?;
    require(
        (demo_host.as_str(), demo_port) != (real_host.as_str(), real_port),
        "real and demo listeners unexpectedly match",
    )?;
    demo_checks(&demo_host, demo_port, deadline, results)?;

    let (doctor_code, doctor_out, _doctor_err) =
        run_readonly(cli_binary, &["doctor"], &demo_env, deadline)?;
    let doctor = parse_json_body(&doctor_out)?;
    require(
        doctor_code == 0
            && doctor
                .get("native_execution_qualified")
                .is_some_and(|v| v.is_bool(false)),
        "doctor did not report native execution unqualified",
    )?;
    results.push(py_dict! { "name" => "doctor_read_only_capability", "state" => "pass" });
    let (help_code, help_out, help_err) =
        run_readonly(cli_binary, &["demo", "--help"], &demo_env, deadline)?;
    let mut combined = help_out;
    combined.extend_from_slice(&help_err);
    let help_text = String::from_utf8_lossy(&combined);
    require(
        help_code == 0
            && !help_text.contains("--models-file")
            && !help_text.contains("--legacy-status-file"),
        "demo CLI advertises real model or legacy configuration flags",
    )?;
    results.push(py_dict! { "name" => "demo_cli_has_no_real_policy_flags", "state" => "pass" });
    Ok(())
}

const CLOCK: &str = "UTC start timestamp plus monotonic elapsed/deadline measurements";
const RANDOMNESS: &str = "ephemeral token from OS CSPRNG; token value not recorded";

fn binaries(server: &Path, cli: &Path) -> Checked<PyValue> {
    Ok(py_dict! {
        "admiraltyd_sha256" => binary_digest(server).map_err(other)?,
        "admiralty_sha256" => binary_digest(cli).map_err(other)?,
    })
}

/// `execute_smoke(server, cli, scratch)`: the receipt, or the error that
/// escaped (validation before the scoped run, or a failed cleanup).
pub fn execute_smoke(server_arg: &str, cli_arg: &str, scratch_arg: &str) -> Checked<PyDict> {
    let started_at = utc_isoformat_now();
    let started = http::monotonic();
    let deadline = started + MAX_TOTAL_SECONDS;
    let mut results = Vec::new();
    let server_binary = validate_absolute_owned_file(server_arg, true)?;
    let cli_binary = validate_absolute_owned_file(cli_arg, true)?;
    let scratch = validate_scratch_root(scratch_arg)?;
    require(
        server_binary != cli_binary,
        "server and CLI binaries must be distinct paths",
    )?;
    let mut scope = SmokeScope::default();
    let outcome = smoke_body(
        &mut scope,
        &server_binary,
        &cli_binary,
        &scratch,
        deadline,
        &mut results,
    );
    let elapsed_ms = || py::round_half_even((http::monotonic() - started) * 1000.0);
    let build = |duration_ms: i64, diagnostic: Option<&CanaryError>, binaries: PyValue| {
        let mut receipt = PyDict::new();
        receipt.insert("started_at_utc", started_at.as_str());
        receipt.insert("duration_ms", duration_ms);
        receipt.insert("status", if diagnostic.is_none() { "pass" } else { "fail" });
        receipt.insert("synthetic", false);
        receipt.insert("clock", CLOCK);
        receipt.insert("randomness", RANDOMNESS);
        receipt.insert("checks", results.clone());
        if let Some(error) = diagnostic {
            receipt.insert("diagnostic", py::truncate_chars(error.message(), 512));
        }
        receipt.insert("binaries", binaries);
        receipt
    };
    // As in the Python `try`/`except`, a digest failure while building the
    // passing receipt becomes the failure diagnostic (digests are retried).
    let duration_ms = elapsed_ms();
    let receipt = match outcome {
        Ok(()) => match binaries(&server_binary, &cli_binary) {
            Ok(digests) => Ok(build(duration_ms, None, digests)),
            Err(error) => {
                let duration_ms = elapsed_ms();
                binaries(&server_binary, &cli_binary)
                    .map(|digests| build(duration_ms, Some(&error), digests))
            }
        },
        Err(error) => binaries(&server_binary, &cli_binary)
            .map(|digests| build(duration_ms, Some(&error), digests)),
    };
    let mut cleanup = PyDict::new();
    if let Some(child) = scope.demo_child.as_mut() {
        cleanup.insert("demo", cleanup_owned_child(child).map_err(smoke)?.as_str());
    }
    if let Some(child) = scope.real_child.as_mut() {
        cleanup.insert(
            "admiraltyd",
            cleanup_owned_child(child).map_err(smoke)?.as_str(),
        );
    }
    for directory in [scope.demo_root.as_ref(), scope.real_root.as_ref()]
        .into_iter()
        .flatten()
    {
        let removed = fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
            .and_then(|()| fs::remove_dir_all(directory));
        if removed.is_err() {
            cleanup.insert("scratch_cleanup", "incomplete");
        }
    }
    let mut receipt = receipt?;
    receipt.insert("cleanup", cleanup);
    receipt.insert("acceptance", false);
    receipt.insert("fleet_rollout_closure", false);
    Ok(receipt)
}

/// `admiralty-canary` smoke arguments (all required for a real smoke).
#[derive(Debug, Clone, Default, clap::Args)]
pub struct SmokeArgs {
    /// Absolute path of the owned `admiraltyd` executable.
    #[arg(long = "admiraltyd-binary")]
    pub admiraltyd_binary: Option<String>,
    /// Absolute path of the owned `admiralty` executable.
    #[arg(long = "admiralty-binary")]
    pub admiralty_binary: Option<String>,
    /// Absolute private (0700, owned) scratch directory.
    #[arg(long = "scratch-root")]
    pub scratch_root: Option<String>,
}

const MISSING_ARGS: &str =
    "real smoke requires explicit --admiraltyd-binary, --admiralty-binary and --scratch-root";

/// `parse_args`: the explicit binary and scratch triple, or the usage error.
pub fn resolve_args(args: &SmokeArgs) -> Result<(&str, &str, &str), String> {
    match (
        args.admiraltyd_binary.as_deref().filter(|v| !v.is_empty()),
        args.admiralty_binary.as_deref().filter(|v| !v.is_empty()),
        args.scratch_root.as_deref().filter(|v| !v.is_empty()),
    ) {
        (Some(server), Some(cli), Some(scratch)) => Ok((server, cli, scratch)),
        _ => Err(MISSING_ARGS.into()),
    }
}

/// Run the real smoke and print the sorted compact receipt. Exit 0 on pass,
/// 1 on fail and 2 on a usage error.
pub fn run(args: SmokeArgs) -> Result<i32, Box<dyn std::error::Error + Send + Sync>> {
    let (server, cli, scratch) = match resolve_args(&args) {
        Ok(triple) => triple,
        Err(message) => {
            eprintln!("admiralty-canary: error: {message}");
            return Ok(2);
        }
    };
    let receipt = match execute_smoke(server, cli, scratch) {
        Ok(receipt) => receipt,
        Err(error) => {
            let mut receipt = PyDict::new();
            receipt.insert("started_at_utc", utc_isoformat_now());
            receipt.insert("status", "fail");
            receipt.insert("synthetic", false);
            receipt.insert("checks", Vec::<PyValue>::new());
            receipt.insert("diagnostic", py::truncate_chars(error.message(), 512));
            receipt.insert("acceptance", false);
            receipt.insert("fleet_rollout_closure", false);
            receipt
        }
    };
    let passed = receipt
        .get("status")
        .is_some_and(|status| py::py_eq(status, &PyValue::from("pass")));
    let receipt = PyValue::Dict(receipt);
    println!("{}", py::dumps(&receipt, Dump::COMPACT_SORTED));
    Ok(if passed { 0 } else { 1 })
}

#[cfg(test)]
#[path = "canary_tests.rs"]
mod tests;
