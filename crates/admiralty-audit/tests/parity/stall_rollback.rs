//! Ported from tests/test_sc_audit_stall_rollback.py.

use admiralty_audit::AuditConfig;
use admiralty_audit::github::main_ci_severity;
use admiralty_audit::progress::{last_sustained_increase, stall_escalation};
use admiralty_audit::py::J;

use crate::common::at;

const NOW: &str = "2026-10-18T19:18:00+00:00";

fn h(pairs: &[(&str, i64)]) -> Vec<J> {
    pairs
        .iter()
        .map(|(u, d)| admiralty_audit::jobj! {"utc" => *u, "done" => *d})
        .collect()
}

fn lsi(pairs: &[(&str, i64)], current: i64) -> J {
    last_sustained_increase(&h(pairs), &J::Int(current)).expect("lsi")
}

/// Ported from tests/test_sc_audit_stall_rollback.py::Rollback::test_dope_overclaim_blip_does_not_reset_clock
#[test]
fn test_dope_overclaim_blip_does_not_reset_clock() {
    let pairs = [
        ("2026-10-18T11:40:00+00:00", 58),
        ("2026-10-18T11:49:03+00:00", 59),
        ("2026-10-18T18:51:54+00:00", 60),
        ("2026-10-18T19:09:49+00:00", 59),
    ];
    assert_eq!(lsi(&pairs, 59), J::from("2026-10-18T11:49:03+00:00"));
}

/// Ported from tests/test_sc_audit_stall_rollback.py::Rollback::test_blip_with_current_rollback
#[test]
fn test_blip_with_current_rollback() {
    let pairs = [
        ("2026-10-18T11:49:03+00:00", 59),
        ("2026-10-18T12:00:00+00:00", 59),
        ("2026-10-18T18:51:54+00:00", 60),
    ];
    assert_eq!(lsi(&pairs, 59), J::Null);
}

/// Ported from tests/test_sc_audit_stall_rollback.py::Rollback::test_real_increase_counts
#[test]
fn test_real_increase_counts() {
    let pairs = [
        ("2026-10-18T11:49:03+00:00", 59),
        ("2026-10-18T18:51:54+00:00", 60),
        ("2026-10-18T19:09:49+00:00", 60),
    ];
    assert_eq!(lsi(&pairs, 60), J::from("2026-10-18T18:51:54+00:00"));
}

/// Ported from tests/test_sc_audit_stall_rollback.py::Rollback::test_overclaim_then_real
#[test]
fn test_overclaim_then_real() {
    let pairs = [
        ("2026-10-18T11:00:00+00:00", 58),
        ("2026-10-18T11:49:03+00:00", 59),
        ("2026-10-18T18:00:00+00:00", 60),
        ("2026-10-18T18:30:00+00:00", 59),
        ("2026-10-18T19:00:00+00:00", 60),
    ];
    assert_eq!(lsi(&pairs, 60), J::from("2026-10-18T19:00:00+00:00"));
}

/// Ported from tests/test_sc_audit_stall_rollback.py::Rollback::test_stall_fires_on_dope_case
#[test]
fn test_stall_fires_on_dope_case() {
    let pairs = [
        ("2026-10-18T11:40:00+00:00", 58),
        ("2026-10-18T11:49:03+00:00", 59),
        ("2026-10-18T18:51:54+00:00", 60),
        ("2026-10-18T19:09:49+00:00", 59),
    ];
    let since = lsi(&pairs, 59);
    let got = stall_escalation(
        &AuditConfig::with_home("/home/ubuntu"),
        "dope",
        &J::Int(59),
        &J::Int(65),
        &since,
        &at(NOW),
        &[],
    )
    .expect("escalation");
    let codes: Vec<&str> = got.iter().map(|(_, c, _)| *c).collect();
    assert!(codes.contains(&"STALL_60M"));
    assert!(codes.contains(&"STALL_REPEATED"));
}

fn run(sha: &str, c: &str, t: &str) -> J {
    admiralty_audit::jobj! {"headSha" => sha, "conclusion" => c, "status" => "completed", "workflowName" => "Admiralty checks", "createdAt" => t}
}

fn severity(
    project: &str,
    head: &str,
    bad: &[&str],
    runs: &[J],
) -> (&'static str, &'static str, String) {
    let bad = J::Arr(bad.iter().map(|b| J::from(*b)).collect());
    main_ci_severity(
        &AuditConfig::with_home("/home/ubuntu"),
        project,
        &J::from(head),
        &bad,
        runs,
        &at(NOW),
    )
    .expect("severity")
}

/// Ported from tests/test_sc_audit_stall_rollback.py::MainRed::test_p1_persistent_red
#[test]
fn test_p1_persistent_red() {
    let runs = vec![
        run("7bb", "failure", "2026-10-18T19:05:36Z"),
        run("2f7", "failure", "2026-10-18T18:50:32Z"),
        run("843", "failure", "2026-10-18T18:41:05Z"),
        run("a68", "failure", "2026-10-18T17:43:50Z"),
        run("ok1", "success", "2026-10-18T17:30:05Z"),
    ];
    let (sev, code, text) = severity("admiralty", "7bb2784dc", &["Admiralty checks"], &runs);
    assert_eq!((sev, code), ("red", "MAIN_CI_RED_PERSISTENT"));
    assert!(text.contains("4 consecutive"));
}

/// Ported from tests/test_sc_audit_stall_rollback.py::MainRed::test_p1_fresh_single_red_stays_amber
#[test]
fn test_p1_fresh_single_red_stays_amber() {
    let runs = vec![
        run("new", "failure", "2026-10-18T19:10:00Z"),
        run("ok1", "success", "2026-10-18T19:00:00Z"),
    ];
    assert_eq!(severity("admiralty", "new", &["x"], &runs).0, "amber");
}

/// Ported from tests/test_sc_audit_stall_rollback.py::MainRed::test_p2_stays_amber
#[test]
fn test_p2_stays_amber() {
    let runs = vec![
        run("a", "failure", "2026-10-18T15:00:00Z"),
        run("b", "failure", "2026-10-18T14:00:00Z"),
        run("c", "failure", "2026-10-18T13:00:00Z"),
    ];
    assert_eq!(severity("dope", "a", &["CI"], &runs).1, "MAIN_CI_RED");
}
