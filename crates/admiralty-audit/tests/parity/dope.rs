//! Ported from tests/test_sc_audit_dope.py (DOPE per-task receipt checks).

use std::collections::BTreeSet;

use serde_json::{Value, json};

use admiralty_audit::ctx::Audit;
use admiralty_audit::host::fake::FakeHost;
use admiralty_audit::ledgers::dope::{DOPE_GENERIC_CMD_MIN, check_dope_receipt, dope_cmd_program};
use admiralty_audit::py::J;

use crate::common::{at, hermetic_config, no_commands_host, sha};

fn main_sha() -> String {
    "a".repeat(40)
}
fn other_sha() -> String {
    "b".repeat(40)
}
fn on_main(s: &str) -> bool {
    s == main_sha()
}

pub struct Packet {
    _tmp: tempfile::TempDir,
    pub d: String,
    pub tasks: Vec<Value>,
}

impl Packet {
    pub fn new() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let d = tmp.path().to_string_lossy().to_string();
        std::fs::create_dir_all(format!("{d}/verify/main_x/logs")).expect("mkdir");
        Self {
            _tmp: tmp,
            d,
            tasks: Vec::new(),
        }
    }

    pub fn log_with(&self, name: &str, head: &str, ec: i64, extra: &str) -> String {
        let p = format!("{}/verify/main_x/logs/{name}.log", self.d);
        std::fs::write(&p, format!("$ cmd {name}\nhead_sha: {head}\nexit_code: {ec}\n--- STDOUT ---\nok {name} {extra}\n"))
            .expect("write log");
        p
    }

    pub fn log(&self, name: &str) -> String {
        self.log_with(name, &main_sha(), 0, "")
    }

    /// `receipt(tid, body=None, pin=True, **over)`.
    pub fn receipt(
        &self,
        tid: &str,
        body: Option<Value>,
        pin: bool,
        over: Value,
    ) -> (String, Option<String>) {
        let lp = self.log(tid);
        let cmd = json!({"cmd": format!("pytest {tid}"), "exit_code": 0, "head_sha": main_sha(), "log_path": lp, "log_sha256": sha(&lp)});
        let mut r = body.unwrap_or_else(|| {
            json!({"task_id": tid, "status": "passed", "head_sha": main_sha(), "commands": [cmd], "verified_at_utc": "2026-10-18T12:00:00Z"})
        });
        if let (Some(r), Some(over)) = (r.as_object_mut(), over.as_object()) {
            for (k, v) in over {
                r.insert(k.clone(), v.clone());
            }
        }
        let rp = format!("{}/verify/main_x/{tid}.json", self.d);
        std::fs::write(&rp, serde_json::to_string(&r).expect("json")).expect("write receipt");
        let pin_hash = if pin { Some(sha(&rp)) } else { None };
        (rp, pin_hash)
    }

    pub fn task(&mut self, tid: &str, reference: Option<(String, Option<String>)>, status: &str) {
        let mut ev = vec![json!("some prose")];
        if let Some((path, pin)) = reference {
            ev.push(json!(match pin {
                Some(h) => format!("{path}#sha256={h}"),
                None => path,
            }));
        }
        self.tasks
            .push(json!({"id": tid, "status": status, "evidence": ev}));
    }

    pub fn write(&self) -> String {
        std::fs::write(
            format!("{}/tasks.json", self.d),
            serde_json::to_string(&json!({"tasks": self.tasks})).expect("json"),
        )
        .expect("write tasks");
        self.d.clone()
    }
}

fn load(rp: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(rp).expect("read")).expect("json")
}

fn save(rp: &str, v: &Value) {
    std::fs::write(rp, serde_json::to_string(v).expect("json")).expect("write");
}

fn check(tid: &str, reference: &(String, Option<String>)) -> Vec<String> {
    check_dope_receipt(
        &J::from(tid),
        &reference.0,
        reference.1.as_deref(),
        &on_main,
        None,
    )
    .expect("check")
    .0
}

fn none() -> Value {
    json!({})
}

/// Ported from tests/test_sc_audit_dope.py::CheckReceipt::test_good_receipt_passes
#[test]
fn test_good_receipt_passes() {
    let p = Packet::new();
    assert_eq!(
        check("A01", &p.receipt("A01", None, true, none())),
        Vec::<String>::new()
    );
}

/// Ported from tests/test_sc_audit_dope.py::CheckReceipt::test_prose_exit_code_rejected
#[test]
fn test_prose_exit_code_rejected() {
    let p = Packet::new();
    let (rp, _) = p.receipt("A01", None, true, none());
    let mut r = load(&rp);
    r["commands"][0]["exit_code"] = json!("0 (all tests passed)");
    save(&rp, &r);
    assert!(check("A01", &(rp, None)).contains(&"exit_code_not_integer".to_string()));
}

/// Ported from tests/test_sc_audit_dope.py::CheckReceipt::test_bool_exit_code_rejected
#[test]
fn test_bool_exit_code_rejected() {
    let p = Packet::new();
    let (rp, _) = p.receipt("A01", None, true, none());
    let mut r = load(&rp);
    r["commands"][0]["exit_code"] = json!(false);
    save(&rp, &r);
    assert!(check("A01", &(rp, None)).contains(&"exit_code_not_integer".to_string()));
}

/// Ported from tests/test_sc_audit_dope.py::CheckReceipt::test_exit_code_list_ints_ok_and_nonzero_flagged
#[test]
fn test_exit_code_list_ints_ok_and_nonzero_flagged() {
    let p = Packet::new();
    let (rp, _) = p.receipt("A01", None, true, none());
    let mut r = load(&rp);
    if let Some(c) = r["commands"][0].as_object_mut() {
        c.remove("exit_code");
        c.insert("exit_codes".to_string(), json!([0, 0]));
    }
    save(&rp, &r);
    assert_eq!(check("A01", &(rp.clone(), None)), Vec::<String>::new());
    r["commands"][0]["exit_codes"] = json!([0, 1]);
    save(&rp, &r);
    assert!(check("A01", &(rp, None)).contains(&"nonzero_exit".to_string()));
}

/// Ported from tests/test_sc_audit_dope.py::CheckReceipt::test_missing_log_and_no_log_path
#[test]
fn test_missing_log_and_no_log_path() {
    let p = Packet::new();
    let (rp, _) = p.receipt("A01", None, true, none());
    let mut r = load(&rp);
    std::fs::remove_file(r["commands"][0]["log_path"].as_str().expect("log path")).expect("rm");
    assert!(check("A01", &(rp.clone(), None)).contains(&"log_missing".to_string()));
    if let Some(c) = r["commands"][0].as_object_mut() {
        c.remove("log_path");
    }
    save(&rp, &r);
    assert!(check("A01", &(rp, None)).contains(&"no_log_path".to_string()));
}

/// Ported from tests/test_sc_audit_dope.py::CheckReceipt::test_log_overwritten
#[test]
fn test_log_overwritten() {
    let p = Packet::new();
    let (rp, _) = p.receipt("A01", None, true, none());
    let r = load(&rp);
    let lp = r["commands"][0]["log_path"]
        .as_str()
        .expect("log path")
        .to_string();
    let mut text = std::fs::read_to_string(&lp).expect("read");
    text.push_str("tampered\n");
    std::fs::write(&lp, text).expect("write");
    assert!(check("A01", &(rp, None)).contains(&"log_sha256_mismatch".to_string()));
}

/// Ported from tests/test_sc_audit_dope.py::CheckReceipt::test_no_log_sha256
#[test]
fn test_no_log_sha256() {
    let p = Packet::new();
    let (rp, _) = p.receipt("A01", None, true, none());
    let mut r = load(&rp);
    r["commands"][0]["log_sha256"] = json!("abc123");
    save(&rp, &r);
    assert!(check("A01", &(rp, None)).contains(&"no_log_sha256".to_string()));
}

/// Ported from tests/test_sc_audit_dope.py::CheckReceipt::test_checkout_sha_differs
#[test]
fn test_checkout_sha_differs() {
    let p = Packet::new();
    let (rp, _) = p.receipt("A01", None, true, none());
    let mut r = load(&rp);
    r["commands"][0]["head_sha"] = json!(other_sha());
    save(&rp, &r);
    assert!(check("A01", &(rp, None)).contains(&"checkout_sha_mismatch".to_string()));
}

/// Ported from tests/test_sc_audit_dope.py::CheckReceipt::test_explicit_checkout_sha_differs
#[test]
fn test_explicit_checkout_sha_differs() {
    let p = Packet::new();
    let (rp, _) = p.receipt("A01", None, true, json!({"checkout_sha": other_sha()}));
    assert!(check("A01", &(rp, None)).contains(&"checkout_sha_mismatch".to_string()));
}

/// Ported from tests/test_sc_audit_dope.py::CheckReceipt::test_sha_not_on_main
#[test]
fn test_sha_not_on_main() {
    let p = Packet::new();
    let lp = p.log_with("A01-other", &other_sha(), 0, "");
    let body = json!({"task_id": "A01", "status": "passed", "head_sha": other_sha(), "commands": [
        {"cmd": "x", "exit_code": 0, "head_sha": other_sha(), "log_path": lp, "log_sha256": sha(&lp)}]});
    let (rp, _) = p.receipt("A01", Some(body), true, none());
    assert_eq!(
        check("A01", &(rp, None)),
        vec!["sha_not_on_main".to_string()]
    );
}

/// Ported from tests/test_sc_audit_dope.py::CheckReceipt::test_truncated_sha
#[test]
fn test_truncated_sha() {
    let p = Packet::new();
    let (rp, _) = p.receipt("A01", None, true, json!({"head_sha": &main_sha()[..9]}));
    assert!(check("A01", &(rp, None)).contains(&"bad_or_missing_sha".to_string()));
}

/// Ported from tests/test_sc_audit_dope.py::CheckReceipt::test_log_header_disagrees
#[test]
fn test_log_header_disagrees() {
    let p = Packet::new();
    let (rp, _) = p.receipt("A01", None, true, none());
    let mut r = load(&rp);
    let lp = p.log_with("A01", &main_sha(), 2, "");
    r["commands"][0]["log_sha256"] = json!(sha(&lp));
    save(&rp, &r);
    assert!(check("A01", &(rp.clone(), None)).contains(&"log_exit_code_mismatch".to_string()));
    let lp = p.log_with("A01", &other_sha(), 0, "");
    r["commands"][0]["log_sha256"] = json!(sha(&lp));
    save(&rp, &r);
    assert!(check("A01", &(rp, None)).contains(&"log_names_other_sha".to_string()));
}

/// Ported from tests/test_sc_audit_dope.py::CheckReceipt::test_receipt_edited_after_pin
#[test]
fn test_receipt_edited_after_pin() {
    let p = Packet::new();
    let (rp, h) = p.receipt("A01", None, true, none());
    let mut text = std::fs::read_to_string(&rp).expect("read");
    text.push(' ');
    std::fs::write(&rp, text).expect("write");
    assert!(check("A01", &(rp, h)).contains(&"receipt_sha256_mismatch".to_string()));
}

/// Ported from tests/test_sc_audit_dope.py::CheckReceipt::test_not_passed_and_no_commands
#[test]
fn test_not_passed_and_no_commands() {
    let p = Packet::new();
    let (rp, _) = p.receipt(
        "A01",
        None,
        true,
        json!({"status": "failed", "commands": []}),
    );
    let got: BTreeSet<String> = check("A01", &(rp, None)).into_iter().collect();
    for want in ["status_not_passed", "no_commands", "no_checkout_sha"] {
        assert!(got.contains(want), "{want} missing from {got:?}");
    }
}

/// Ported from tests/test_sc_audit_dope.py::CheckReceipt::test_receipt_for_other_task
#[test]
fn test_receipt_for_other_task() {
    let p = Packet::new();
    let (rp, _) = p.receipt("A01", None, true, none());
    assert_eq!(
        check("A02", &(rp, None)),
        vec!["receipt_not_for_task".to_string()]
    );
}

/// Ported from tests/test_sc_audit_dope.py::CheckReceipt::test_shared_receipt_without_entries
#[test]
fn test_shared_receipt_without_entries() {
    let p = Packet::new();
    let body = json!({"status": "passed", "head_sha": main_sha(), "exit_code": "0", "covers": ["A01", "A02"]});
    let (rp, _) = p.receipt("SUITE", Some(body), true, none());
    assert_eq!(
        check("A01", &(rp, None)),
        vec!["shared_receipt_no_task_entry".to_string()]
    );
}

/// Ported from tests/test_sc_audit_dope.py::CheckReceipt::test_shared_receipt_with_per_task_entries
#[test]
fn test_shared_receipt_with_per_task_entries() {
    let p = Packet::new();
    let mut e = serde_json::Map::new();
    for t in ["A01", "A02"] {
        let lp = p.log(t);
        e.insert(
            t.to_string(),
            json!({"status": "passed", "commands": [{"cmd": t, "exit_code": 0, "head_sha": main_sha(), "log_path": lp, "log_sha256": sha(&lp)}]}),
        );
    }
    let (rp, _) = p.receipt(
        "SUITE",
        Some(json!({"head_sha": main_sha(), "tasks": e})),
        true,
        none(),
    );
    assert_eq!(check("A01", &(rp.clone(), None)), Vec::<String>::new());
    assert_eq!(
        check("A03", &(rp, None)),
        vec!["shared_receipt_no_task_entry".to_string()]
    );
}

/// Ported from tests/test_sc_audit_dope.py::CheckReceipt::test_predates_commit
#[test]
fn test_predates_commit() {
    let p = Packet::new();
    let (rp, _) = p.receipt(
        "A01",
        None,
        true,
        json!({"verified_at_utc": "2026-10-18T10:00:00Z"}),
    );
    let ct = |_: &str| Some(at("2026-10-18T11:00:00+00:00"));
    let probs = check_dope_receipt(&J::from("A01"), &rp, None, &on_main, Some(&ct))
        .expect("check")
        .0;
    assert!(probs.contains(&"receipt_predates_sha".to_string()));
}

fn dope_audit_cfg(p: &Packet) -> admiralty_audit::AuditConfig {
    let mut cfg = hermetic_config(&format!("{}/home", p.d));
    if let Some(proj) = cfg.projects.iter_mut().find(|x| x.name == "dope") {
        proj.log = "/nonexistent/COMMS.md".to_string();
    }
    cfg
}

fn host() -> FakeHost {
    no_commands_host(at("2026-10-18T14:00:00+00:00"))
}

/// Ported from tests/test_sc_audit_dope.py::AuditDope::test_counts_only_verified
#[test]
fn test_counts_only_verified() {
    let mut p = Packet::new();
    let a01 = p.receipt("A01", None, true, none());
    p.task("A01", Some(a01), "done");
    let (rp, _) = p.receipt("A02", None, true, none());
    let mut r = load(&rp);
    r["commands"][0]["exit_code"] = json!("passed");
    save(&rp, &r);
    p.task("A02", Some((rp, None)), "done");
    p.task("A03", None, "done");
    p.task("A04", None, "todo");
    let (shared, _) = p.receipt("A05", None, true, none());
    p.task("A05", Some((shared.clone(), None)), "done");
    p.task("A06", Some((shared, None)), "done");
    let packet = p.write();
    let cfg = dope_audit_cfg(&p);
    let h = host();
    let mut audit = Audit::new(&cfg, &h);
    let res = audit
        .audit_dope(Some(&packet), Some(&on_main), None)
        .expect("audit");
    let o = res.as_obj().expect("obj");
    assert_eq!(o.get("ledger_done"), Some(&J::Int(5)));
    assert_eq!(o.get("done_recomputed"), Some(&J::Int(2)));
    assert_eq!(
        o.get("unverified_done"),
        Some(&J::from(vec![
            "A02".to_string(),
            "A03".to_string(),
            "A06".to_string()
        ]))
    );
    let got: BTreeSet<String> = audit.codes().into_iter().collect();
    for want in [
        "RECEIPT_EXIT_CODE_NOT_INTEGER",
        "RECEIPT_NO_RECEIPT",
        "RECEIPT_RECEIPT_NOT_FOR_TASK",
        "DONE_UNVERIFIED",
    ] {
        assert!(got.contains(want), "{want} missing from {got:?}");
    }
}

/// Ported from tests/test_sc_audit_dope.py::AuditDope::test_same_log_for_two_tasks_not_counted
#[test]
fn test_same_log_for_two_tasks_not_counted() {
    let mut p = Packet::new();
    let (rp1, _) = p.receipt("A01", None, true, none());
    let lp = load(&rp1)["commands"][0]["log_path"]
        .as_str()
        .expect("lp")
        .to_string();
    let body = json!({"task_id": "A02", "status": "passed", "head_sha": main_sha(), "commands": [
        {"cmd": "x", "exit_code": 0, "head_sha": main_sha(), "log_path": lp, "log_sha256": sha(&lp)}]});
    let (rp2, _) = p.receipt("A02", Some(body), true, none());
    p.task("A01", Some((rp1, None)), "done");
    p.task("A02", Some((rp2, None)), "done");
    let packet = p.write();
    let cfg = dope_audit_cfg(&p);
    let h = host();
    let mut audit = Audit::new(&cfg, &h);
    let res = audit
        .audit_dope(Some(&packet), Some(&on_main), None)
        .expect("audit");
    assert_eq!(
        res.as_obj().and_then(|o| o.get("done_recomputed")),
        Some(&J::Int(0))
    );
    assert!(
        audit
            .codes()
            .contains(&"RECEIPT_LOG_SHARED_ACROSS_TASKS".to_string())
    );
}

/// Ported from tests/test_sc_audit_dope.py::AuditDope::test_generic_command_reported_but_counted
#[test]
fn test_generic_command_reported_but_counted() {
    let mut p = Packet::new();
    for i in 0..DOPE_GENERIC_CMD_MIN {
        let t = format!("G{i:02}");
        let lp = p.log(&t);
        let body = json!({"task_id": t, "status": "passed", "head_sha": main_sha(), "commands": [
            {"cmd": "python3 -m unittest generic", "exit_code": 0, "head_sha": main_sha(), "log_path": lp, "log_sha256": sha(&lp)}]});
        let reference = p.receipt(&t, Some(body), true, none());
        p.task(&t, Some(reference), "done");
    }
    let packet = p.write();
    let cfg = dope_audit_cfg(&p);
    let h = host();
    let mut audit = Audit::new(&cfg, &h);
    let res = audit
        .audit_dope(Some(&packet), Some(&on_main), None)
        .expect("audit");
    assert_eq!(
        res.as_obj().and_then(|o| o.get("done_recomputed")),
        Some(&J::from(DOPE_GENERIC_CMD_MIN))
    );
    assert!(
        audit
            .codes()
            .contains(&"RECEIPT_GENERIC_COMMAND".to_string())
    );
}

/// `GBRedTeam0645._with_log`.
fn with_log(p: &Packet, tid: &str, cmd: &str, body: &str) -> Vec<String> {
    let (rp, _) = p.receipt(tid, None, true, none());
    let mut r = load(&rp);
    let lp = r["commands"][0]["log_path"]
        .as_str()
        .expect("lp")
        .to_string();
    let mut text = std::fs::read_to_string(&lp).expect("read");
    text.push_str(body);
    std::fs::write(&lp, text).expect("write");
    r["commands"][0]["cmd"] = json!(cmd);
    r["commands"][0]["log_sha256"] = json!(sha(&lp));
    save(&rp, &r);
    check_dope_receipt(&J::from(tid), &rp, None, &on_main, None)
        .expect("check")
        .0
}

/// Ported from tests/test_sc_audit_dope.py::GBRedTeam0645::test_cargo_filter_matching_zero_tests_is_vacuous
#[test]
fn test_cargo_filter_matching_zero_tests_is_vacuous() {
    let p = Packet::new();
    let probs = with_log(
        &p,
        "D00",
        "cargo test --locked --offline --lib prepared_view -- --nocapture",
        "running 0 tests\ntest result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 278 filtered out; finished in 0.00s\n",
    );
    assert!(probs.contains(&"vacuous_zero_tests".to_string()));
}

/// Ported from tests/test_sc_audit_dope.py::GBRedTeam0645::test_cargo_with_real_tests_ok
#[test]
fn test_cargo_with_real_tests_ok() {
    let p = Packet::new();
    let probs = with_log(
        &p,
        "R04",
        "cargo test --locked --offline --lib teacher_study -- --nocapture",
        "test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\ntest result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 273 filtered out\n",
    );
    assert_eq!(probs, Vec::<String>::new());
}

/// Ported from tests/test_sc_audit_dope.py::GBRedTeam0645::test_unittest_ran_zero
#[test]
fn test_unittest_ran_zero() {
    let p = Packet::new();
    let probs = with_log(
        &p,
        "X01",
        "PYTHONPATH=. python3 -m unittest x -v",
        "Ran 0 tests in 0.000s\n\nOK\n",
    );
    assert!(probs.contains(&"vacuous_zero_tests".to_string()));
}

/// Ported from tests/test_sc_audit_dope.py::GBRedTeam0645::test_sha256sum_without_args_is_not_a_test_and_empty_hash
#[test]
fn test_sha256sum_without_args_is_not_a_test_and_empty_hash() {
    let p = Packet::new();
    let probs = with_log(
        &p,
        "P00",
        "sha256sum ",
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855  -\n",
    );
    assert!(probs.contains(&"not_a_test_command".to_string()));
    assert!(probs.contains(&"empty_input_hash".to_string()));
}

/// Ported from tests/test_sc_audit_dope.py::GBRedTeam0645::test_git_log_and_gh_run_list_not_tests
#[test]
fn test_git_log_and_gh_run_list_not_tests() {
    let p = Packet::new();
    assert!(
        with_log(&p, "I00", "git -C /x log -n 70 --oneline abc", "")
            .contains(&"not_a_test_command".to_string())
    );
    assert!(
        with_log(&p, "V03", "gh run list --repo a/b --branch main -L 2", "")
            .contains(&"not_a_test_command".to_string())
    );
}

/// Ported from tests/test_sc_audit_dope.py::GBRedTeam0645::test_env_prefix_and_wrappers_skipped
#[test]
fn test_env_prefix_and_wrappers_skipped() {
    assert_eq!(
        dope_cmd_program("PYTHONPATH=. taskset -c 1-2 nice -n 5 python3 -m unittest t"),
        "python3"
    );
    assert_eq!(dope_cmd_program("A=1 B=2 sha256sum f"), "sha256sum");
}
