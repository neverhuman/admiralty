//! Ported from tests/test_sc_audit_integrity.py (offline regression checks for
//! audit counts and actionable lane state).

use std::collections::BTreeSet;

use serde_json::{Value, json};

use admiralty_audit::ctx::Audit;
use admiralty_audit::ledgers::admiralty::ADM_FINISHED;
use admiralty_audit::ledgers::dope::{check_dope_receipt, dope_receipt_refs, verify_dope_tasks};
use admiralty_audit::py::{J, Obj};
use admiralty_audit::screen::Screens;

use crate::common::{at, hermetic_config, no_commands_host, sha, sj};

const NOW: &str = "2026-10-18T14:00:00+00:00";
const OLD: &str = "2026-10-18T12:00:00Z";

fn head() -> String {
    "a".repeat(40)
}

fn verification(changes: Value) -> Value {
    let mut v = json!({
        "receipt": "receipts/P01/check.json",
        "revision": head(),
        "verifier": "independent-reviewer",
        "observed_utc": "2026-10-18T13:59:00Z",
    });
    if let (Some(o), Some(c)) = (v.as_object_mut(), changes.as_object()) {
        for (k, x) in c {
            o.insert(k.clone(), x.clone());
        }
    }
    v
}

/// `AdmiraltyIntegrity.audit(packets, lanes, screens, prev)`.
fn audit(packets: Vec<Value>, lanes: Vec<Value>, screens: &[(&str, i64)]) -> (J, BTreeSet<String>) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cfg = hermetic_config(&tmp.path().to_string_lossy());
    std::fs::create_dir_all(&cfg.adm).expect("mkdir");
    let now = at(NOW);
    std::fs::write(
        format!("{}/rollout.json", cfg.adm),
        serde_json::to_string(
            &json!({"updated_utc": "2026-10-18T14:00:00+00:00", "packets": packets}),
        )
        .expect("json"),
    )
    .expect("write");
    std::fs::write(
        format!("{}/live-lanes.json", cfg.adm),
        serde_json::to_string(&json!({"lanes": lanes})).expect("json"),
    )
    .expect("write");
    let host = no_commands_host(now).with_hardcopy(|_, _| "working on bounded task".to_string());
    let mut a = Audit::new(&cfg, &host);
    let scr: Screens = screens
        .iter()
        .map(|(n, p)| ((*n).to_string(), *p))
        .collect();
    let result = a
        .audit_admiralty(&scr, &J::Obj(Obj::new()), &mut Obj::new(), &now)
        .expect("audit");
    let codes = a.codes().into_iter().collect();
    (result, codes)
}

fn field<'a>(j: &'a J, k: &str) -> &'a J {
    j.as_obj().and_then(|o| o.get(k)).expect("field")
}

/// Ported from tests/test_sc_audit_integrity.py::AdmiraltyIntegrity::test_truthy_nonrecords_never_count_integrated_packets
#[test]
fn test_truthy_nonrecords_never_count_integrated_packets() {
    for proof in [
        json!(true),
        json!(1),
        json!("verified"),
        json!("false"),
        json!("unknown"),
        json!("unverified"),
        json!(["passed"]),
        json!({"status": "verified"}),
        json!({"receipt": "prose"}),
        json!({}),
    ] {
        let (result, codes) = audit(
            vec![json!({"id": "P01", "status": "integrated", "verification": proof})],
            vec![],
            &[],
        );
        assert_eq!(
            field(&result, "done_recomputed"),
            &J::Int(0),
            "proof={proof}"
        );
        assert!(codes.contains("DONE_NO_EVIDENCE"), "proof={proof}");
    }
}

/// Ported from tests/test_sc_audit_integrity.py::AdmiraltyIntegrity::test_complete_record_preserves_valid_positive_and_verified_is_not_done
#[test]
fn test_complete_record_preserves_valid_positive_and_verified_is_not_done() {
    for extra in [
        json!({}),
        json!({"status": "passed"}),
        json!({"result": "success"}),
        json!({"verified": true, "passed": true}),
    ] {
        let packets: Vec<Value> = ["integrated", "verified", "in_progress"]
            .iter()
            .map(|status| json!({"id": status, "status": status, "verification": verification(extra.clone())}))
            .collect();
        let (result, codes) = audit(packets, vec![], &[]);
        assert_eq!(
            field(&result, "done_recomputed"),
            &J::Int(1),
            "extra={extra}"
        );
        assert_eq!(
            field(&result, "verified_or_integrated_claimed"),
            &J::Int(2),
            "extra={extra}"
        );
        assert!(!codes.contains("DONE_NO_EVIDENCE"), "extra={extra}");
        assert!(
            field(&result, "note")
                .as_str()
                .is_some_and(|n| n.contains("not independently verified"))
        );
    }
}

/// Ported from tests/test_sc_audit_integrity.py::AdmiraltyIntegrity::test_missing_malformed_and_placeholder_provenance_rejected
#[test]
fn test_missing_malformed_and_placeholder_provenance_rejected() {
    let mut proofs = Vec::new();
    for key in ["receipt", "revision", "verifier", "observed_utc"] {
        let mut proof = verification(json!({}));
        if let Some(o) = proof.as_object_mut() {
            o.remove(key);
        }
        proofs.push(proof);
        for invalid in [
            json!(false),
            json!(""),
            json!("unknown"),
            json!("unverified"),
            json!("false"),
            json!("x\nforged"),
            json!("x".repeat(1001)),
        ] {
            proofs.push(verification(json!({key: invalid})));
        }
    }
    proofs.push(verification(json!({"revision": "abc123"})));
    for proof in proofs {
        let (result, codes) = audit(
            vec![json!({"id": "P01", "status": "integrated", "verification": proof})],
            vec![],
            &[],
        );
        assert_eq!(
            field(&result, "done_recomputed"),
            &J::Int(0),
            "proof={proof}"
        );
        assert!(codes.contains("DONE_NO_EVIDENCE"), "proof={proof}");
    }
}

/// Ported from tests/test_sc_audit_integrity.py::AdmiraltyIntegrity::test_explicit_negative_or_unknown_outcome_never_counts
#[test]
fn test_explicit_negative_or_unknown_outcome_never_counts() {
    for key in ["status", "result", "outcome"] {
        for outcome in [
            json!(false),
            json!(true),
            json!("false"),
            json!("unverified"),
            json!("not_verified"),
            json!("unknown"),
            json!("pending"),
            json!("failed"),
        ] {
            let packet = json!({"id": "P01", "status": "integrated", "verification": verification(json!({key: outcome}))});
            let (result, _) = audit(vec![packet], vec![], &[]);
            assert_eq!(
                field(&result, "done_recomputed"),
                &J::Int(0),
                "key={key} outcome={outcome}"
            );
        }
    }
    for key in ["passed", "verified", "success"] {
        for outcome in [json!(false), json!(1), json!("true"), json!("unknown")] {
            let packet = json!({"id": "P01", "status": "integrated", "verification": verification(json!({key: outcome}))});
            let (result, _) = audit(vec![packet], vec![], &[]);
            assert_eq!(
                field(&result, "done_recomputed"),
                &J::Int(0),
                "key={key} outcome={outcome}"
            );
        }
    }
}

/// Ported from tests/test_sc_audit_integrity.py::AdmiraltyIntegrity::test_present_candidate_integration_and_claimant_must_agree
#[test]
fn test_present_candidate_integration_and_claimant_must_agree() {
    for fields in [
        json!({"candidate_revision": "b".repeat(40)}),
        json!({"integration": {"revision": "b".repeat(40)}}),
        json!({"claim": {"owner": "independent-reviewer"}}),
    ] {
        let mut packet =
            json!({"id": "P01", "status": "integrated", "verification": verification(json!({}))});
        if let (Some(o), Some(f)) = (packet.as_object_mut(), fields.as_object()) {
            for (k, v) in f {
                o.insert(k.clone(), v.clone());
            }
        }
        let (result, _) = audit(vec![packet], vec![], &[]);
        assert_eq!(
            field(&result, "done_recomputed"),
            &J::Int(0),
            "fields={fields}"
        );
    }
    let packet = json!({"id": "P01", "status": "integrated", "verification": verification(json!({})),
        "candidate_revision": head(), "integration": {"revision": head()}, "claim": {"owner": "task-worker"}});
    let (result, _) = audit(vec![packet], vec![], &[]);
    assert_eq!(field(&result, "done_recomputed"), &J::Int(1));
}

/// Ported from tests/test_sc_audit_integrity.py::AdmiraltyIntegrity::test_unknown_naive_or_future_verification_time_rejected
#[test]
fn test_unknown_naive_or_future_verification_time_rejected() {
    for observed in ["not-a-time", "2026-10-18T13:00:00", "2026-10-18T14:01:00Z"] {
        let packet = json!({"id": "P01", "status": "integrated", "verification": verification(json!({"observed_utc": observed}))});
        let (result, _) = audit(vec![packet], vec![], &[]);
        assert_eq!(
            field(&result, "done_recomputed"),
            &J::Int(0),
            "observed={observed}"
        );
    }
}

/// Ported from tests/test_sc_audit_integrity.py::AdmiraltyIntegrity::test_substring_terminal_words_do_not_hide_dead_lanes
#[test]
fn test_substring_terminal_words_do_not_hide_dead_lanes() {
    for status in [
        json!("candidate_committed_unverified"),
        json!("not_verified"),
        json!("unfinished"),
        json!("not_closed"),
        json!("verified_pending"),
        json!("integrated_pending"),
        json!("released_pending"),
        json!("unknown"),
        json!(false),
    ] {
        let lane = json!({"screen": "123.worker", "status": status, "packet_id": "P01", "updated_utc": OLD});
        let (result, codes) = audit(vec![], vec![lane], &[]);
        let alive = field(&result, "workers")
            .as_obj()
            .and_then(|w| w.get("worker"))
            .and_then(|w| w.as_obj())
            .and_then(|w| w.get("alive"))
            .cloned();
        assert_eq!(alive, Some(J::Bool(false)), "status={status}");
        assert!(codes.contains("LANE_DEAD"), "status={status}");
        if status == json!("candidate_committed_unverified") {
            assert!(codes.contains("UNVERIFIED_CANDIDATES_AGING"));
        }
    }
}

/// Ported from tests/test_sc_audit_integrity.py::AdmiraltyIntegrity::test_unverified_and_unknown_live_lanes_remain_stale
#[test]
fn test_unverified_and_unknown_live_lanes_remain_stale() {
    for status in ["candidate_committed_unverified", "not_verified", "unknown"] {
        let lane = json!({"screen": "123.worker", "status": status, "packet_id": "P01", "updated_utc": OLD});
        let (_, codes) = audit(vec![], vec![lane], &[("worker", 123)]);
        assert!(codes.contains("LANE_STALE"), "status={status}");
        assert!(!codes.contains("LANE_DEAD"), "status={status}");
    }
}

/// Ported from tests/test_sc_audit_integrity.py::AdmiraltyIntegrity::test_only_exact_terminal_states_preserve_lane_closeout
#[test]
fn test_only_exact_terminal_states_preserve_lane_closeout() {
    for status in ADM_FINISHED {
        for alive in [false, true] {
            let lane = json!({"screen": "worker", "status": format!(" {} ", status.to_uppercase()), "updated_utc": OLD});
            let screens: &[(&str, i64)] = if alive { &[("worker", 123)] } else { &[] };
            let (_, codes) = audit(vec![], vec![lane], screens);
            assert!(
                !codes.contains("LANE_DEAD"),
                "status={status} alive={alive}"
            );
            assert!(
                !codes.contains("LANE_STALE"),
                "status={status} alive={alive}"
            );
        }
    }
}

struct DopeRoot {
    _tmp: tempfile::TempDir,
    root: String,
}

impl DopeRoot {
    fn new() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_string_lossy().to_string();
        std::fs::create_dir_all(format!("{root}/verify")).expect("mkdir");
        Self { _tmp: tmp, root }
    }

    fn task(&self, task_id: &str, sub_status: &str) -> Value {
        let log = format!("{}/verify/{task_id}.log", self.root);
        std::fs::write(
            &log,
            format!(
                "head_sha: {}\nexit_code: 0\n{task_id}\nRan 1 test in 0.001s\nOK\n",
                head()
            ),
        )
        .expect("write");
        let receipt = format!("{}/verify/{task_id}.json", self.root);
        let body = json!({"task_id": task_id, "status": "passed", "head_sha": head(),
            "commands": [{"cmd": "python3 -m unittest task_checks", "exit_code": 0, "head_sha": head(), "log_path": log, "log_sha256": sha(&log)}]});
        std::fs::write(&receipt, serde_json::to_string(&body).expect("json")).expect("write");
        json!({"id": task_id, "status": "done", "sub_status": sub_status, "evidence": [format!("{receipt}#sha256={}", sha(&receipt))]})
    }
}

fn is_head(s: &str) -> bool {
    s == head()
}

/// Ported from tests/test_sc_audit_integrity.py::DopeIntegrity::test_blocked_partial_and_failed_cannot_count_despite_valid_receipts
#[test]
fn test_blocked_partial_and_failed_cannot_count_despite_valid_receipts() {
    let d = DopeRoot::new();
    let mut tasks = vec![d.task("GOOD", "completed")];
    for (index, status) in [
        "BLOCKED: needs owner input",
        "partial implementation",
        "failed final check",
    ]
    .iter()
    .enumerate()
    {
        tasks.push(d.task(&format!("BAD{index}"), status));
    }
    // The receipts really do pass the independent receipt checks.
    for task in &tasks {
        let t = sj(task.clone());
        let refs = dope_receipt_refs(&t).expect("refs");
        let (path, pin) = &refs[0];
        let id = t
            .as_obj()
            .and_then(|o| o.get("id"))
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            check_dope_receipt(&id, path, pin.as_deref(), &is_head, None)
                .expect("check")
                .0,
            Vec::<String>::new()
        );
    }
    std::fs::write(
        format!("{}/tasks.json", d.root),
        serde_json::to_string(&json!({"tasks": tasks})).expect("json"),
    )
    .expect("write");
    // max_claim finds nothing: the hermetic config has no captain log or hardcopy (Python patched max_claim to None).
    let cfg = hermetic_config(&format!("{}/home", d.root));
    let host = no_commands_host(at(NOW));
    let mut a = Audit::new(&cfg, &host);
    let result = a
        .audit_dope(Some(&d.root), Some(&is_head), None)
        .expect("audit");
    assert_eq!(field(&result, "ledger_done"), &J::Int(4));
    assert_eq!(field(&result, "done_recomputed"), &J::Int(1));
    assert_eq!(
        field(&result, "unverified_done"),
        &J::from(vec![
            "BAD0".to_string(),
            "BAD1".to_string(),
            "BAD2".to_string()
        ])
    );
    for task_id in ["BAD0", "BAD1", "BAD2"] {
        let probs = field(&result, "receipt_problems")
            .as_obj()
            .and_then(|o| o.get(task_id))
            .cloned()
            .expect("problems");
        assert!(
            probs
                .as_arr()
                .is_some_and(|a| a.contains(&J::from("contradictory_sub_status"))),
            "{task_id}"
        );
    }
    assert!(
        a.codes()
            .contains(&"DONE_BUT_SUBSTATUS_BLOCKED".to_string())
    );
}

/// Ported from tests/test_sc_audit_integrity.py::DopeIntegrity::test_contradiction_reason_retained_alongside_missing_receipt
#[test]
fn test_contradiction_reason_retained_alongside_missing_receipt() {
    let tasks = vec![sj(
        json!({"id": "BAD", "status": "done", "sub_status": "blocked", "evidence": []}),
    )];
    let (verified, problems, _) = verify_dope_tasks(&tasks, &|_| true, None).expect("verify");
    assert!(verified.is_empty());
    let got: BTreeSet<String> = problems
        .get("BAD")
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .collect();
    let want: BTreeSet<String> = ["no_receipt", "contradictory_sub_status"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(got, want);
}
