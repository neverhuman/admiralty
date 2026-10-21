//! Ported from tests/test_sc_audit_jopedime_mr.py (major-release tracker adapter).

use serde_json::json;

use admiralty_audit::ctx::Audit;
use admiralty_audit::host::CmdOut;
use admiralty_audit::host::fake::FakeHost;
use admiralty_audit::py::J;

use crate::common::{at, hermetic_config, sha};

const MAIN: &str = "a2759e9d375d0697ed40a32c23ac297b5de42347";

/// Ported from tests/test_sc_audit_jopedime_mr.py::JopeDimeMajorReleaseAuditTest::test_valid_receipt_and_dependency_enforcement
#[test]
fn test_valid_receipt_and_dependency_enforcement() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let t = tmp.path().to_string_lossy().to_string();
    let vdir = format!("{t}/verify");
    std::fs::create_dir_all(&vdir).expect("mkdir");
    let log_file = format!("{t}/mr01.log");
    std::fs::write(&log_file, b"test result: ok. 1 passed; 0 failed\n").expect("write");
    let log_sha = sha(&log_file);
    let tracker = json!({
        "schema": "jope.major_release_tracker.v1",
        "verify_dir": vdir,
        "tasks": [
            {"id": "MR01", "owner_screen": "2830191.jopedime_agy", "deps": [], "gate": "G00",
             "exact_acceptance_test": "cargo test -p jope-records --test i53_i62_executable_receipts -- --exact i53_and_i62_contracts_execute",
             "acceptance_cases": ["i53_executable_contract_test"],
             "receipt_format": {"schema": "jope.major_release_receipt.v1"}},
            {"id": "MR02", "owner_screen": "2724936.captain_jopedime", "deps": ["MR01"], "gate": "G00",
             "exact_acceptance_test": "cargo test -p jope-engine --test mr02 -- --exact mr02_case",
             "acceptance_cases": ["mr02_case"],
             "receipt_format": {"schema": "jope.major_release_receipt.v1"}}
        ]
    });
    std::fs::write(
        format!("{t}/tasks.json"),
        serde_json::to_string(&tracker).expect("json"),
    )
    .expect("write");
    let case = json!({
        "acceptance_case": "i53_executable_contract_test",
        "command": "cargo test -p jope-records --test i53_i62_executable_receipts -- --exact i53_and_i62_contracts_execute",
        "test_or_code": "i53_and_i62_contracts_execute",
        "exit_code": 0, "passed": true, "log_path": log_file, "log_sha256": log_sha
    });
    let r1 = json!({"id": "MR01", "status": "PASS", "sha": MAIN, "checkout_sha": MAIN,
        "verified_at_utc": "2026-10-18T16:30:00Z", "case_to_test_map": [case.clone()]});
    std::fs::write(
        format!("{vdir}/MR01.json"),
        serde_json::to_string(&r1).expect("json"),
    )
    .expect("write");

    // JE_MR -> tmp; commit_time -> 2026-10-18T15:00Z (the Python test patched both)
    let mut cfg = hermetic_config(&format!("{t}/home"));
    cfg.je_mr = t.clone();
    let commit_epoch = at("2026-10-18T15:00:00+00:00").timestamp();
    let host = FakeHost::new(at("2026-10-18T17:00:00+00:00")).with_runner(move |argv, _| {
        if argv.contains(&"log") && argv.contains(&"--format=%ct") {
            CmdOut::ok(format!("{commit_epoch}\n"))
        } else {
            panic!("unexpected command {argv:?}")
        }
    });
    let mut audit = Audit::new(&cfg, &host);
    let res = audit
        .audit_jopedime_major_release("/nonexistent-repo", Some(MAIN))
        .expect("audit");
    let o = res.as_obj().expect("obj");
    assert_eq!(o.get("total"), Some(&J::Int(2)));
    assert_eq!(o.get("done_recomputed"), Some(&J::Int(1)));
    assert_eq!(o.get("suspect"), Some(&J::Obj(Default::default())));

    // Now remove MR01 and add MR02 (whose dep MR01 is not passed)
    std::fs::remove_file(format!("{vdir}/MR01.json")).expect("rm");
    let mut case2 = case;
    case2["acceptance_case"] = json!("mr02_case");
    case2["command"] = json!("cargo test -p jope-engine --test mr02 -- --exact mr02_case");
    let mut r2 = r1;
    r2["id"] = json!("MR02");
    r2["case_to_test_map"] = json!([case2]);
    std::fs::write(
        format!("{vdir}/MR02.json"),
        serde_json::to_string(&r2).expect("json"),
    )
    .expect("write");
    audit.findings.clear();
    let res2 = audit
        .audit_jopedime_major_release("/nonexistent-repo", Some(MAIN))
        .expect("audit");
    let o2 = res2.as_obj().expect("obj");
    assert_eq!(o2.get("done_recomputed"), Some(&J::Int(0)));
    let deps = o2
        .get("suspect")
        .and_then(|s| s.as_obj())
        .and_then(|s| s.get("deps_not_passed"))
        .cloned()
        .unwrap_or(J::Arr(vec![]));
    assert!(deps.as_arr().is_some_and(|a| a.contains(&J::from("MR02"))));
}
