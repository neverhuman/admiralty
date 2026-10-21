//! Parity against outcomes recorded from the retired Python harnesses.
//!
//! `tests/fixtures/python_parity.json` was produced by running CPython 3.13
//! against `scripts/release/package.py`, `scripts/check/canary-smoke.py` and
//! `tests/qualification/release_evidence.py` immediately before they were
//! deleted. Each vector holds an input and the Python result (`{"ok": ...}`
//! or `{"error": <exception class>, "message": <str(error)>}`); byte inputs
//! are run-length segments `[[base64(pattern), repeat], ...]`.
//!
//! Documented, deliberate differences are asserted explicitly:
//! non-ASCII archive paths are refused (NFC is not provable without Unicode
//! tables), and JSON syntax-error wording comes from `serde_json`.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::thread;
use std::time::Duration;

use admiralty_qualify::canary::{self, CanaryError};
use admiralty_qualify::http::monotonic;
use admiralty_qualify::py::{self, Dump, PyValue};
use admiralty_qualify::pynet;
use admiralty_qualify::release_evidence::{self, EvidenceError};
use admiralty_qualify::source_package::{self, SourcePackageError};
use sha2::{Digest, Sha256};

fn fixture() -> PyValue {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/python_parity.json");
    let raw = std::fs::read(&path).expect("read parity fixture");
    py::loads_bytes(&raw, false).expect("parse parity fixture")
}

fn section<'a>(fixture: &'a PyValue, name: &str) -> &'a [PyValue] {
    fixture
        .get(name)
        .and_then(PyValue::as_list)
        .unwrap_or_else(|| panic!("fixture section {name}"))
}

fn base64_decode(text: &str) -> Vec<u8> {
    let value = |c: u8| -> u32 {
        match c {
            b'A'..=b'Z' => u32::from(c - b'A'),
            b'a'..=b'z' => u32::from(c - b'a') + 26,
            b'0'..=b'9' => u32::from(c - b'0') + 52,
            b'+' => 62,
            b'/' => 63,
            _ => panic!("invalid base64 {c}"),
        }
    };
    let symbols: Vec<u8> = text.bytes().filter(|c| *c != b'=').collect();
    let mut out = Vec::new();
    for group in symbols.chunks(4) {
        let mut accumulator = 0u32;
        for (index, symbol) in group.iter().enumerate() {
            accumulator |= value(*symbol) << (18 - 6 * index);
        }
        let bytes = accumulator.to_be_bytes();
        out.extend_from_slice(&bytes[1..group.len()]);
    }
    out
}

fn segments(value: &PyValue) -> Vec<u8> {
    let mut out = Vec::new();
    for segment in value.as_list().expect("segment list") {
        let parts = segment.as_list().expect("segment pair");
        let pattern = base64_decode(parts[0].as_str().expect("segment base64"));
        let PyValue::Int(count) = parts[1] else {
            panic!("segment count");
        };
        for _ in 0..count {
            out.extend_from_slice(&pattern);
        }
    }
    out
}

fn text(value: &PyValue) -> String {
    String::from_utf8(segments(value)).expect("UTF-8 vector")
}

enum Expected<'a> {
    Ok(&'a PyValue),
    Error { class: &'a str, message: &'a str },
}

fn expected(outcome: &PyValue) -> Expected<'_> {
    if let Some(value) = outcome.get("ok") {
        return Expected::Ok(value);
    }
    Expected::Error {
        class: outcome
            .get("error")
            .and_then(PyValue::as_str)
            .expect("error class"),
        message: outcome
            .get("message")
            .and_then(PyValue::as_str)
            .expect("error message"),
    }
}

fn package_matches(
    label: &str,
    input: &str,
    actual: Result<String, SourcePackageError>,
    outcome: &PyValue,
    failures: &mut Vec<String>,
) {
    match (expected(outcome), actual) {
        (Expected::Ok(value), Ok(actual)) if value.as_str() == Some(actual.as_str()) => {}
        // Documented stricter rule: non-ASCII archive paths are refused.
        (Expected::Ok(_), Err(SourcePackageError::Invalid(message)))
            if label == "archive_path" && !input.is_ascii() =>
        {
            assert_eq!(message, "archive_path must use NFC Unicode normalization");
        }
        (
            Expected::Error {
                class: "PackageError",
                message,
            },
            Err(SourcePackageError::Invalid(actual)),
        ) if message == actual => {}
        (
            Expected::Error {
                class: "PackageError",
                ..
            },
            Err(SourcePackageError::Invalid(_)),
        ) if label == "archive_path" && !input.is_ascii() => {}
        (expected_outcome, actual) => failures.push(format!(
            "{label} {:?}: python {} / rust {actual:?}",
            py::truncate_chars(input, 80),
            describe(&expected_outcome)
        )),
    }
}

fn describe(outcome: &Expected<'_>) -> String {
    match outcome {
        Expected::Ok(value) => format!(
            "ok {}",
            py::truncate_chars(&py::dumps(value, Dump::COMPACT), 200)
        ),
        Expected::Error { class, message } => format!("{class}: {message}"),
    }
}

/// `_archive_path` and `_source_path` against the recorded Python results.
#[test]
fn package_path_rules_match_python() {
    let fixture = fixture();
    let mut failures = Vec::new();
    for (label, check) in [
        (
            "archive_path",
            source_package::archive_path as fn(&PyValue) -> _,
        ),
        (
            "source_path",
            source_package::source_path as fn(&PyValue) -> _,
        ),
    ] {
        for vector in section(&fixture, label) {
            let parts = vector.as_list().expect("vector");
            let input = text(&parts[0]);
            let actual = check(&PyValue::from(input.as_str()));
            package_matches(label, &input, actual, &parts[1], &mut failures);
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// `_validate_metadata` against the recorded Python results.
#[test]
fn package_metadata_rules_match_python() {
    let fixture = fixture();
    let mut failures = Vec::new();
    for vector in section(&fixture, "metadata") {
        let parts = vector.as_list().expect("vector");
        let args: Vec<&str> = parts[0]
            .as_list()
            .expect("metadata args")
            .iter()
            .map(|v| v.as_str().expect("metadata str"))
            .collect();
        let actual = source_package::validate_metadata(args[0], args[1], args[2])
            .map(|(a, b, c)| format!("{a}|{b}|{c}"));
        let expected_ok = parts[1].get("ok").and_then(PyValue::as_list).map(|values| {
            values
                .iter()
                .map(|v| v.as_str().unwrap_or_default())
                .collect::<Vec<_>>()
                .join("|")
        });
        match (expected_ok, &actual, expected(&parts[1])) {
            (Some(ok), Ok(actual), _) if ok == *actual => {}
            (None, Err(SourcePackageError::Invalid(actual)), Expected::Error { message, .. })
                if message == actual => {}
            (_, actual, outcome) => failures.push(format!(
                "metadata {args:?}: python {} / rust {actual:?}",
                describe(&outcome)
            )),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// `_parse_inputs` against the recorded Python results.
#[test]
fn package_inputs_parsing_matches_python() {
    let fixture = fixture();
    let mut failures = Vec::new();
    for vector in section(&fixture, "parse_inputs") {
        let parts = vector.as_list().expect("vector");
        let raw = segments(&parts[0]);
        let actual = source_package::parse_inputs(&raw);
        let label = String::from_utf8_lossy(&raw[..raw.len().min(60)]).into_owned();
        match (expected(&parts[1]), actual) {
            (Expected::Ok(records), Ok(actual)) => {
                let expected_records: Vec<(String, String, i128)> = records
                    .as_list()
                    .expect("records")
                    .iter()
                    .map(|record| {
                        let PyValue::Int(mode) =
                            record.get("mode").cloned().unwrap_or(PyValue::Null)
                        else {
                            panic!("mode")
                        };
                        (
                            record
                                .get("source")
                                .and_then(PyValue::as_str)
                                .unwrap_or_default()
                                .to_owned(),
                            record
                                .get("archive_path")
                                .and_then(PyValue::as_str)
                                .unwrap_or_default()
                                .to_owned(),
                            mode,
                        )
                    })
                    .collect();
                let actual_records: Vec<(String, String, i128)> = actual
                    .into_iter()
                    .map(|r| (r.source, r.archive_path, i128::from(r.mode)))
                    .collect();
                if expected_records != actual_records {
                    failures.push(format!(
                        "{label}: {expected_records:?} != {actual_records:?}"
                    ));
                }
            }
            (
                Expected::Error {
                    class: "PackageError",
                    message,
                },
                Err(SourcePackageError::Invalid(actual)),
            ) => {
                let same = message == actual
                    || (message.starts_with("invalid input JSON: ")
                        && actual.starts_with("invalid input JSON: ")
                        && !message.ends_with("duplicate JSON key"));
                if !same {
                    failures.push(format!("{label}: python {message} / rust {actual}"));
                }
            }
            (outcome, actual) => failures.push(format!(
                "{label}: python {} / rust {actual:?}",
                describe(&outcome)
            )),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

fn canary_matches(
    label: &str,
    expected_outcome: Expected<'_>,
    actual: Result<PyValue, CanaryError>,
    failures: &mut Vec<String>,
) {
    match (&expected_outcome, &actual) {
        (Expected::Ok(value), Ok(actual)) if py::py_eq(value, actual) => {}
        (
            Expected::Error {
                class: "SmokeError",
                message,
            },
            Err(CanaryError::Smoke(actual)),
        ) if message == actual => {}
        (
            Expected::Error {
                class: "ValueError",
                message,
            },
            Err(CanaryError::Other(actual)),
        ) if message == actual => {}
        _ => failures.push(format!(
            "{label}: python {} / rust {actual:?}",
            describe(&expected_outcome)
        )),
    }
}

/// `validate_listener_line` against the recorded Python results.
#[test]
fn canary_listener_validation_matches_python() {
    let fixture = fixture();
    let mut failures = Vec::new();
    for vector in section(&fixture, "listener") {
        let parts = vector.as_list().expect("vector");
        let line = segments(&parts[0]);
        let actual = canary::validate_listener_line(&line)
            .map(|(host, port)| PyValue::List(vec![PyValue::from(host), PyValue::from(port)]));
        let label = String::from_utf8_lossy(&line[..line.len().min(90)]).into_owned();
        canary_matches(&label, expected(&parts[1]), actual, &mut failures);
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// `parse_json_body` against the recorded Python results.
#[test]
fn canary_json_body_bounds_match_python() {
    let fixture = fixture();
    let mut failures = Vec::new();
    for vector in section(&fixture, "json_body") {
        let parts = vector.as_list().expect("vector");
        let body = segments(&parts[0]);
        let label = String::from_utf8_lossy(&body[..body.len().min(60)]).into_owned();
        let actual = canary::parse_json_body(&body);
        match (expected(&parts[1]), actual) {
            (Expected::Ok(_), Ok(_)) => {}
            (
                Expected::Error {
                    class: "SmokeError",
                    message,
                },
                Err(CanaryError::Smoke(actual)),
            ) if message == actual => {}
            (outcome, actual) => failures.push(format!(
                "{label}: python {} / rust {actual:?}",
                describe(&outcome)
            )),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// `redact` against the recorded Python results.
#[test]
fn canary_redaction_matches_python() {
    let fixture = fixture();
    for vector in section(&fixture, "redact") {
        let parts = vector.as_list().expect("vector");
        let data = segments(&parts[0]);
        let secret = parts[1].as_str().expect("secret");
        assert_eq!(canary::redact(&data, secret), text(&parts[2]), "{secret:?}");
    }
}

/// `ipaddress.ip_address` (compressed form, loopback) against Python.
#[test]
fn ip_address_rules_match_python() {
    let fixture = fixture();
    let mut failures = Vec::new();
    for vector in section(&fixture, "ip_address") {
        let parts = vector.as_list().expect("vector");
        let input = parts[0].as_str().expect("ip input");
        let actual = pynet::ip_address(input).map(|ip| {
            PyValue::List(vec![
                PyValue::from(ip.compressed()),
                PyValue::from(ip.is_loopback()),
            ])
        });
        match (expected(&parts[1]), &actual) {
            (Expected::Ok(value), Ok(actual)) if py::py_eq(value, actual) => {}
            (Expected::Error { message, .. }, Err(actual)) if message == actual => {}
            (outcome, actual) => failures.push(format!(
                "{input}: python {} / rust {actual:?}",
                describe(&outcome)
            )),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// `json.dumps` layouts against Python.
#[test]
fn json_dumps_layouts_match_python() {
    let fixture = fixture();
    for vector in section(&fixture, "dumps") {
        let parts = vector.as_list().expect("vector");
        // The input is Python's own json.dumps text, preserving key order.
        let value =
            py::loads_str(parts[0].as_str().expect("dumps input"), false).expect("dumps value");
        let value = &value;
        let expectations = &parts[1];
        let raw_sorted = Dump {
            ensure_ascii: false,
            ..Dump::COMPACT_SORTED
        };
        for (key, options) in [
            ("default", Dump::DEFAULT),
            ("compact_sorted", Dump::COMPACT_SORTED),
            ("indent2", Dump::INDENT2),
            ("indent2_sorted", Dump::INDENT2_SORTED),
            ("raw_sorted", raw_sorted),
        ] {
            assert_eq!(
                py::dumps(value, options),
                expectations
                    .get(key)
                    .and_then(PyValue::as_str)
                    .expect("expectation"),
                "{key}"
            );
        }
    }
}

/// `release_evidence.timestamp` (CPython `datetime.fromisoformat`) against Python.
#[test]
fn evidence_timestamps_match_python() {
    let fixture = fixture();
    let mut failures = Vec::new();
    for vector in section(&fixture, "timestamp") {
        let parts = vector.as_list().expect("vector");
        let input = &parts[0];
        let actual = release_evidence::timestamp(Some(input));
        let label = py::dumps(input, Dump::DEFAULT);
        match (expected(&parts[1]), &actual) {
            (Expected::Ok(PyValue::Int(micros)), Ok(actual)) if i128::from(*actual) == *micros => {}
            (
                Expected::Error {
                    class: "ValueError",
                    message,
                },
                Err(EvidenceError::Value(actual)),
            ) if message == actual => {}
            (
                Expected::Error {
                    class: "AttributeError",
                    message,
                },
                Err(EvidenceError::Uncaught(actual)),
            ) if actual == &format!("AttributeError: {message}") => {}
            (outcome, actual) => failures.push(format!(
                "{label}: python {} / rust {actual:?}",
                describe(&outcome)
            )),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

fn replay(payload: Vec<u8>) -> u32 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind replay server");
    let port = listener.local_addr().expect("replay address").port();
    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
            let mut request = Vec::new();
            let mut chunk = [0u8; 4096];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                match stream.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(count) => request.extend_from_slice(&chunk[..count]),
                }
            }
            let _ = stream.write_all(&payload);
            let _ = stream.shutdown(std::net::Shutdown::Write);
            thread::sleep(Duration::from_millis(50));
        }
    });
    u32::from(port)
}

/// `bounded_http` response handling against Python `http.client` replays.
#[test]
fn canary_http_framing_matches_python() {
    let fixture = fixture();
    let mut failures = Vec::new();
    for vector in section(&fixture, "http") {
        let parts = vector.as_list().expect("vector");
        let payload = segments(&parts[0]);
        let method = parts[1].as_str().expect("method");
        let label = format!(
            "{method} {:?}",
            String::from_utf8_lossy(&payload[..payload.len().min(70)])
        );
        let port = replay(payload);
        let actual = canary::bounded_http(
            "127.0.0.1",
            port,
            method,
            "/x",
            &[],
            None,
            monotonic() + 3.0,
        )
        .map(|(status, headers, body)| {
            let headers: BTreeMap<String, String> = headers;
            let mut header_dict = py::PyDict::new();
            for (name, value) in headers {
                header_dict.insert(name, value);
            }
            admiralty_qualify::py_dict! {
                "status" => u32::from(status),
                "headers" => header_dict,
                "body" => admiralty_qualify::py_dict! {
                    "len" => body.len(),
                    "sha256" => format!("{:x}", Sha256::digest(&body)),
                },
            }
        });
        canary_matches(&label, expected(&parts[2]), actual, &mut failures);
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
