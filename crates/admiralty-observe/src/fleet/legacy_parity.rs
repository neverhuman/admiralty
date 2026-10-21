//! Fleet-side parity for retired `dashboard/collector.py` capture behavior
//! and the synthetic `stale_capture` legacy case.
use super::*;
use chrono::{DateTime, Duration, Utc};

/// Synthetic legacy case `stale_capture`: text captured from an earlier
/// process is never reused for the current process, and an observation older
/// than the retention bound is never reused even for the same process. The
/// observer has no input authority at all, so `input_allowed` is always false.
#[test]
fn legacy_case_stale_capture() {
    let case = crate::legacy_cases::case("stale_capture");
    let input = &case["input"];
    let capture = &input["capture"];
    let at = |key: &Value| {
        DateTime::<Utc>::from_timestamp(key.as_i64().expect("fixture epoch"), 0)
            .expect("fixture instant")
    };
    let observed = at(&capture["observed_epoch"]);
    let current = at(&input["current_epoch"]);
    let entry = ModelObservation {
        pid: 42,
        start: capture["process_identity"]
            .as_str()
            .expect("captured identity")
            .to_owned(),
        family: "codex".into(),
        model: "gpt-6-luna".into(),
        effort: Some("medium".into()),
        observed_utc: observed.to_rfc3339(),
    };
    let process = |start: &Value| AgentProcess {
        pid: 42,
        family: "codex",
        start: start.as_str().expect("process identity").to_owned(),
    };
    assert!(!entry.matches(&process(&input["current_process_identity"]), current));
    let same = process(&capture["process_identity"]);
    assert!(entry.matches(&same, observed + Duration::seconds(1)));
    assert!(!entry.matches(
        &same,
        observed + Duration::seconds(MODEL_CACHE_TTL_SECONDS + 1)
    ));
    assert_eq!(case["expected"]["input_allowed"], false);
    assert_eq!(case["expected"]["capture_state"], "stale_or_failed");
}

/// Parity with tests/legacy/test_rollout_live_workers.py::test_tail_is_cleaned_bounded_and_never_copied_without_a_sanitizer
/// (fleet half): every published tail line, including a rollout captain's
/// copied screen record, is sanitized and bounded to 240 characters, with
/// credential and typed-input lines withheld.
#[test]
fn tail_is_cleaned_bounded_and_never_copied_without_a_sanitizer() {
    let mut raw = vec![format!("progress {}", "x".repeat(500)); 20].join("\n");
    raw.push_str("\ntoken=abcdefghijklmnopqrstuvwx\n> typed private input\n");
    let clean = safe_tail(&raw);
    assert!(clean.lines().all(|line| line.chars().count() <= 240));
    assert!(!clean.contains("abcdefghijklmnopqrstuvwx"));
    assert!(!clean.contains("typed private input"));
}
