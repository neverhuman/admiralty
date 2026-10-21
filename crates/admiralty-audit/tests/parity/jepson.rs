//! Ported from tests/test_sc_audit_jepson.py.

use admiralty_audit::github::{jepson_pr_reasons, jepson_protection_reasons, jepson_text_hits};

use crate::common::j;

fn hits(text: &str) -> usize {
    jepson_text_hits(&[("s".to_string(), text.to_string())]).len()
}

/// Ported from tests/test_sc_audit_jepson.py::JepsonBottleneck::test_review_request_flagged
#[test]
fn test_review_request_flagged() {
    let got = jepson_pr_reasons(&j(r#"{"reviewRequests": [{"login": "jepsontaylor"}]}"#))
        .expect("reasons");
    assert_eq!(got, vec!["review requested from jepsontaylor".to_string()]);
}

/// Ported from tests/test_sc_audit_jepson.py::JepsonBottleneck::test_bot_review_not_flagged
#[test]
fn test_bot_review_not_flagged() {
    let p = j(
        r#"{"reviewRequests": [{"login": "neverhumanbot"}], "latestReviews": [{"author": {"login": "neverhumanbot"}, "state": "CHANGES_REQUESTED"}]}"#,
    );
    assert_eq!(
        jepson_pr_reasons(&p).expect("reasons"),
        Vec::<String>::new()
    );
}

/// Ported from tests/test_sc_audit_jepson.py::JepsonBottleneck::test_jepson_changes_requested
#[test]
fn test_jepson_changes_requested() {
    let p = j(
        r#"{"latestReviews": [{"author": {"login": "JepsonTaylor"}, "state": "CHANGES_REQUESTED"}]}"#,
    );
    assert_eq!(
        jepson_pr_reasons(&p).expect("reasons"),
        vec!["jepsontaylor requested changes".to_string()]
    );
}

/// Ported from tests/test_sc_audit_jepson.py::JepsonBottleneck::test_draft_ignored
#[test]
fn test_draft_ignored() {
    let p = j(r#"{"isDraft": true, "reviewRequests": [{"login": "jepsontaylor"}]}"#);
    assert_eq!(
        jepson_pr_reasons(&p).expect("reasons"),
        Vec::<String>::new()
    );
}

/// Ported from tests/test_sc_audit_jepson.py::JepsonBottleneck::test_text_waiting_on_jepson
#[test]
fn test_text_waiting_on_jepson() {
    assert_eq!(hits("PR #9 is waiting on Jepson to merge"), 1);
    assert_eq!(hits("W01 NEEDS JEPSON decision on schedule"), 1);
}

/// Ported from tests/test_sc_audit_jepson.py::JepsonBottleneck::test_text_reserved_excepted
#[test]
fn test_text_reserved_excepted() {
    assert_eq!(hits("Kumo license is new paid spend, waiting on Jepson"), 0);
    assert_eq!(hits("anything on xbabe0 needs Jepson"), 0);
}

/// Ported from tests/test_sc_audit_jepson.py::JepsonBottleneck::test_text_clean
#[test]
fn test_text_clean() {
    assert_eq!(hits("Jepson ruling applied; super_captain merged #12"), 0);
}

/// Ported from tests/test_sc_audit_jepson.py::JepsonBottleneck::test_protection_codeowner
#[test]
fn test_protection_codeowner() {
    let prot = j(r#"{"required_pull_request_reviews": {"require_code_owner_reviews": true}}"#);
    assert!(
        !jepson_protection_reasons(&prot, "* @jepsontaylor\n")
            .expect("reasons")
            .is_empty()
    );
    assert_eq!(
        jepson_protection_reasons(&prot, "* @neverhuman\n").expect("reasons"),
        Vec::<String>::new()
    );
    assert_eq!(
        jepson_protection_reasons(&j("{}"), "* @jepsontaylor\n").expect("reasons"),
        Vec::<String>::new()
    );
}

/// Ported from tests/test_sc_audit_jepson.py::JepsonBottleneck::test_protection_restrictions
#[test]
fn test_protection_restrictions() {
    let prot = j(r#"{"restrictions": {"users": [{"login": "jepsontaylor"}]}}"#);
    assert!(
        !jepson_protection_reasons(&prot, "")
            .expect("reasons")
            .is_empty()
    );
}
