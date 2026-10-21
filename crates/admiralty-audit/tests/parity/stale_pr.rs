//! Ported from tests/test_sc_audit_stale_pr.py.

use serde_json::{Value, json};

use admiralty_audit::github::{STALE_PR_MIN, stale_pr_problems};

use crate::common::{at, sj};

const NOW: &str = "2026-10-18T16:30:00+00:00";

fn pr(kw: Value) -> Value {
    let mut base = json!({"number": 1, "headRefName": "fix/x", "isDraft": false, "createdAt": "2026-10-18T16:20:00Z", "mergeable": "MERGEABLE", "statusCheckRollup": []});
    if let (Some(b), Some(k)) = (base.as_object_mut(), kw.as_object()) {
        for (key, v) in k {
            b.insert(key.clone(), v.clone());
        }
    }
    base
}

fn problems(p: Value) -> Vec<String> {
    stale_pr_problems(&sj(p), &at(NOW), STALE_PR_MIN).expect("problems")
}

/// Ported from tests/test_sc_audit_stale_pr.py::StalePr::test_fresh_clean_pr_ok
#[test]
fn test_fresh_clean_pr_ok() {
    assert_eq!(problems(pr(json!({}))), Vec::<String>::new());
}

/// Ported from tests/test_sc_audit_stale_pr.py::StalePr::test_old_pr_flagged
#[test]
fn test_old_pr_flagged() {
    assert_eq!(
        problems(pr(json!({"createdAt": "2026-10-18T15:30:00Z"}))),
        vec!["open 60 min".to_string()]
    );
}

/// Ported from tests/test_sc_audit_stale_pr.py::StalePr::test_draft_ignored
#[test]
fn test_draft_ignored() {
    assert_eq!(
        problems(pr(
            json!({"isDraft": true, "createdAt": "2026-10-01T00:00:00Z"})
        )),
        Vec::<String>::new()
    );
}

/// Ported from tests/test_sc_audit_stale_pr.py::StalePr::test_conflict_and_backup_branch
#[test]
fn test_conflict_and_backup_branch() {
    let got = problems(pr(
        json!({"mergeable": "CONFLICTING", "headRefName": "backup/pr208-pre-rebase"}),
    ));
    assert!(got.contains(&"conflicts with main".to_string()));
    assert!(got.iter().any(|g| g.contains("backup branch")));
}

/// Ported from tests/test_sc_audit_stale_pr.py::StalePr::test_failure_superseded_by_later_success_not_flagged
#[test]
fn test_failure_superseded_by_later_success_not_flagged() {
    let roll = json!([{"name": "cpu", "conclusion": "FAILURE", "completedAt": "2026-10-18T16:10:00Z"},
        {"name": "cpu", "conclusion": "SUCCESS", "completedAt": "2026-10-18T16:25:00Z"}]);
    assert_eq!(
        problems(pr(json!({"statusCheckRollup": roll}))),
        Vec::<String>::new()
    );
}

/// Ported from tests/test_sc_audit_stale_pr.py::StalePr::test_latest_failure_flagged
#[test]
fn test_latest_failure_flagged() {
    let roll = json!([{"name": "security", "conclusion": "SUCCESS", "completedAt": "2026-10-18T16:00:00Z"},
        {"name": "security", "conclusion": "FAILURE", "completedAt": "2026-10-18T16:22:00Z"}]);
    assert_eq!(
        problems(pr(json!({"statusCheckRollup": roll}))),
        vec!["failing security".to_string()]
    );
}

/// Ported from tests/test_sc_audit_stale_pr.py::StalePr::test_bad_timestamp_ignored
#[test]
fn test_bad_timestamp_ignored() {
    assert_eq!(
        problems(pr(json!({"createdAt": null}))),
        Vec::<String>::new()
    );
}
