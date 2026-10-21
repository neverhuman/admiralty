//! Ported from tests/test_sc_audit_stall.py.

use admiralty_audit::AuditConfig;
use admiralty_audit::progress::{Escalation, stall_escalation, stall_plan_changes};
use admiralty_audit::py::{Dt, J};
use chrono::Timelike;

use crate::common::at;

const NOW: &str = "2026-10-18T17:20:00+00:00";

fn esc(proj: &str, done: J, total: J, since: J, pc: &[Dt]) -> Vec<Escalation> {
    stall_escalation(
        &AuditConfig::with_home("/home/ubuntu"),
        proj,
        &done,
        &total,
        &since,
        &at(NOW),
        pc,
    )
    .expect("escalation")
}

fn codes(r: &[Escalation]) -> Vec<&'static str> {
    r.iter().map(|(_, c, _)| *c).collect()
}

/// Ported from tests/test_sc_audit_stall.py::Stall::test_under_60_quiet
#[test]
fn test_under_60_quiet() {
    assert!(
        esc(
            "jarambit",
            J::Int(6),
            J::Int(37),
            J::from("2026-10-18T16:30:00+00:00"),
            &[]
        )
        .is_empty()
    );
}

/// Ported from tests/test_sc_audit_stall.py::Stall::test_60_needs_inspection_and_plan
#[test]
fn test_60_needs_inspection_and_plan() {
    let got = esc(
        "jarambit",
        J::Int(6),
        J::Int(37),
        J::from("2026-10-18T16:15:00+00:00"),
        &[],
    );
    assert_eq!(codes(&got), vec!["STALL_60M", "STALL_NO_PLAN_CHANGE"]);
}

/// Ported from tests/test_sc_audit_stall.py::Stall::test_plan_change_after_stall_clears_only_that
#[test]
fn test_plan_change_after_stall_clears_only_that() {
    let pc = [at("2026-10-18T16:50:00+00:00")];
    let got = esc(
        "jarambit",
        J::Int(6),
        J::Int(37),
        J::from("2026-10-18T16:15:00+00:00"),
        &pc,
    );
    assert_eq!(codes(&got), vec!["STALL_60M"]);
}

/// Ported from tests/test_sc_audit_stall.py::Stall::test_plan_change_before_stall_does_not_count
#[test]
fn test_plan_change_before_stall_does_not_count() {
    let pc = [at("2026-10-18T14:00:00+00:00")];
    let got = esc(
        "jarambit",
        J::Int(6),
        J::Int(37),
        J::from("2026-10-18T16:15:00+00:00"),
        &pc,
    );
    assert!(codes(&got).contains(&"STALL_NO_PLAN_CHANGE"));
}

/// Ported from tests/test_sc_audit_stall.py::Stall::test_jarambit_today_is_repeated
#[test]
fn test_jarambit_today_is_repeated() {
    // 6/37 since 14:31Z, merges of #115/#116 must not clear it
    let pc = [at("2026-10-18T15:30:00+00:00")];
    let got = esc(
        "jarambit",
        J::Int(6),
        J::Int(37),
        J::from("2026-10-18T14:31:32+00:00"),
        &pc,
    );
    assert_eq!(codes(&got), vec!["STALL_60M", "STALL_REPEATED"]);
    assert!(
        got.last()
            .is_some_and(|(_, _, t)| t.contains("did not move the count"))
    );
}

/// Ported from tests/test_sc_audit_stall.py::Stall::test_complete_project_exempt
#[test]
fn test_complete_project_exempt() {
    assert!(
        esc(
            "admiralty",
            J::Int(32),
            J::Int(32),
            J::from("2026-10-18T10:00:00+00:00"),
            &[]
        )
        .is_empty()
    );
}

/// Ported from tests/test_sc_audit_stall.py::Stall::test_bad_inputs
#[test]
fn test_bad_inputs() {
    assert!(
        esc(
            "x",
            J::Null,
            J::Int(10),
            J::from("2026-10-18T10:00:00+00:00"),
            &[]
        )
        .is_empty()
    );
    assert!(esc("x", J::Int(1), J::Int(10), J::Null, &[]).is_empty());
    assert!(esc("x", J::Int(1), J::Int(10), J::from("garbage"), &[]).is_empty());
}

/// Ported from tests/test_sc_audit_stall.py::Stall::test_parse_plan_change_lines
#[test]
fn test_parse_plan_change_lines() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let f = tmp
        .path()
        .join("decisions.md")
        .to_string_lossy()
        .to_string();
    std::fs::write(
        &f,
        "## x\n- STALL_PLAN_CHANGE jarambit 2026-10-18T17:05:00Z: added 3 workers\n- STALL_PLAN_CHANGE `dope` `2026-10-18T17:06Z` re-scope\n",
    )
    .expect("write");
    let got = stall_plan_changes(&[f]);
    assert_eq!(
        got.get("jarambit").and_then(|v| v.first()).copied(),
        Some(at("2026-10-18T17:05:00+00:00"))
    );
    assert_eq!(
        got.get("dope").and_then(|v| v.first()).map(|t| t.minute()),
        Some(6)
    );
}
