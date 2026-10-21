//! Ported from tests/test_sc_audit_ci_queue.py.

use admiralty_audit::github::stuck_queued_checks;

use crate::common::{at, j};

const NOW: &str = "2026-10-18T13:50:00+00:00";

/// Ported from tests/test_sc_audit_ci_queue.py::QueuedStuck::test_queued_an_hour_is_stuck
#[test]
fn test_queued_an_hour_is_stuck() {
    let roll = j(
        r#"[{"name": "contracts", "status": "QUEUED", "conclusion": "", "startedAt": "2026-10-18T12:47:06Z"}]"#,
    );
    assert_eq!(
        stuck_queued_checks(&roll, &at(NOW), 30.0).expect("checks"),
        vec![("contracts".to_string(), 63)]
    );
}

/// Ported from tests/test_sc_audit_ci_queue.py::QueuedStuck::test_recent_queue_and_running_and_done_not_stuck
#[test]
fn test_recent_queue_and_running_and_done_not_stuck() {
    let roll = j(
        r#"[{"name": "a", "status": "QUEUED", "conclusion": "", "startedAt": "2026-10-18T13:40:00Z"},
        {"name": "b", "status": "IN_PROGRESS", "conclusion": "", "startedAt": "2026-10-18T12:00:00Z"},
        {"name": "c", "status": "COMPLETED", "conclusion": "SUCCESS", "startedAt": "2026-10-18T12:00:00Z"}]"#,
    );
    assert_eq!(
        stuck_queued_checks(&roll, &at(NOW), 30.0).expect("checks"),
        vec![]
    );
}

/// Ported from tests/test_sc_audit_ci_queue.py::QueuedStuck::test_bad_timestamp_ignored
#[test]
fn test_bad_timestamp_ignored() {
    let roll = j(r#"[{"name": "x", "status": "QUEUED", "startedAt": null}]"#);
    assert_eq!(
        stuck_queued_checks(&roll, &at(NOW), 30.0).expect("checks"),
        vec![]
    );
}
